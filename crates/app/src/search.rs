//! Session search: plain text search across every live pane's
//! scrollback+screen text and the execution-timeline tail. Honest scope:
//! exact/fuzzy subsequence matching over a snapshot taken when the overlay
//! opens — never embeddings, never a live re-index per keystroke.
//!
//! # Interaction contract
//!
//! "Search session output" (chord `ctrl+shift+f`, the fuzzy palette, the
//! pane context menu) snapshots the searchable text once — each live
//! terminal grid's
//! scrollback+screen (bounded at 2000 rows by the grid), each task pane's
//! output grid, each agent pane's output tail, and the timeline tail — so
//! results stay stable while panes flood; reopen to search newer output.
//!
//! Typing filters the snapshot. Query grammar (tokens AND together):
//!
//! - `pane:<substring>` matches the source pane's title or id
//! - `kind:<terminal|task|agent|timeline>` matches the source family
//!   (prefix match, like the timeline's `kind:` filter)
//! - anything else must fuzzy-subsequence-match the line
//!   ([`mandatum_commands::fuzzy`]), matched chars highlighted
//!
//! Results are grouped by source in pane order (timeline last), most recent
//! first within each group, capped at [`MAX_SEARCH_RESULTS`] with an honest
//! "+N more" count. Enter on a pane hit focuses the pane and (for terminal
//! panes) scrolls its viewport to the matched row through the pointer-view
//! mechanics; Enter on a timeline hit opens the timeline overlay positioned
//! at that entry. Esc returns. The footer names the keys.

use std::collections::BTreeSet;

use mandatum_commands::fuzzy::fuzzy_match;
use mandatum_core::PaneId;
use mandatum_scene::{
    SceneSize, SearchEntry, SearchOverlay,
    layout::{palette_item_window, pane_inner_rect, search_overlay_rect},
};
use mandatum_terminal_vt::TerminalGrid;

use crate::timeline::TimelineEvent;

/// Display cap for one query's results; the overflow count keeps the list
/// honest.
pub(crate) const MAX_SEARCH_RESULTS: usize = 200;

/// What a pane source is, for the `kind:` filter and the source label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchSourceKind {
    Terminal,
    Task,
    Agent,
}

impl SearchSourceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Task => "task",
            Self::Agent => "agent",
        }
    }
}

/// One line of searchable text with the absolute buffer row it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchLine {
    /// Absolute row in the source's scrollback+screen buffer (or tail index
    /// for agent output, where no viewport jump exists).
    pub(crate) row: usize,
    pub(crate) text: String,
}

/// One pane's snapshotted text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchSource {
    pub(crate) pane_id: PaneId,
    pub(crate) title: String,
    pub(crate) kind: SearchSourceKind,
    pub(crate) lines: Vec<SearchLine>,
}

impl SearchSource {
    /// Snapshot a terminal grid: every scrollback+screen row, trailing
    /// whitespace trimmed, blank rows skipped.
    pub(crate) fn from_grid(
        pane_id: PaneId,
        title: &str,
        kind: SearchSourceKind,
        grid: &TerminalGrid,
    ) -> Self {
        let scrollback_len = grid.scrollback_len();
        let lines = (0..grid.total_rows())
            .filter_map(|row| {
                let text = if row < scrollback_len {
                    grid.scrollback_row_text(row)?
                } else {
                    grid.row_text((row - scrollback_len) as u16)?
                };
                let text = text.trim_end().to_owned();
                (!text.is_empty()).then_some(SearchLine { row, text })
            })
            .collect();
        Self {
            pane_id,
            title: title.to_owned(),
            kind,
            lines,
        }
    }

    /// Snapshot plain output lines (agent tails).
    pub(crate) fn from_lines<'a>(
        pane_id: PaneId,
        title: &str,
        kind: SearchSourceKind,
        lines: impl Iterator<Item = &'a String>,
    ) -> Self {
        let lines = lines
            .enumerate()
            .filter_map(|(row, text)| {
                let text = text.trim_end().to_owned();
                (!text.is_empty()).then_some(SearchLine { row, text })
            })
            .collect();
        Self {
            pane_id,
            title: title.to_owned(),
            kind,
            lines,
        }
    }

    fn label(&self) -> String {
        format!("{} · {} ({})", self.title, self.pane_id, self.kind.label())
    }
}

