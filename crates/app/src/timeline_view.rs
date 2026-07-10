//! The timeline overlay model: live view state over the durable log
//! (`crate::timeline`), the text/structured filter, and the scene overlay
//! build. Runtime presentation only; never serialized.

use mandatum_scene::{
    PaneId, SceneSize, TimelineEntry, TimelineOverlay,
    layout::{palette_item_window, pane_inner_rect, timeline_overlay_rect},
};

use crate::timeline::{TimelineEvent, TimelineTail, timeline_glyph_meaning};

/// Live overlay state while the timeline is open.
pub(crate) struct TimelineViewState {
    pub(crate) query: String,
    pub(crate) selected: usize,
    /// Newest first (reverse-chronological display order).
    pub(crate) events: Vec<TimelineEvent>,
    pub(crate) malformed: usize,
    pub(crate) error: Option<String>,
    /// Indices into `events` matching the live query, newest first. Cached:
    /// filtering (multi-token fuzzy matching over the whole tail) is too
    /// expensive to recompute every frame, so it recomputes only when the
    /// query changes ([`Self::refilter`]). `since:` windows therefore anchor
    /// to the moment the query was last edited, which is when the user
    /// asked the question.
    filtered: Vec<usize>,
}

impl TimelineViewState {
    pub(crate) fn from_tail(tail: TimelineTail) -> Self {
        let mut events = tail.events;
        events.reverse();
        let mut view = Self {
            query: String::new(),
            selected: 0,
            events,
            malformed: tail.malformed,
            error: tail.error,
            filtered: Vec::new(),
        };
        view.refilter(crate::timeline::now_ms());
        view
    }

    /// The cached filter result, newest first.
    pub(crate) fn filtered(&self) -> &[usize] {
        &self.filtered
    }

    /// Append text to the query and recompute the filter once.
    pub(crate) fn push_query(&mut self, text: &str, now_ms: u64) {
        self.query.push_str(text);
        self.selected = 0;
        self.refilter(now_ms);
    }

    /// Delete the last query character and recompute the filter once.
    pub(crate) fn pop_query(&mut self, now_ms: u64) {
        self.query.pop();
        self.selected = 0;
        self.refilter(now_ms);
    }

    /// Recompute the cached filter for the current query.
    pub(crate) fn refilter(&mut self, now_ms: u64) {
        self.filtered = filter_indices(&self.events, &self.query, now_ms);
    }
}

/// Apply the timeline filter: plain tokens fuzzy-match the description,
/// `pane:` / `kind:` / `since:` prefixes filter structurally.
fn filter_indices(events: &[TimelineEvent], query: &str, now_ms: u64) -> Vec<usize> {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            tokens.iter().all(|token| {
                if let Some(pane) = token.strip_prefix("pane:") {
                    return event
                        .kind
                        .pane()
                        .is_some_and(|candidate| candidate.contains(pane));
                }
                if let Some(kind) = token.strip_prefix("kind:") {
                    return event.kind.kind_label().starts_with(kind);
                }
                if let Some(since) = token.strip_prefix("since:") {
                    return match parse_duration_ms(since) {
                        Some(window_ms) => event.at_ms >= now_ms.saturating_sub(window_ms),
                        // An unparsable window matches nothing rather than
                        // silently matching everything.
                        None => false,
                    };
                }
                mandatum_commands::fuzzy::fuzzy_match(token, &event.kind.describe()).is_some()
            })
        })
        .map(|(index, _)| index)
        .collect()
}

/// Parse "30s" / "5m" / "2h" / "1d" into milliseconds.
fn parse_duration_ms(text: &str) -> Option<u64> {
    let (digits, unit) = text.split_at(text.len().checked_sub(1)?);
    let value: u64 = digits.parse().ok()?;
    let unit_ms = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    value.checked_mul(unit_ms)
}

/// Build the overlay scene for the current view state.
pub(crate) fn timeline_overlay(
    view: &TimelineViewState,
    size: SceneSize,
    now_ms: u64,
) -> TimelineOverlay {
    let items: Vec<TimelineEntry> = view
        .filtered()
        .iter()
        .map(|&index| {
            let event = &view.events[index];
            TimelineEntry {
                glyph: event.kind.glyph().to_owned(),
                when: format_relative(now_ms, event.at_ms),
                text: event.kind.describe(),
                pane: event.kind.pane().map(PaneId::new),
            }
        })
        .collect();
    let selected = if items.is_empty() {
        None
    } else {
        Some(view.selected.min(items.len() - 1))
    };

    let area = timeline_overlay_rect(size);
    let window = palette_item_window(pane_inner_rect(area), items.len(), selected);
    let mut footer = String::new();
    let hidden_above = window.start;
    let hidden_below = items.len().saturating_sub(window.end);
    if hidden_above > 0 || hidden_below > 0 {
        footer.push_str(&format!("↑ {hidden_above} / ↓ {hidden_below} more · "));
    }
    footer.push_str("type to filter (pane:/kind:/since:) · enter jump · esc close");
    if view.malformed > 0 {
        footer.push_str(&format!(" · {} malformed line(s) skipped", view.malformed));
    }
    if let Some(error) = &view.error {
        footer.push_str(&format!(" · {error}"));
    }
    if let Some(legend) = glyph_legend(&items) {
        footer.push_str(&format!(" · {legend}"));
    }

    TimelineOverlay {
        area,
        query: view.query.clone(),
        items,
        selected,
        skipped_malformed: view.malformed,
        footer,
    }
}

