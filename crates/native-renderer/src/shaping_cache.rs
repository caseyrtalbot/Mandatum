//! Bounded identity and retention for admitted native text shaping.
//!
//! The cache owns no scene placement. A key describes one complete shaping
//! unit: its text, resolved rich-text styles, declared byte-to-cell topology,
//! font/metrics generations, and shaping-policy generation. This prevents a
//! rejected or differently mapped run from borrowing an admitted layout.

use std::hash::Hash;
use std::mem::size_of;
use std::num::NonZeroUsize;

#[cfg(test)]
use crate::row_run::ResolvedGlyphStyle;
use crate::row_run::{ByteCellSpan, RowRun, RowRunStyleRange};
use lru::LruCache;

pub(crate) const MAX_SHAPING_CACHE_ENTRIES: usize = 4_096;
pub(crate) const MAX_SHAPING_CACHE_ACCOUNTED_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_SHAPING_CACHE_ENTRY_ACCOUNTED_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShapingCacheContext {
    pub(crate) font_generation: u64,
    pub(crate) scale_generation: u64,
    /// Generation of the complete typography metric set.
    pub(crate) metric_generation: u64,
    /// Stable slot within that set (terminal, interface, label, caption).
    pub(crate) metric_slot: u8,
    pub(crate) renderer_config_generation: u64,
    pub(crate) font_size_bits: u32,
    pub(crate) line_height_bits: u32,
    pub(crate) cell_width_bits: u32,
    pub(crate) cell_height_bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ShapingStyleKey {
    bytes_start: usize,
    bytes_end: usize,
    foreground: [u8; 4],
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

impl From<&RowRunStyleRange> for ShapingStyleKey {
    fn from(range: &RowRunStyleRange) -> Self {
        Self {
            bytes_start: range.bytes.start,
            bytes_end: range.bytes.end,
            foreground: range.style.foreground,
            bold: range.style.bold,
            italic: range.style.italic,
            underline: range.style.underline,
            strikethrough: range.style.strikethrough,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ByteCellSpanKey {
    bytes_start: usize,
    bytes_end: usize,
    cells_start: u16,
    cells_end: u16,
}

impl From<&ByteCellSpan> for ByteCellSpanKey {
    fn from(span: &ByteCellSpan) -> Self {
        Self {
            bytes_start: span.bytes.start,
            bytes_end: span.bytes.end,
            cells_start: span.cells.start,
            cells_end: span.cells.end,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShapingCacheKey {
    context: ShapingCacheContext,
    width: u16,
    text: Box<str>,
    styles: Box<[ShapingStyleKey]>,
    byte_cells: Box<[ByteCellSpanKey]>,
}

impl ShapingCacheKey {
    pub(crate) fn from_run(run: &RowRun, context: ShapingCacheContext) -> Self {
        Self {
            context,
            width: run.width,
            text: run.text.clone().into_boxed_str(),
            styles: run.style_ranges.iter().map(ShapingStyleKey::from).collect(),
            byte_cells: run.byte_cells.iter().map(ByteCellSpanKey::from).collect(),
        }
    }

    /// Exact bytes directly owned by the key.
    pub(crate) fn owned_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.text.len())
            .saturating_add(
                self.styles
                    .len()
                    .saturating_mul(size_of::<ShapingStyleKey>()),
            )
            .saturating_add(
                self.byte_cells
                    .len()
                    .saturating_mul(size_of::<ByteCellSpanKey>()),
            )
    }

    #[cfg(test)]
    fn synthetic(
        text: &str,
        width: u16,
        style: ResolvedGlyphStyle,
        byte_cells: impl IntoIterator<Item = (std::ops::Range<usize>, std::ops::Range<u16>)>,
        context: ShapingCacheContext,
    ) -> Self {
        Self {
            context,
            width,
            text: text.into(),
            styles: vec![ShapingStyleKey {
                bytes_start: 0,
                bytes_end: text.len(),
                foreground: style.foreground,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                strikethrough: style.strikethrough,
            }]
            .into_boxed_slice(),
            byte_cells: byte_cells
                .into_iter()
                .map(|(bytes, cells)| ByteCellSpanKey {
                    bytes_start: bytes.start,
                    bytes_end: bytes.end,
                    cells_start: cells.start,
                    cells_end: cells.end,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShapingCacheStats {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) evictions: u64,
    pub(crate) rejections: u64,
    pub(crate) invalidations: u64,
    pub(crate) entries_high_water: usize,
    pub(crate) accounted_bytes_high_water: usize,
}

#[derive(Debug)]
struct CacheEntry<V> {
    value: V,
    accounted_bytes: usize,
}

/// Count-and-byte bounded least-recently-used cache.
///
/// `accounted_bytes` is supplied by the renderer's explicit conservative
/// accounting contract. Cosmic-text does not expose every internal allocation
/// capacity, so this metric is intentionally not named exact resident bytes.
/// The hard entry ceiling remains independent. The cache refuses a single
/// oversized value and evicts before insertion, so neither accounted ceiling
/// can refill or be bypassed.
#[derive(Debug)]
pub(crate) struct ShapingCache<V> {
    entries: LruCache<ShapingCacheKey, CacheEntry<V>>,
    accounted_bytes: usize,
    max_entries: usize,
    max_accounted_bytes: usize,
    max_entry_accounted_bytes: usize,
    stats: ShapingCacheStats,
}

impl<V: Clone> ShapingCache<V> {
    pub(crate) fn new() -> Self {
        Self::with_limits(
            MAX_SHAPING_CACHE_ENTRIES,
            MAX_SHAPING_CACHE_ACCOUNTED_BYTES,
            MAX_SHAPING_CACHE_ENTRY_ACCOUNTED_BYTES,
        )
    }

    fn with_limits(
        max_entries: usize,
        max_accounted_bytes: usize,
        max_entry_accounted_bytes: usize,
    ) -> Self {
        Self {
            entries: LruCache::new(
                NonZeroUsize::new(max_entries.max(1)).expect("cache capacity is nonzero"),
            ),
            accounted_bytes: 0,
            max_entries,
            max_accounted_bytes,
            max_entry_accounted_bytes,
            stats: ShapingCacheStats::default(),
        }
    }

    pub(crate) fn get_cloned(&mut self, key: &ShapingCacheKey) -> Option<V> {
        let Some(entry) = self.entries.get(key) else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            return None;
        };
        self.stats.hits = self.stats.hits.saturating_add(1);
        Some(entry.value.clone())
    }

    pub(crate) fn insert(
        &mut self,
        key: ShapingCacheKey,
        value: V,
        accounted_bytes: usize,
    ) -> bool {
        if self.max_entries == 0
            || accounted_bytes > self.max_accounted_bytes
            || accounted_bytes > self.max_entry_accounted_bytes
            || accounted_bytes == usize::MAX
        {
            self.stats.rejections = self.stats.rejections.saturating_add(1);
            return false;
        }

        if let Some(replaced) = self.entries.pop(&key) {
            self.accounted_bytes = self
                .accounted_bytes
                .saturating_sub(replaced.accounted_bytes);
        }

        while self.entries.len() >= self.max_entries
            || self
                .accounted_bytes
                .checked_add(accounted_bytes)
                .is_none_or(|bytes| bytes > self.max_accounted_bytes)
        {
            if !self.evict_lru() {
                self.stats.rejections = self.stats.rejections.saturating_add(1);
                return false;
            }
        }

        self.accounted_bytes = self.accounted_bytes.saturating_add(accounted_bytes);
        self.entries.put(
            key,
            CacheEntry {
                value,
                accounted_bytes,
            },
        );
        self.stats.entries_high_water = self.stats.entries_high_water.max(self.entries.len());
        self.stats.accounted_bytes_high_water = self
            .stats
            .accounted_bytes_high_water
            .max(self.accounted_bytes);
        true
    }

    pub(crate) fn invalidate(&mut self) {
        if !self.entries.is_empty() {
            self.stats.invalidations = self.stats.invalidations.saturating_add(1);
        }
        self.entries = LruCache::new(
            NonZeroUsize::new(self.max_entries.max(1)).expect("cache capacity is nonzero"),
        );
        self.accounted_bytes = 0;
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn accounted_bytes(&self) -> usize {
        self.accounted_bytes
    }

    pub(crate) fn stats(&self) -> ShapingCacheStats {
        self.stats
    }

    pub(crate) fn preserve_stats_after_cold_reset(
        &mut self,
        previous: ShapingCacheStats,
        invalidated_entries: bool,
    ) {
        debug_assert!(self.entries.is_empty());
        debug_assert_eq!(self.accounted_bytes, 0);
        self.stats = previous;
        if invalidated_entries {
            self.stats.invalidations = self.stats.invalidations.saturating_add(1);
        }
    }

    fn evict_lru(&mut self) -> bool {
        let Some((_, removed)) = self.entries.pop_lru() else {
            return false;
        };
        self.accounted_bytes = self.accounted_bytes.saturating_sub(removed.accounted_bytes);
        self.stats.evictions = self.stats.evictions.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> ResolvedGlyphStyle {
        ResolvedGlyphStyle {
            foreground: [220, 220, 224, 255],
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }

    fn context(generation: u64) -> ShapingCacheContext {
        ShapingCacheContext {
            font_generation: 7,
            scale_generation: generation,
            metric_generation: 11,
            metric_slot: 0,
            renderer_config_generation: 1,
            font_size_bits: 15.0_f32.to_bits(),
            line_height_bits: 20.0_f32.to_bits(),
            cell_width_bits: 9.0_f32.to_bits(),
            cell_height_bits: 20.0_f32.to_bits(),
        }
    }

    fn key(text: &str, generation: u64) -> ShapingCacheKey {
        ShapingCacheKey::synthetic(
            text,
            text.len() as u16,
            style(),
            text.char_indices().map(|(start, character)| {
                let end = start + character.len_utf8();
                (start..end, start as u16..end as u16)
            }),
            context(generation),
        )
    }

    #[test]
    fn exact_unit_hits_but_generation_and_topology_changes_miss() {
        let exact = key("fi", 1);
        let next_generation = key("fi", 2);
        let different_topology =
            ShapingCacheKey::synthetic("fi", 1, style(), [(0..2, 0..1)], context(1));
        let mut cache = ShapingCache::with_limits(8, 8_192, 4_096);

        assert!(cache.get_cloned(&exact).is_none());
        assert!(cache.insert(exact.clone(), 17_u8, 256));
        assert_eq!(cache.get_cloned(&exact), Some(17));
        assert!(cache.get_cloned(&next_generation).is_none());
        assert!(cache.get_cloned(&different_topology).is_none());
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 3);
    }

    #[test]
    fn every_generation_and_metric_bit_participates_in_identity() {
        let base = context(1);
        let exact = ShapingCacheKey::synthetic("x", 1, style(), [(0..1, 0..1)], base);
        let variants = [
            ShapingCacheContext {
                font_generation: base.font_generation + 1,
                ..base
            },
            ShapingCacheContext {
                scale_generation: base.scale_generation + 1,
                ..base
            },
            ShapingCacheContext {
                metric_generation: base.metric_generation + 1,
                ..base
            },
            ShapingCacheContext {
                metric_slot: base.metric_slot + 1,
                ..base
            },
            ShapingCacheContext {
                renderer_config_generation: base.renderer_config_generation + 1,
                ..base
            },
            ShapingCacheContext {
                font_size_bits: 16.0_f32.to_bits(),
                ..base
            },
            ShapingCacheContext {
                line_height_bits: 21.0_f32.to_bits(),
                ..base
            },
            ShapingCacheContext {
                cell_width_bits: 10.0_f32.to_bits(),
                ..base
            },
            ShapingCacheContext {
                cell_height_bits: 21.0_f32.to_bits(),
                ..base
            },
        ];
        let mut cache = ShapingCache::with_limits(16, 16_384, 4_096);
        assert!(cache.insert(exact.clone(), 9_u8, 256));
        assert_eq!(cache.get_cloned(&exact), Some(9));
        for context in variants {
            let variant = ShapingCacheKey::synthetic("x", 1, style(), [(0..1, 0..1)], context);
            assert!(cache.get_cloned(&variant).is_none());
        }
    }

    #[test]
    fn count_and_byte_limits_evict_without_refill_or_overflow() {
        let mut cache = ShapingCache::with_limits(2, 600, 600);
        assert!(cache.insert(key("a", 1), 1_u8, 300));
        assert!(cache.insert(key("b", 1), 2_u8, 300));
        assert!(cache.insert(key("c", 1), 3_u8, 300));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.accounted_bytes(), 600);
        assert!(cache.get_cloned(&key("a", 1)).is_none());
        assert_eq!(cache.get_cloned(&key("c", 1)), Some(3));
        assert!(!cache.insert(key("oversized", 1), 4_u8, 601));
        assert!(!cache.insert(key("overflow", 1), 5_u8, usize::MAX));
        assert!(cache.len() <= 2);
        assert!(cache.accounted_bytes() <= 600);
        assert!(cache.stats().entries_high_water <= 2);
        assert!(cache.stats().accounted_bytes_high_water <= 600);
    }

    #[test]
    fn aggregate_and_single_entry_byte_ceilings_are_independent() {
        let mut cache = ShapingCache::with_limits(4, 600, 400);
        assert!(cache.insert(key("a", 1), 1_u8, 300));
        assert!(cache.insert(key("b", 1), 2_u8, 300));
        assert!(cache.insert(key("c", 1), 3_u8, 300));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.accounted_bytes(), 600);

        assert!(!cache.insert(key("single-too-large", 1), 4_u8, 401));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.accounted_bytes(), 600);
        assert_eq!(cache.stats().rejections, 1);
    }

    #[test]
    fn count_pressure_evicts_the_true_lru_entry() {
        let first = key("first", 1);
        let second = key("second", 1);
        let third = key("third", 1);
        let mut cache = ShapingCache::with_limits(2, 10_000, 10_000);
        assert!(cache.insert(first.clone(), 1_u8, 100));
        assert!(cache.insert(second.clone(), 2_u8, 100));
        assert_eq!(cache.get_cloned(&first), Some(1));
        assert!(cache.insert(third.clone(), 3_u8, 100));

        assert_eq!(cache.get_cloned(&first), Some(1));
        assert!(cache.get_cloned(&second).is_none());
        assert_eq!(cache.get_cloned(&third), Some(3));
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn checked_add_overflow_evicts_instead_of_bypassing_the_ceiling() {
        let mut cache = ShapingCache::with_limits(3, usize::MAX, usize::MAX.saturating_sub(1));
        assert!(cache.insert(key("near-max", 1), 1_u8, usize::MAX.saturating_sub(10),));
        assert!(cache.insert(key("overflowing-add", 1), 2_u8, 20));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.accounted_bytes(), 20);
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn oversized_replacement_preserves_the_previous_value() {
        let exact = key("same", 1);
        let mut cache = ShapingCache::with_limits(4, 1_000, 500);
        assert!(cache.insert(exact.clone(), 1_u8, 300));
        assert!(!cache.insert(exact.clone(), 2_u8, 501));
        assert_eq!(cache.get_cloned(&exact), Some(1));
        assert_eq!(cache.accounted_bytes(), 300);
    }

    #[test]
    fn long_churn_never_exceeds_either_aggregate_bound() {
        let mut cache = ShapingCache::with_limits(8, 2_000, 500);
        for index in 0..1_000 {
            let text = format!("entry-{index}");
            assert!(cache.insert(key(&text, 1), index, 300));
            assert!(cache.len() <= 8);
            assert!(cache.accounted_bytes() <= 2_000);
            assert!(cache.stats().entries_high_water <= 8);
            assert!(cache.stats().accounted_bytes_high_water <= 2_000);
        }
    }

    #[test]
    fn replacement_is_charged_once_and_invalidation_clears_current_retention() {
        let exact = key("same", 1);
        let mut cache = ShapingCache::with_limits(4, 1_000, 1_000);
        assert!(cache.insert(exact.clone(), 1_u8, 200));
        assert!(cache.insert(exact.clone(), 2_u8, 400));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.accounted_bytes(), 400);
        assert_eq!(cache.get_cloned(&exact), Some(2));

        cache.invalidate();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.accounted_bytes(), 0);
        assert_eq!(cache.stats().invalidations, 1);
    }

    #[test]
    fn cold_reset_can_preserve_lifetime_stats_without_retaining_values() {
        let exact = key("same", 1);
        let mut previous = ShapingCache::with_limits(4, 1_000, 1_000);
        assert!(previous.get_cloned(&exact).is_none());
        assert!(previous.insert(exact.clone(), 2_u8, 400));
        assert_eq!(previous.get_cloned(&exact), Some(2));

        let mut replacement = ShapingCache::<u8>::with_limits(4, 1_000, 1_000);
        replacement.preserve_stats_after_cold_reset(previous.stats(), !previous.entries.is_empty());
        assert_eq!(replacement.len(), 0);
        assert_eq!(replacement.accounted_bytes(), 0);
        assert_eq!(replacement.stats().hits, 1);
        assert_eq!(replacement.stats().misses, 1);
        assert_eq!(replacement.stats().invalidations, 1);
    }
}