/// The text snapshot one search session runs over.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct SearchCorpus {
    /// Pane sources in session pane order.
    pub(crate) sources: Vec<SearchSource>,
    /// Timeline tail, newest first.
    pub(crate) timeline: Vec<TimelineEvent>,
}

/// Where Enter on a result goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchHitTarget {
    /// Focus the pane; for terminal panes, scroll the viewport to `row`.
    PaneRow {
        pane_id: PaneId,
        row: usize,
        kind: SearchSourceKind,
    },
    /// Open the timeline overlay positioned at this event.
    Timeline { event: TimelineEvent },
}

/// One search result: the display entry data plus its jump target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub(crate) source: String,
    pub(crate) text: String,
    pub(crate) match_indices: Vec<usize>,
    pub(crate) target: SearchHitTarget,
}

/// Live overlay state while search is open. The corpus is fixed at open;
/// results recompute from it on every query edit.
pub(crate) struct SearchViewState {
    pub(crate) query: String,
    pub(crate) selected: usize,
    corpus: SearchCorpus,
    pub(crate) results: Vec<SearchHit>,
    /// Total matches before the display cap.
    pub(crate) total_hits: usize,
}

impl SearchViewState {
    pub(crate) fn new(corpus: SearchCorpus) -> Self {
        let mut view = Self {
            query: String::new(),
            selected: 0,
            corpus,
            results: Vec::new(),
            total_hits: 0,
        };
        view.refresh();
        view
    }

    pub(crate) fn source_count(&self) -> usize {
        self.corpus.sources.len()
    }

    pub(crate) fn timeline_event_count(&self) -> usize {
        self.corpus.timeline.len()
    }

    /// Recompute results for the current query and reset the selection.
    pub(crate) fn refresh(&mut self) {
        let (results, total_hits) = search(&self.corpus, &self.query);
        self.results = results;
        self.total_hits = total_hits;
        self.selected = 0;
    }
}

/// The parsed query: structured filters plus plain fuzzy terms.
struct ParsedQuery {
    pane_filters: Vec<String>,
    kind_filters: Vec<String>,
    terms: Vec<String>,
}

impl ParsedQuery {
    fn parse(query: &str) -> Self {
        let mut parsed = Self {
            pane_filters: Vec::new(),
            kind_filters: Vec::new(),
            terms: Vec::new(),
        };
        for token in query.split_whitespace() {
            if let Some(pane) = token.strip_prefix("pane:") {
                parsed.pane_filters.push(pane.to_ascii_lowercase());
            } else if let Some(kind) = token.strip_prefix("kind:") {
                parsed.kind_filters.push(kind.to_ascii_lowercase());
            } else {
                parsed.terms.push(token.to_owned());
            }
        }
        parsed
    }

    fn is_empty(&self) -> bool {
        self.pane_filters.is_empty() && self.kind_filters.is_empty() && self.terms.is_empty()
    }

    /// Every `kind:` token must prefix-match the source family (the same
    /// rule as the timeline's `kind:` filter).
    fn passes_kind(&self, kind_label: &str) -> bool {
        self.kind_filters
            .iter()
            .all(|filter| kind_label.starts_with(filter.as_str()))
    }

    /// Every `pane:` token must substring-match the pane title or id,
    /// case-insensitively. Timeline entries have no pane and fail any
    /// `pane:` filter.
    fn passes_pane(&self, title: &str, pane_id: &str) -> bool {
        let title = title.to_ascii_lowercase();
        let pane_id = pane_id.to_ascii_lowercase();
        self.pane_filters
            .iter()
            .all(|filter| title.contains(filter.as_str()) || pane_id.contains(filter.as_str()))
    }

    /// Every plain term must fuzzy-subsequence-match the line. Cheap linear
    /// pre-check; the DP highlight pass runs only on hits that get shown.
    fn passes_terms(&self, line: &str) -> bool {
        self.terms
            .iter()
            .all(|term| subsequence_matches(term, line))
    }