/// The footer legend for the glyphs actually on screen, in first-seen order
/// ("» command · ✗ failed"). Generated from the same table `glyph()` is
/// tested against, so a drawn glyph can never lack a meaning.
fn glyph_legend(items: &[TimelineEntry]) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    for item in items {
        if !seen.contains(&item.glyph.as_str()) {
            seen.push(item.glyph.as_str());
        }
    }
    let parts: Vec<String> = seen
        .iter()
        .filter_map(|glyph| {
            timeline_glyph_meaning(glyph).map(|meaning| format!("{glyph} {meaning}"))
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// "just now", "5s ago", "42s ago", "5m ago", "3h ago", "2d ago". Seconds
/// granularity from the first second, so a burst of recent events reads as
/// a sequence instead of a wall of "just now". Future timestamps (clock
/// skew) read as "just now".
pub(crate) fn format_relative(now_ms: u64, at_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(at_ms) / 1_000;
    match seconds {
        0 => "just now".to_owned(),
        1..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::TimelineEventKind;

    fn task_exit(pane: &str, exit: &str) -> TimelineEventKind {
        TimelineEventKind::TaskExited {
            pane: pane.to_owned(),
            command: "cargo test".to_owned(),
            exit: exit.to_owned(),
        }
    }

    /// Build a view over newest-first events with the filter cache primed,
    /// the way `from_tail` would.
    fn view_over(events: Vec<TimelineEvent>, malformed: usize, now: u64) -> TimelineViewState {
        let mut view = TimelineViewState {
            query: String::new(),
            selected: 0,
            events,
            malformed,
            error: None,
            filtered: Vec::new(),
        };
        view.refilter(now);
        view
    }

    #[test]
    fn relative_timestamps_read_calmly() {
        let now = 1_000_000_000_000;
        assert_eq!(format_relative(now, now), "just now");
        assert_eq!(format_relative(now, now - 5_000), "5s ago");
        assert_eq!(format_relative(now, now - 9_000), "9s ago");
        assert_eq!(format_relative(now, now - 42_000), "42s ago");
        assert_eq!(format_relative(now, now - 5 * 60_000), "5m ago");
        assert_eq!(format_relative(now, now - 3 * 3_600_000), "3h ago");
        assert_eq!(format_relative(now, now - 2 * 86_400_000), "2d ago");
        // Clock skew never yields negative ages.
        assert_eq!(format_relative(now, now + 60_000), "just now");
    }

    #[test]
    fn filter_matches_text_and_structured_prefixes() {
        let now = 1_000_000_000_000;
        let events = vec![
            TimelineEvent {
                at_ms: now - 10_000,
                kind: task_exit("pane-2", "failed: exit 3"),
            },
            TimelineEvent {
                at_ms: now - 400_000,
                kind: TimelineEventKind::AgentStatus {
                    pane: "pane-4".to_owned(),
                    status: "running".to_owned(),
                },
            },
            TimelineEvent {
                at_ms: now - 500_000,
                kind: TimelineEventKind::WorkspaceSaved {
                    path: "/tmp/w.json".to_owned(),
                },
            },
        ];

        assert_eq!(filter_indices(&events, "", now), vec![0, 1, 2]);
        assert_eq!(filter_indices(&events, "failed", now), vec![0]);
        assert_eq!(filter_indices(&events, "pane:pane-4", now), vec![1]);
        assert_eq!(filter_indices(&events, "kind:task", now), vec![0]);
        assert_eq!(filter_indices(&events, "kind:workspace", now), vec![2]);
        assert_eq!(filter_indices(&events, "since:1m", now), vec![0]);
        assert_eq!(filter_indices(&events, "since:10m", now), vec![0, 1, 2]);
        // Tokens combine with AND.
        assert_eq!(
            filter_indices(&events, "kind:task pane:pane-2 failed", now),
            vec![0]
        );
        assert_eq!(
            filter_indices(&events, "since:banana", now),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn overlay_lists_newest_first_with_glyphs_and_relative_times() {
        let now = 1_000_000_000_000;
        let view = view_over(
            vec![
                TimelineEvent {
                    at_ms: now - 5_000,
                    kind: task_exit("pane-2", "failed: exit 3"),
                },
                TimelineEvent {
                    at_ms: now - 120_000,
                    kind: TimelineEventKind::TaskStarted {
                        pane: "pane-2".to_owned(),
                        command: "cargo test".to_owned(),
                    },
                },
            ],
            1,
            now,
        );

        let overlay = timeline_overlay(&view, SceneSize::new(100, 30), now);
        assert_eq!(overlay.items.len(), 2);
        assert_eq!(overlay.items[0].glyph, "✗");
        assert_eq!(overlay.items[0].when, "5s ago");
        assert!(overlay.items[0].text.contains("failed: exit 3"));
        assert_eq!(overlay.items[0].pane, Some(PaneId::new("pane-2")));
        assert_eq!(overlay.items[1].when, "2m ago");
        assert_eq!(overlay.selected, Some(0));
        assert_eq!(overlay.skipped_malformed, 1);
        assert!(overlay.footer.contains("1 malformed line(s) skipped"));
        assert!(overlay.footer.contains("esc close"));
    }

    #[test]
    fn footer_legend_names_every_glyph_on_screen() {
        let now = 1_000_000_000_000;
        let mut view = TimelineViewState {
            query: String::new(),
            selected: 0,
            events: vec![
                TimelineEvent {
                    at_ms: now - 5_000,
                    kind: task_exit("pane-2", "failed: exit 3"),
                },
                TimelineEvent {
                    at_ms: now - 60_000,
                    kind: TimelineEventKind::ApprovalRequested {
                        pane: "pane-3".to_owned(),
                        command: "rm -rf target".to_owned(),
                        scope: "/tmp".to_owned(),
                        risk: "high".to_owned(),
                    },
                },
                TimelineEvent {
                    at_ms: now - 120_000,
                    kind: TimelineEventKind::CommandDispatched {
                        command: "split-right".to_owned(),
                        pane: None,
                    },
                },
            ],
            malformed: 0,
            error: None,
            filtered: Vec::new(),
        };
        view.refilter(now);

        let overlay = timeline_overlay(&view, SceneSize::new(120, 30), now);
        // Every glyph drawn in the rows appears in the footer legend with
        // its meaning, so a stranger can decode the column.
        for item in &overlay.items {
            let meaning = crate::timeline::timeline_glyph_meaning(&item.glyph)
                .expect("drawn glyph must be in the legend table");
            assert!(
                overlay
                    .footer
                    .contains(&format!("{} {meaning}", item.glyph)),
                "footer {:?} must explain glyph {:?}",
                overlay.footer,
                item.glyph
            );
        }
        assert!(overlay.footer.contains("✗ failed"));
        assert!(overlay.footer.contains("? approval"));
        assert!(overlay.footer.contains("» command"));
    }

    // The filtered view is a cache: building the overlay never recomputes
    // it (a hostile multi-token query must not tax every frame), and query
    // edits are exactly what invalidates it.
    #[test]
    fn filter_cache_recomputes_on_query_edits_not_on_overlay_builds() {
        let now = 1_000_000_000_000;
        let mut view = view_over(
            vec![
                TimelineEvent {
                    at_ms: now - 5_000,
                    kind: task_exit("pane-2", "failed: exit 3"),
                },
                TimelineEvent {
                    at_ms: now - 10_000,
                    kind: task_exit("pane-3", "succeeded: exit 0"),
                },
            ],
            0,
            now,
        );
        assert_eq!(view.filtered(), &[0, 1]);

        // Mutate the event set behind the cache's back: overlay builds keep
        // serving the cached view (proof no per-frame recompute happens).
        view.events.push(TimelineEvent {
            at_ms: now - 1_000,
            kind: task_exit("pane-9", "failed: exit 9"),
        });
        let overlay = timeline_overlay(&view, SceneSize::new(100, 30), now);
        assert_eq!(overlay.items.len(), 2, "cache served without recompute");
        let overlay = timeline_overlay(&view, SceneSize::new(100, 30), now);
        assert_eq!(overlay.items.len(), 2);

        // A query edit recomputes once, over the current event set.
        view.push_query("failed", now);
        assert_eq!(view.filtered(), &[0, 2]);
        let overlay = timeline_overlay(&view, SceneSize::new(100, 30), now);
        assert_eq!(overlay.items.len(), 2);
        assert!(
            overlay
                .items
                .iter()
                .all(|item| item.text.contains("failed"))
        );

        // Deleting recomputes too, widening back out.
        for _ in 0.."failed".len() {
            view.pop_query(now);
        }
        assert_eq!(view.filtered(), &[0, 1, 2]);
    }
}
