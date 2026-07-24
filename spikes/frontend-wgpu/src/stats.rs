//! Bounded measurement and deterministic stress-state helpers.

use std::time::{Duration, Instant};

use serde::Serialize;

/// A bounded bag of millisecond samples that computes order statistics on
/// demand. Long soaks must not turn instrumentation into an unbounded memory
/// leak, so samples after `limit` are counted as misses instead of retained.
pub struct Samples {
    values: Vec<f64>,
    limit: usize,
    misses: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct MetricSummary {
    pub sample_count: usize,
    pub misses: u64,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub max: Option<f64>,
}

impl Samples {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            values: Vec::with_capacity(limit.min(4_096)),
            limit,
            misses: 0,
        }
    }

    pub fn push(&mut self, value_ms: f64) {
        if !value_ms.is_finite() || value_ms < 0.0 || self.values.len() >= self.limit {
            self.misses = self.misses.saturating_add(1);
            return;
        }
        self.values.push(value_ms);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn miss(&mut self) {
        self.misses = self.misses.saturating_add(1);
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Nearest-rank percentile in `[0, 100]`. Direct callers receive `0.0`
    /// when empty; structured summaries expose empty percentiles as `null`.
    pub fn percentile(&self, p: f64) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        let mut sorted = self.values.clone();
        sorted.sort_by(f64::total_cmp);
        let p = if p.is_finite() {
            p.clamp(0.0, 100.0)
        } else {
            0.0
        };
        let rank = (p / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
        sorted[rank.min(sorted.len() - 1)]
    }

    pub fn max(&self) -> f64 {
        self.values.iter().copied().fold(0.0, f64::max)
    }

    pub fn summary(&self) -> MetricSummary {
        let populated = !self.values.is_empty();
        MetricSummary {
            sample_count: self.len(),
            misses: self.misses(),
            p50: populated.then(|| self.percentile(50.0)),
            p95: populated.then(|| self.percentile(95.0)),
            max: populated.then(|| self.max()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemorySummary {
    pub sample_count: usize,
    pub misses: u64,
    pub start_rss_bytes: u64,
    pub end_rss_bytes: u64,
    pub min_rss_bytes: u64,
    pub max_rss_bytes: u64,
    pub delta_rss_bytes: i64,
    pub trend_sample_count: usize,
    pub trend_start_rss_bytes: u64,
    pub trend_end_rss_bytes: u64,
    pub trend_delta_rss_bytes: i64,
    /// `Some(true)` means total growth exceeded the tolerance without a
    /// material decrease, including slow growth below the per-sample noise
    /// threshold. Fewer than three samples are inconclusive (`None`).
    pub monotonic_growth: Option<bool>,
}

/// Bounded aggregate memory evidence. It intentionally retains no per-sample
/// vector, so a 30-minute soak and a multi-day soak have identical overhead.
pub struct MemorySamples {
    count: usize,
    misses: u64,
    first: Option<u64>,
    last: Option<u64>,
    min: u64,
    max: u64,
    trend_enabled: bool,
    trend_count: usize,
    trend_first: Option<u64>,
    trend_last: Option<u64>,
    material_decreases: usize,
}

impl Default for MemorySamples {
    fn default() -> Self {
        Self {
            count: 0,
            misses: 0,
            first: None,
            last: None,
            min: 0,
            max: 0,
            trend_enabled: true,
            trend_count: 0,
            trend_first: None,
            trend_last: None,
            material_decreases: 0,
        }
    }
}

impl MemorySamples {
    /// Treat changes of at most 1 MiB as allocator/RSS sampling noise.
    const GROWTH_TOLERANCE_BYTES: u64 = 1024 * 1024;

    pub fn push(&mut self, rss_bytes: Option<u64>) {
        let Some(rss_bytes) = rss_bytes else {
            self.misses = self.misses.saturating_add(1);
            return;
        };
        if self.last.is_none() {
            self.first = Some(rss_bytes);
            self.min = rss_bytes;
            self.max = rss_bytes;
        }
        self.last = Some(rss_bytes);
        self.min = self.min.min(rss_bytes);
        self.max = self.max.max(rss_bytes);
        self.count += 1;
        if self.trend_enabled {
            if let Some(previous) = self.trend_last
                && previous > rss_bytes.saturating_add(Self::GROWTH_TOLERANCE_BYTES)
            {
                self.material_decreases += 1;
            }
            self.trend_first.get_or_insert(rss_bytes);
            self.trend_last = Some(rss_bytes);
            self.trend_count += 1;
        }
    }

    /// Exclude startup cache allocation from the leak trend while preserving
    /// full-run start/end/min/max RSS evidence.
    pub fn pause_trend(&mut self) {
        self.trend_enabled = false;
        self.trend_count = 0;
        self.trend_first = None;
        self.trend_last = None;
        self.material_decreases = 0;
    }

    pub fn begin_trend(&mut self) {
        self.trend_enabled = true;
        self.trend_count = 0;
        self.trend_first = None;
        self.trend_last = None;
        self.material_decreases = 0;
    }

    pub fn summary(&self) -> MemorySummary {
        let first = self.first.unwrap_or(0);
        let last = self.last.unwrap_or(0);
        let trend_first = self.trend_first.unwrap_or(0);
        let trend_last = self.trend_last.unwrap_or(0);
        MemorySummary {
            sample_count: self.count,
            misses: self.misses,
            start_rss_bytes: first,
            end_rss_bytes: last,
            min_rss_bytes: self.first.map_or(0, |_| self.min),
            max_rss_bytes: self.max,
            delta_rss_bytes: i128::from(last)
                .saturating_sub(i128::from(first))
                .clamp(i128::from(i64::MIN), i128::from(i64::MAX))
                as i64,
            trend_sample_count: self.trend_count,
            trend_start_rss_bytes: trend_first,
            trend_end_rss_bytes: trend_last,
            trend_delta_rss_bytes: i128::from(trend_last)
                .saturating_sub(i128::from(trend_first))
                .clamp(i128::from(i64::MIN), i128::from(i64::MAX))
                as i64,
            monotonic_growth: (self.trend_count >= 3).then_some(
                self.material_decreases == 0
                    && trend_last > trend_first.saturating_add(Self::GROWTH_TOLERANCE_BYTES),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StressKind {
    ResizeExercise,
    Soak,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StressAction {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub inject_input: bool,
    pub restart_flood: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct StressSummary {
    pub requested: Option<u64>,
    /// Number of actions the configured schedule requires. For a soak this is
    /// the number of interval slots strictly before its deadline.
    pub expected_actions: u64,
    pub issued: u64,
    /// Soak actions whose input injection was acknowledged by the caller.
    /// This is delivery accounting, not input-to-present latency evidence.
    pub input_actions_issued: u64,
    /// Scheduled soak slots skipped because the event loop did not service the
    /// stress state in time.
    pub cadence_misses: u64,
    pub resize_observed: u64,
    pub scale_applied: u64,
    pub changes_applied: u64,
    pub presented: u64,
    pub misses: u64,
    pub completed: bool,
}

/// Deterministic resize/scale/input schedule. At most one present correlation
/// is pending, making a missed redraw explicit when a later step overtakes it.
pub struct StressState {
    kind: StressKind,
    requested: Option<u64>,
    expected_actions: u64,
    issued: u64,
    input_actions_issued: u64,
    cadence_misses: u64,
    resize_observed: u64,
    scale_applied: u64,
    changes_applied: u64,
    presented: u64,
    misses: u64,
    pending: Option<PendingChange>,
    interval: Duration,
    next_at: Instant,
    deadline: Option<Instant>,
    flood_every_steps: u64,
}

struct PendingChange {
    sequence: u64,
    width: u32,
    height: u32,
    resize_observed: bool,
    scale_applied: bool,
    applied_counted: bool,
}

impl StressState {
    pub fn resize_exercise(start: Instant, steps: u64, interval: Duration) -> Self {
        Self::new(
            StressKind::ResizeExercise,
            Some(steps),
            steps,
            start,
            interval,
            None,
        )
    }

    pub fn soak(start: Instant, duration: Duration, interval: Duration) -> Self {
        let interval = interval.max(Duration::from_millis(1));
        let expected_actions = if duration.is_zero() {
            0
        } else {
            let duration_nanos = duration.as_nanos();
            let interval_nanos = interval.as_nanos();
            duration_nanos
                .div_ceil(interval_nanos)
                .min(u128::from(u64::MAX)) as u64
        };
        Self::new(
            StressKind::Soak,
            None,
            expected_actions,
            start,
            interval,
            Some(start + duration),
        )
    }

    fn new(
        kind: StressKind,
        requested: Option<u64>,
        expected_actions: u64,
        start: Instant,
        interval: Duration,
        deadline: Option<Instant>,
    ) -> Self {
        let interval_ms = interval.as_millis().max(1) as u64;
        Self {
            kind,
            requested,
            expected_actions,
            issued: 0,
            input_actions_issued: 0,
            cadence_misses: 0,
            resize_observed: 0,
            scale_applied: 0,
            changes_applied: 0,
            presented: 0,
            misses: 0,
            pending: None,
            interval,
            next_at: start,
            deadline,
            flood_every_steps: (30_000 / interval_ms).max(1),
        }
    }

    pub fn next_at(&self) -> Instant {
        self.next_at
    }

    pub fn is_due(&self, now: Instant) -> bool {
        now >= self.next_at && !self.is_finished(now)
    }

    pub fn is_finished(&self, now: Instant) -> bool {
        self.requested.is_some_and(|steps| self.issued >= steps)
            || self.deadline.is_some_and(|deadline| now >= deadline)
    }

    pub fn issue(&mut self, now: Instant) -> Option<StressAction> {
        if !self.is_due(now) {
            return None;
        }
        let schedule_advance = if self.kind == StressKind::Soak {
            let due_slots = now
                .duration_since(self.next_at)
                .as_nanos()
                .checked_div(self.interval.as_nanos())
                .unwrap_or(0)
                .saturating_add(1)
                .min(u128::from(
                    self.expected_actions
                        .saturating_sub(self.issued)
                        .saturating_sub(self.cadence_misses),
                ));
            let skipped = due_slots.saturating_sub(1).min(u128::from(u64::MAX)) as u64;
            self.cadence_misses = self.cadence_misses.saturating_add(skipped);
            due_slots.min(u128::from(u64::MAX)) as u64
        } else {
            0
        };
        if self.pending.is_some() {
            self.misses = self.misses.saturating_add(1);
        }
        let sequence = self.issued;
        self.issued += 1;
        // Do not burst stale requests after a stalled event loop. Soaks retain
        // a fixed cadence and explicitly count skipped schedule slots.
        self.next_at = if self.kind == StressKind::Soak {
            self.next_at
                .checked_add(duration_mul(self.interval, schedule_advance))
                .unwrap_or(now + self.interval)
        } else {
            now + self.interval
        };

        const SIZES: [(u32, u32); 5] = [
            (960, 600),
            (1280, 720),
            (800, 640),
            (1440, 900),
            (1024, 768),
        ];
        const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 2.0, 1.0];
        let slot = sequence as usize % SIZES.len();
        let action = StressAction {
            sequence,
            width: SIZES[slot].0,
            height: SIZES[slot].1,
            scale: SCALES[slot],
            inject_input: self.kind == StressKind::Soak,
            restart_flood: self.kind == StressKind::Soak
                && sequence.is_multiple_of(self.flood_every_steps),
        };
        self.pending = Some(PendingChange {
            sequence,
            width: action.width,
            height: action.height,
            resize_observed: false,
            scale_applied: false,
            applied_counted: false,
        });
        Some(action)
    }

    /// Acknowledge that the caller sent the input attached to a soak action.
    /// This deliberately does not imply that a particular frame was caused by
    /// that input.
    pub fn mark_input_issued(&mut self, sequence: u64) {
        if self.kind == StressKind::Soak
            && sequence < self.issued
            && sequence == self.input_actions_issued
        {
            self.input_actions_issued = self.input_actions_issued.saturating_add(1);
        }
    }

    pub fn observe_resize(&mut self, width: u32, height: u32) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        if !pending.resize_observed && (pending.width, pending.height) == (width, height) {
            pending.resize_observed = true;
            self.resize_observed = self.resize_observed.saturating_add(1);
            self.count_applied_if_ready();
        }
    }

    pub fn mark_scale_applied(&mut self, sequence: u64) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        if pending.sequence == sequence && !pending.scale_applied {
            pending.scale_applied = true;
            self.scale_applied = self.scale_applied.saturating_add(1);
            self.count_applied_if_ready();
        }
    }

    fn count_applied_if_ready(&mut self) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        if pending.resize_observed && pending.scale_applied && !pending.applied_counted {
            pending.applied_counted = true;
            self.changes_applied = self.changes_applied.saturating_add(1);
        }
    }

    pub fn presented(&mut self) {
        if self.pending.as_ref().is_some_and(|pending| {
            pending.resize_observed && pending.scale_applied && pending.applied_counted
        }) {
            self.pending = None;
            self.presented = self.presented.saturating_add(1);
        }
    }

    pub fn finish(&mut self, now: Instant) -> StressSummary {
        if self.kind == StressKind::Soak && self.is_finished(now) {
            let accounted = self.issued.saturating_add(self.cadence_misses);
            self.cadence_misses = self
                .cadence_misses
                .saturating_add(self.expected_actions.saturating_sub(accounted));
        }
        if self.pending.is_some() {
            self.pending = None;
            self.misses = self.misses.saturating_add(1);
        }
        self.summary(now)
    }

    pub fn summary(&self, now: Instant) -> StressSummary {
        StressSummary {
            requested: self.requested,
            expected_actions: self.expected_actions,
            issued: self.issued,
            input_actions_issued: self.input_actions_issued,
            cadence_misses: self.cadence_misses,
            resize_observed: self.resize_observed,
            scale_applied: self.scale_applied,
            changes_applied: self.changes_applied,
            presented: self.presented,
            misses: self.misses,
            completed: self.is_finished(now)
                && self.pending.is_none()
                && self.misses == 0
                && self.cadence_misses == 0
                && self.issued == self.expected_actions
                && (self.kind != StressKind::Soak
                    || (self.expected_actions > 0
                        && self.issued > 0
                        && self.input_actions_issued == self.issued))
                && self.changes_applied == self.issued
                && self.presented == self.issued,
        }
    }
}

fn duration_mul(duration: Duration, multiplier: u64) -> Duration {
    let nanos = duration.as_nanos().saturating_mul(u128::from(multiplier));
    let seconds = (nanos / 1_000_000_000).min(u128::from(u64::MAX)) as u64;
    let subsecond_nanos = if seconds == u64::MAX {
        999_999_999
    } else {
        (nanos % 1_000_000_000) as u32
    };
    Duration::new(seconds, subsecond_nanos)
}

#[cfg(test)]
mod tests {
    use super::{MemorySamples, Samples, StressState};
    use std::time::{Duration, Instant};

    #[test]
    fn samples_are_bounded_and_emit_complete_metric_shape() {
        let mut samples = Samples::with_limit(3);
        for value in [30.0, 10.0, 20.0, 40.0, f64::NAN] {
            samples.push(value);
        }
        samples.miss();
        assert_eq!(samples.len(), 3);
        assert_eq!(samples.percentile(50.0), 20.0);
        assert_eq!(samples.percentile(95.0), 30.0);
        assert_eq!(samples.max(), 30.0);
        assert_eq!(samples.summary().misses, 3);
    }

    #[test]
    fn memory_evidence_distinguishes_monotonic_growth_from_a_material_drop() {
        let mib = 1024 * 1024;
        let mut growing = MemorySamples::default();
        growing.push(Some(10 * mib));
        growing.push(Some(12 * mib));
        growing.push(Some(14 * mib));
        assert_eq!(growing.summary().monotonic_growth, Some(true));

        growing.push(Some(11 * mib));
        growing.push(None);
        let summary = growing.summary();
        assert_eq!(summary.monotonic_growth, Some(false));
        assert_eq!(summary.delta_rss_bytes, mib as i64);
        assert_eq!(summary.misses, 1);
    }

    #[test]
    fn memory_evidence_detects_slow_steady_growth_below_interval_tolerance() {
        let kib = 1024;
        let mut memory = MemorySamples::default();
        for rss in [10_000, 10_512, 11_024, 11_536] {
            memory.push(Some(rss * kib));
        }
        assert_eq!(memory.summary().monotonic_growth, Some(true));
    }

    #[test]
    fn memory_trend_can_exclude_startup_growth_without_losing_full_run_rss() {
        let mib = 1024 * 1024;
        let mut memory = MemorySamples::default();
        memory.pause_trend();
        memory.push(Some(10 * mib));
        memory.push(Some(30 * mib));
        memory.begin_trend();
        memory.push(Some(30 * mib));
        memory.push(Some(30 * mib));
        memory.push(Some(30 * mib));
        let summary = memory.summary();
        assert_eq!(summary.start_rss_bytes, 10 * mib);
        assert_eq!(summary.delta_rss_bytes, 20 * mib as i64);
        assert_eq!(summary.trend_sample_count, 3);
        assert_eq!(summary.trend_delta_rss_bytes, 0);
        assert_eq!(summary.monotonic_growth, Some(false));
    }

    #[test]
    fn resize_exercise_is_deterministic_and_counts_overtaken_presents() {
        let start = Instant::now();
        let mut state = StressState::resize_exercise(start, 2, Duration::from_millis(10));
        let first = state.issue(start).expect("first action");
        assert_eq!((first.width, first.height, first.scale), (960, 600, 1.0));
        assert!(state.issue(start + Duration::from_millis(5)).is_none());
        let second = state
            .issue(start + Duration::from_millis(10))
            .expect("second action");
        assert_eq!(
            (second.width, second.height, second.scale),
            (1280, 720, 1.25)
        );
        state.mark_scale_applied(second.sequence);
        state.observe_resize(second.width, second.height);
        state.presented();
        let summary = state.finish(start + Duration::from_millis(20));
        assert_eq!(summary.requested, Some(2));
        assert_eq!(summary.issued, 2);
        assert_eq!(summary.resize_observed, 1);
        assert_eq!(summary.scale_applied, 1);
        assert_eq!(summary.changes_applied, 1);
        assert_eq!(summary.presented, 1);
        assert_eq!(summary.misses, 1);
        assert!(!summary.completed, "a missed redraw is not a passing run");
    }

    #[test]
    fn stress_present_only_counts_after_matching_resize_and_scale() {
        let start = Instant::now();
        let mut state = StressState::resize_exercise(start, 1, Duration::from_millis(10));
        let action = state.issue(start).expect("action");
        state.presented();
        assert_eq!(state.summary(start).presented, 0);
        state.observe_resize(action.width + 1, action.height);
        state.mark_scale_applied(action.sequence);
        state.presented();
        assert_eq!(state.summary(start).presented, 0);
        state.observe_resize(action.width, action.height);
        state.presented();
        let summary = state.finish(start + Duration::from_millis(10));
        assert_eq!(summary.resize_observed, 1);
        assert_eq!(summary.scale_applied, 1);
        assert_eq!(summary.changes_applied, 1);
        assert_eq!(summary.presented, 1);
        assert_eq!(summary.misses, 0);
        assert!(summary.completed);
    }

    #[test]
    fn zero_action_soak_fails_and_accounts_for_every_scheduled_slot() {
        let start = Instant::now();
        let mut state =
            StressState::soak(start, Duration::from_millis(30), Duration::from_millis(10));

        let summary = state.finish(start + Duration::from_millis(30));

        assert_eq!(summary.expected_actions, 3);
        assert_eq!(summary.issued, 0);
        assert_eq!(summary.input_actions_issued, 0);
        assert_eq!(summary.cadence_misses, 3);
        assert!(!summary.completed);
    }

    #[test]
    fn zero_duration_soak_cannot_pass_vacuously() {
        let start = Instant::now();
        let mut state = StressState::soak(start, Duration::ZERO, Duration::from_millis(10));

        let summary = state.finish(start);

        assert_eq!(summary.expected_actions, 0);
        assert_eq!(summary.issued, 0);
        assert_eq!(summary.cadence_misses, 0);
        assert!(!summary.completed);
    }

    #[test]
    fn complete_soak_requires_all_actions_input_acks_and_presents() {
        let start = Instant::now();
        let mut state =
            StressState::soak(start, Duration::from_millis(30), Duration::from_millis(10));

        for offset_ms in [0, 10, 20] {
            let action = state
                .issue(start + Duration::from_millis(offset_ms))
                .expect("scheduled soak action");
            assert!(action.inject_input);
            state.mark_input_issued(action.sequence);
            state.mark_scale_applied(action.sequence);
            state.observe_resize(action.width, action.height);
            state.presented();
        }

        let summary = state.finish(start + Duration::from_millis(30));
        assert_eq!(summary.expected_actions, 3);
        assert_eq!(summary.issued, 3);
        assert_eq!(summary.input_actions_issued, 3);
        assert_eq!(summary.cadence_misses, 0);
        assert_eq!(summary.changes_applied, 3);
        assert_eq!(summary.presented, 3);
        assert!(summary.completed);
    }

    #[test]
    fn few_action_soak_fails_with_trailing_cadence_misses() {
        let start = Instant::now();
        let mut state =
            StressState::soak(start, Duration::from_millis(30), Duration::from_millis(10));
        let action = state.issue(start).expect("first action");
        state.mark_input_issued(action.sequence);
        state.mark_scale_applied(action.sequence);
        state.observe_resize(action.width, action.height);
        state.presented();

        let summary = state.finish(start + Duration::from_millis(30));

        assert_eq!(summary.expected_actions, 3);
        assert_eq!(summary.issued, 1);
        assert_eq!(summary.input_actions_issued, 1);
        assert_eq!(summary.cadence_misses, 2);
        assert_eq!(summary.issued + summary.cadence_misses, 3);
        assert!(!summary.completed);
    }

    #[test]
    fn delayed_soak_service_records_missed_fixed_cadence() {
        let start = Instant::now();
        let mut state =
            StressState::soak(start, Duration::from_millis(40), Duration::from_millis(10));

        let first = state.issue(start).expect("first action");
        state.mark_input_issued(first.sequence);
        state.mark_scale_applied(first.sequence);
        state.observe_resize(first.width, first.height);
        state.presented();

        let delayed = state
            .issue(start + Duration::from_millis(25))
            .expect("one bounded action after a stalled callback");
        state.mark_input_issued(delayed.sequence);
        state.mark_scale_applied(delayed.sequence);
        state.observe_resize(delayed.width, delayed.height);
        state.presented();

        let summary = state.finish(start + Duration::from_millis(40));
        assert_eq!(summary.expected_actions, 4);
        assert_eq!(summary.issued, 2);
        assert_eq!(summary.cadence_misses, 2);
        assert_eq!(summary.issued + summary.cadence_misses, 4);
        assert!(!summary.completed);
    }
}