    /// Merged highlight indices of every term, for a line already known to
    /// match.
    fn highlight_indices(&self, line: &str) -> Vec<usize> {
        let mut merged = BTreeSet::new();
        for term in &self.terms {
            if let Some(hit) = fuzzy_match(term, line) {
                merged.extend(hit.indices);
            }
        }
        merged.into_iter().collect()
    }
}

/// Case-insensitive subsequence containment, linear in the line length.
fn subsequence_matches(term: &str, line: &str) -> bool {
    let mut needle = term.chars().map(|c| c.to_ascii_lowercase()).peekable();
    for hay in line.chars() {
        match needle.peek() {
            Some(&next) if hay.to_ascii_lowercase() == next => {
                needle.next();
            }
            Some(_) => {}
            None => return true,
        }
    }
    needle.peek().is_none()
}

/// Run one query over the corpus: hits grouped by source in corpus order
/// (timeline last), most recent first within each group, capped at
/// [`MAX_SEARCH_RESULTS`]. Returns `(shown, total)` so the overlay can say
/// "+N more". An empty query matches nothing (calm over noisy).
pub(crate) fn search(corpus: &SearchCorpus, query: &str) -> (Vec<SearchHit>, usize) {
    let parsed = ParsedQuery::parse(query);
    if parsed.is_empty() {
        return (Vec::new(), 0);
    }

    let mut results = Vec::new();
    let mut total = 0usize;

    for source in &corpus.sources {
        if !parsed.passes_kind(source.kind.label())
            || !parsed.passes_pane(&source.title, source.pane_id.as_str())
        {
            continue;
        }
        // Most recent first: the bottom of the buffer is the newest output.
        for line in source.lines.iter().rev() {
            if !parsed.passes_terms(&line.text) {
                continue;
            }
            total += 1;
            if results.len() < MAX_SEARCH_RESULTS {
                results.push(SearchHit {
                    source: source.label(),
                    text: line.text.clone(),
                    match_indices: parsed.highlight_indices(&line.text),
                    target: SearchHitTarget::PaneRow {
                        pane_id: source.pane_id.clone(),
                        row: line.row,
                        kind: source.kind,
                    },
                });
            }
        }
    }

    // Timeline entries: no pane identity, family "timeline", newest first.
    if parsed.pane_filters.is_empty() && parsed.passes_kind("timeline") {
        for event in &corpus.timeline {
            let text = event.kind.describe();
            if !parsed.passes_terms(&text) {
                continue;
            }
            total += 1;
            if results.len() < MAX_SEARCH_RESULTS {
                results.push(SearchHit {
                    source: "timeline".to_owned(),
                    match_indices: parsed.highlight_indices(&text),
                    text,
                    target: SearchHitTarget::Timeline {
                        event: event.clone(),
                    },
                });
            }
        }
    }

    (results, total)
}

/// The pointer-view scroll offset (rows up from the live bottom) that
/// centers absolute `row` in a viewport of `view_rows` over a buffer of
/// `total_rows`. `0` means following live output.
pub(crate) fn scroll_offset_for_row(total_rows: usize, view_rows: usize, row: usize) -> usize {
    if view_rows == 0 {
        return 0;
    }
    let max_top = total_rows.saturating_sub(view_rows);
    let desired_first = row.saturating_sub(view_rows / 2).min(max_top);
    max_top - desired_first
}

/// Build the overlay scene for the current view state.
pub(crate) fn search_overlay(view: &SearchViewState, size: SceneSize) -> SearchOverlay {
    let items: Vec<SearchEntry> = view
        .results
        .iter()
        .map(|hit| SearchEntry {
            source: hit.source.clone(),
            text: hit.text.clone(),
            match_indices: hit.match_indices.clone(),
            pane: match &hit.target {
                SearchHitTarget::PaneRow { pane_id, .. } => Some(pane_id.clone()),
                SearchHitTarget::Timeline { .. } => None,
            },
        })
        .collect();
    let selected = if items.is_empty() {
        None
    } else {
        Some(view.selected.min(items.len() - 1))
    };

    let area = search_overlay_rect(size);
    let window = palette_item_window(pane_inner_rect(area), items.len(), selected);
    let overflow = view.total_hits.saturating_sub(items.len());
    let mut footer = String::new();
    let hidden_above = window.start;
    let hidden_below = items.len().saturating_sub(window.end);
    if hidden_above > 0 || hidden_below > 0 {
        footer.push_str(&format!("↑ {hidden_above} / ↓ {hidden_below} more · "));
    }
    if overflow > 0 {
        footer.push_str(&format!("+{overflow} beyond cap (narrow the query) · "));
    }
    footer.push_str("type to search (pane:/kind:) · enter jump · esc close");

    SearchOverlay {
        area,
        query: view.query.clone(),
        items,
        selected,
        overflow,
        footer,
    }
}

#[cfg(test)]
mod tests {
    use mandatum_terminal_vt::{TerminalParser, TerminalSize};

    use super::*;
    use crate::timeline::TimelineEventKind;

    fn source(pane: &str, title: &str, kind: SearchSourceKind, lines: &[&str]) -> SearchSource {
        SearchSource {
            pane_id: PaneId::new(pane),
            title: title.to_owned(),
            kind,
            lines: lines
                .iter()
                .enumerate()
                .map(|(row, text)| SearchLine {
                    row,
                    text: (*text).to_owned(),
                })
                .collect(),
        }
    }

    fn timeline_event(text: &str) -> TimelineEvent {
        TimelineEvent {
            at_ms: 1_000,
            kind: TimelineEventKind::TaskStarted {
                pane: "pane-9".to_owned(),
                command: text.to_owned(),
            },
        }
    }

    fn corpus() -> SearchCorpus {
        SearchCorpus {
            sources: vec![
                source(
                    "pane-1",
                    "shell",
                    SearchSourceKind::Terminal,
                    &["alpha build ok", "beta fail", "gamma build ok"],
                ),
                source(
                    "pane-2",
                    "tests",
                    SearchSourceKind::Task,
                    &["running build", "1 test failed"],
                ),
                source(
                    "pane-3",
                    "agent",
                    SearchSourceKind::Agent,
                    &["planning build fix"],
                ),
            ],
            timeline: vec![timeline_event("cargo build")],
        }
    }

    #[test]
    fn hits_group_by_source_most_recent_first_with_timeline_last() {
        let (hits, total) = search(&corpus(), "build");
        let listing: Vec<(&str, &str)> = hits
            .iter()
            .map(|hit| (hit.source.as_str(), hit.text.as_str()))
            .collect();
        assert_eq!(
            listing,
            vec![
                // pane-1's newest matching row first, then its older one.
                ("shell · pane-1 (terminal)", "gamma build ok"),
                ("shell · pane-1 (terminal)", "alpha build ok"),
                ("tests · pane-2 (task)", "running build"),
                ("agent · pane-3 (agent)", "planning build fix"),
                ("timeline", "task pane-9 started: cargo build"),
            ]
        );
        assert_eq!(total, 5);
        // Pane hits carry their jump row; timeline hits carry the event.
        assert_eq!(
            hits[0].target,
            SearchHitTarget::PaneRow {
                pane_id: PaneId::new("pane-1"),
                row: 2,
                kind: SearchSourceKind::Terminal,
            }
        );
        assert!(matches!(hits[4].target, SearchHitTarget::Timeline { .. }));
        // Matched chars are highlighted (contiguous "build" run).
        assert_eq!(hits[0].match_indices, vec![6, 7, 8, 9, 10]);
    }

    #[test]
    fn filters_parse_and_combine_with_and() {
        let corpus = corpus();
        // kind: narrows the family (prefix match, like the timeline filter).
        let (hits, _) = search(&corpus, "kind:task build");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "running build");
        // kind:timeline reaches timeline entries only.
        let (hits, _) = search(&corpus, "kind:timeline build");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "timeline");
        // pane: matches the title substring; timeline has no pane.
        let (hits, _) = search(&corpus, "pane:shell build");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.source.contains("pane-1")));
        // pane: also accepts the pane id, and tokens AND together.
        let (hits, _) = search(&corpus, "pane:pane-2 kind:task fail");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "1 test failed");
        // A filter-only query lists the matching sources' recent lines.
        let (hits, _) = search(&corpus, "kind:agent");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "planning build fix");
        // An unknown kind matches nothing rather than everything.
        let (hits, total) = search(&corpus, "kind:banana build");
        assert!(hits.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn empty_query_matches_nothing_calmly() {
        let (hits, total) = search(&corpus(), "");
        assert!(hits.is_empty());
        assert_eq!(total, 0);
        let (hits, _) = search(&corpus(), "   ");
        assert!(hits.is_empty());
    }

    #[test]
    fn results_cap_with_honest_overflow_count() {
        let lines: Vec<String> = (0..250)
            .map(|index| format!("match line {index}"))
            .collect();
        let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let corpus = SearchCorpus {
            sources: vec![source(
                "pane-1",
                "shell",
                SearchSourceKind::Terminal,
                &line_refs,
            )],
            timeline: Vec::new(),
        };
        let (hits, total) = search(&corpus, "match");
        assert_eq!(hits.len(), MAX_SEARCH_RESULTS);
        assert_eq!(total, 250);
        // Most recent first: the cap keeps the newest rows.
        assert_eq!(hits[0].text, "match line 249");

        let view = SearchViewState {
            query: "match".to_owned(),
            selected: 0,
            corpus,
            results: hits,
            total_hits: total,
        };
        let overlay = search_overlay(&view, SceneSize::new(100, 30));
        assert_eq!(overlay.items.len(), MAX_SEARCH_RESULTS);
        assert_eq!(overlay.overflow, 50);
        assert!(
            overlay.footer.contains("+50 beyond cap"),
            "{}",
            overlay.footer
        );
        assert!(overlay.footer.contains("esc close"));
    }

    #[test]
    fn zero_hit_state_is_calm() {
        let mut view = SearchViewState::new(corpus());
        view.query = "zzzzzz".to_owned();
        view.refresh();
        assert!(view.results.is_empty());
        assert_eq!(view.total_hits, 0);
        let overlay = search_overlay(&view, SceneSize::new(100, 30));
        assert!(overlay.items.is_empty());
        assert_eq!(overlay.selected, None);
        assert_eq!(overlay.overflow, 0);
        assert!(overlay.footer.contains("enter jump · esc close"));
    }

    #[test]
    fn grid_snapshot_covers_scrollback_and_screen_and_skips_blank_rows() {
        // 8x2 grid: four lines push two rows into scrollback.
        let mut parser = TerminalParser::new(TerminalSize::new(8, 2).unwrap());
        parser
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour")
            .unwrap();
        let grid = parser.grid();
        assert!(grid.scrollback_len() >= 2);

        let source = SearchSource::from_grid(
            PaneId::new("pane-1"),
            "shell",
            SearchSourceKind::Terminal,
            grid,
        );
        let texts: Vec<&str> = source.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, vec!["one", "two", "three", "four"]);
        // Rows are absolute scrollback+screen coordinates.
        assert_eq!(source.lines[0].row, 0);
        assert_eq!(source.lines[3].row, 3);
    }

    #[test]
    fn scroll_offset_centers_the_row_and_clamps_at_the_edges() {
        // Buffer of 100 rows, 20-row viewport: max_top = 80.
        assert_eq!(scroll_offset_for_row(100, 20, 50), 40); // centered
        assert_eq!(scroll_offset_for_row(100, 20, 0), 80); // clamped to top
        assert_eq!(scroll_offset_for_row(100, 20, 99), 0); // bottom follows live
        assert_eq!(scroll_offset_for_row(100, 20, 95), 0); // near-bottom too
        // Buffer smaller than the viewport never scrolls.
        assert_eq!(scroll_offset_for_row(10, 20, 5), 0);
        assert_eq!(scroll_offset_for_row(100, 0, 5), 0);
    }

    #[test]
    fn subsequence_precheck_agrees_with_the_fuzzy_matcher() {
        for (term, line) in [
            ("build", "gamma build ok"),
            ("bdok", "gamma build ok"),
            ("BUILD", "gamma build ok"),
            ("xyz", "gamma build ok"),
            ("okg", "gamma build ok"),
        ] {
            assert_eq!(
                subsequence_matches(term, line),
                fuzzy_match(term, line).is_some(),
                "precheck and matcher disagree on {term:?} in {line:?}"
            );
        }
    }
}
