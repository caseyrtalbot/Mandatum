//! The command palette model: fuzzy filtering, context-aware ranking,
//! availability gating, and live key hints.
//!
//! # Interaction contract
//!
//! Ctrl+P opens the palette with an empty filter input. From there:
//!
//! - **Typing filters** every built-in command by case-insensitive fuzzy
//!   subsequence match ([`mandatum_commands::fuzzy`]); best match first, and
//!   commands relevant to the focused pane kind rank ahead of ties.
//! - **Single-letter fast paths are preserved on the first keystroke**:
//!   while the input is empty, a bare key resolves through
//!   `resolve_palette_key` exactly as the pre-fuzzy palette did — bound
//!   keys dispatch their command (including the task-pane and float/dock
//!   substitutions), `q` quits (the Quit command's binding), Tab/BackTab
//!   dispatch focus-next/previous. A key with no binding starts the filter
//!   instead, and Shift+letter always starts the filter, so every command
//!   is reachable by typing even when its first letter is a fast path. The
//!   empty input's placeholder states this rule and the Shift escape.
//! - **Navigation**: Up/Down or Ctrl+N/Ctrl+P move the selection (while the
//!   palette is open Ctrl+P navigates; Esc closes). Tab/BackTab also move
//!   the selection once the filter is non-empty, and the wheel scrolls it.
//!   The footer counts entries hidden above/below the visible window.
//! - **Enter** runs the selected entry; on a greyed entry it reports the
//!   reason instead. **Esc** closes.
//!
//! Commands that are currently impossible stay visible but greyed, with the
//! reason in the detail text: discoverability over minimalism.

use mandatum_commands::{BUILT_IN_COMMANDS, CommandCategory, CommandId, fuzzy::fuzzy_match};
use mandatum_scene::{PaletteEntry, PaneSceneKind};

use crate::keymap::{Keymap, format_chord};

/// Ranking nudge for commands whose category matches the focused pane kind.
/// Sized so context dominates an empty filter and breaks near-ties under a
/// query without overriding a clearly better fuzzy match.
const CONTEXT_BONUS: i32 = 10;

/// Live palette UI state: the typed filter and the selected row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaletteState {
    pub(crate) query: String,
    pub(crate) selected: usize,
}

/// A cheap snapshot of the workspace facts the palette ranks and gates on,
/// captured from `AppState` so the ranking logic stays unit-testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaletteWorkspaceView {
    pub(crate) focused_kind: PaneSceneKind,
    /// "title (pane-id)" of the focused pane, for selection previews.
    pub(crate) focused_pane_label: String,
    pub(crate) focused_agent_session_live: bool,
    pub(crate) focused_agent_pending_approval: bool,
    pub(crate) agent_connector_configured: bool,
    pub(crate) agent_panes_exist: bool,
    pub(crate) any_agent_waiting: bool,
    pub(crate) focused_task_running: bool,
    pub(crate) focused_task_failed: bool,
    pub(crate) focused_has_live_terminal: bool,
    pub(crate) focused_is_floating: bool,
    /// Whether a durable timeline log exists for this workspace.
    pub(crate) timeline_available: bool,
    /// Whether the focused pane sits inside a tiled split (so Grow/Shrink
    /// have a boundary to move).
    pub(crate) focused_in_tiled_split: bool,
    pub(crate) pane_count: usize,
}

/// One resolved palette row: the scene entry plus what executing it means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaletteRow {
    pub(crate) command_id: CommandId,
    pub(crate) enabled: bool,
    pub(crate) entry: PaletteEntry,
}

/// Resolve the palette rows for a filter query: fuzzy-filtered, ranked by
/// score plus context bonus (stable built-in order on ties), with the
/// selected row previewing the focused pane it will affect.
pub(crate) fn palette_rows(
    query: &str,
    selected: usize,
    view: &PaletteWorkspaceView,
    keymap: &Keymap,
) -> Vec<PaletteRow> {
    let needle = query.trim();
    let mut scored = Vec::new();
    for command in BUILT_IN_COMMANDS {
        let Some(hit) = fuzzy_match(needle, command.label) else {
            continue;
        };
        let rank = hit.score + context_bonus(command.category, view.focused_kind);
        let availability = availability(command.id, view);
        let enabled = availability.is_ok();
        let detail = match availability {
            Ok(()) => enabled_detail(command.id, command.category),
            Err(reason) => reason,
        };
        scored.push((
            rank,
            PaletteRow {
                command_id: command.id,
                enabled,
                entry: PaletteEntry {
                    label: command.label.to_owned(),
                    detail,
                    key_hint: key_hint(command.id, keymap),
                    match_indices: hit.indices,
                    enabled,
                },
            },
        ));
    }
    // Stable sort: equal ranks keep the built-in table order.
    scored.sort_by_key(|(rank, _)| core::cmp::Reverse(*rank));
    let mut rows: Vec<PaletteRow> = scored.into_iter().map(|(_, row)| row).collect();

    let selected = selected.min(rows.len().saturating_sub(1));
    if let Some(row) = rows.get_mut(selected)
        && row.enabled
        && previews_focused_pane(row.command_id, view.focused_kind)
    {
        row.entry.detail = format!("{} — {}", row.entry.detail, view.focused_pane_label);
    }
    rows
}

/// The palette footer: the overlay always names its own keys, and marks
/// entries hidden above/below the visible window so the list never looks
/// complete when it is not. (The empty-query fast-path caveat lives in the
/// input placeholder, where it always fits.)
pub(crate) fn palette_footer(hidden_above: usize, hidden_below: usize) -> String {
    let base = "↑/↓ or ctrl+n/p move · enter run · esc close";
    match (hidden_above, hidden_below) {
        (0, 0) => base.to_owned(),
        (0, below) => format!("↓ {below} more · {base}"),
        (above, 0) => format!("↑ {above} more · {base}"),
        (above, below) => format!("↑ {above} / ↓ {below} more · {base}"),
    }
}

/// Why a command is impossible right now, or `Ok` when it can run. Reasons
/// surface in the greyed entry's detail text, and the single-letter fast
/// paths consult the same gate so a bare key can never fire-and-fail where
/// the listed row would be greyed (`app_state::handle_palette_key`).
pub(crate) fn availability(
    command_id: CommandId,
    view: &PaletteWorkspaceView,
) -> Result<(), String> {
    let on_agent = view.focused_kind == PaneSceneKind::Agent;
    let on_task = view.focused_kind == PaneSceneKind::Task;
    match command_id {
        CommandId::ApproveAgentAction | CommandId::RejectAgentAction => {
            if !on_agent {
                Err("focused pane is not an agent pane".to_owned())
            } else if !view.focused_agent_pending_approval {
                Err("no approval is pending in this pane".to_owned())
            } else {
                Ok(())
            }
        }
        CommandId::StopAgent => {
            if !on_agent {
                Err("focused pane is not an agent pane".to_owned())
            } else if !view.focused_agent_session_live {
                Err("no agent session is running in this pane".to_owned())
            } else {
                Ok(())
            }
        }
        CommandId::StartAgent => {
            if !view.agent_connector_configured {
                Err("no agent connector is configured".to_owned())
            } else if !on_agent && view.agent_panes_exist {
                Err("focus an agent pane first".to_owned())
            } else {
                Ok(())
            }
        }
        CommandId::FocusNextWaitingAgent => {
            if view.any_agent_waiting {
                Ok(())
            } else {
                Err("no agent is waiting for approval".to_owned())
            }
        }
        CommandId::RerunTask => {
            if on_task {
                Ok(())
            } else {
                Err("focused pane is not a task pane".to_owned())
            }
        }
        CommandId::StopTask => {
            if !on_task {
                Err("focused pane is not a task pane".to_owned())
            } else if !view.focused_task_running {
                Err("task is not running".to_owned())
            } else {
                Ok(())
            }
        }
        CommandId::InvestigateTaskFailure => {
            if !on_task {
                Err("focused pane is not a task pane".to_owned())
            } else if !view.focused_task_failed {
                Err("focused task has no known failure".to_owned())
            } else if !view.agent_connector_configured {
                Err("no agent connector is configured".to_owned())
            } else {
                Ok(())
            }
        }
        CommandId::RestartPane if on_task => {
            Err("task panes rerun instead (use Rerun task)".to_owned())
        }
        // Float/dock is a labeled toggle: exactly one of the pair is
        // runnable, and running the other reports why instead of no-oping.
        CommandId::FloatPane if view.focused_is_floating => {
            Err("pane is already floating (use Dock pane)".to_owned())
        }
        CommandId::DockPane if !view.focused_is_floating => {
            Err("focused pane is not floating".to_owned())
        }
        // The keyboard float-move quartet needs a floating pane under focus.
        CommandId::MoveFloatLeft
        | CommandId::MoveFloatRight
        | CommandId::MoveFloatUp
        | CommandId::MoveFloatDown
            if !view.focused_is_floating =>
        {
            Err("focused pane is not floating (Float pane first)".to_owned())
        }
        CommandId::GrowPane | CommandId::ShrinkPane => {
            if view.focused_is_floating {
                Err("floating panes move by dragging their title; dock to resize".to_owned())
            } else if !view.focused_in_tiled_split {
                Err("no split to resize: split the pane first".to_owned())
            } else {
                Ok(())
            }
        }
        CommandId::EnterCopyMode => {
            if view.focused_has_live_terminal {
                Ok(())
            } else {
                Err("focused pane has no live terminal".to_owned())
            }
        }
        CommandId::SetAgentObjective => {
            if on_agent {
                Ok(())
            } else {
                Err("focused pane is not an agent pane".to_owned())
            }
        }
        CommandId::ShowTimeline => {
            if view.timeline_available {
                Ok(())
            } else {
                Err("no workspace directory to keep a timeline in".to_owned())
            }
        }
        CommandId::ClosePane if view.pane_count <= 1 => {
            Err("cannot close the last pane".to_owned())
        }
        _ => Ok(()),
    }
}

fn context_bonus(category: CommandCategory, focused: PaneSceneKind) -> i32 {
    let relevant = matches!(
        (focused, category),
        (PaneSceneKind::Agent, CommandCategory::Agent)
            | (PaneSceneKind::Task, CommandCategory::Task)
            | (PaneSceneKind::Terminal, CommandCategory::Pane)
    );
    if relevant { CONTEXT_BONUS } else { 0 }
}

/// Detail for a runnable entry: its category, plus the direct-key note for
/// the approval commands (they also work as bare y/n on a waiting pane).
fn enabled_detail(command_id: CommandId, category: CommandCategory) -> String {
    match command_id {
        CommandId::ApproveAgentAction => {
            "agent (direct key: y while the focused pane awaits approval)".to_owned()
        }
        CommandId::RejectAgentAction => {
            "agent (direct key: n while the focused pane awaits approval)".to_owned()
        }
        _ => category_label(category).to_owned(),
    }
}

/// The entry's current keys from the live keymap: palette letter first,
/// then the global chord, joined when both exist.
fn key_hint(command_id: CommandId, keymap: &Keymap) -> Option<String> {
    let mut hints = Vec::new();
    // Commands that ride another command's letter through a context
    // substitution show that letter: Dock rides Float's (one toggle key for
    // the pair), Rerun task rides Restart pane's (the task-pane
    // substitution).
    let letter_owner = match command_id {
        CommandId::DockPane => CommandId::FloatPane,
        CommandId::RerunTask => CommandId::RestartPane,
        other => other,
    };
    if let Some(letter) = keymap.palette.key_for(letter_owner) {
        hints.push(letter.to_string());
    }
    if let Some(chord) = keymap.chord_for(command_id) {
        hints.push(format_chord(chord));
    }
    if hints.is_empty() {
        None
    } else {
        Some(hints.join(" · "))
    }
}

/// Whether a command acts on the focused pane, so the selected entry can
/// preview it. Kind-gated so, e.g., "Start agent" never previews a shell
/// pane it would not actually target.
fn previews_focused_pane(command_id: CommandId, focused: PaneSceneKind) -> bool {
    match command_id {
        CommandId::ClosePane
        | CommandId::RestartPane
        | CommandId::ZoomPane
        | CommandId::FloatPane
        | CommandId::DockPane
        | CommandId::GrowPane
        | CommandId::ShrinkPane
        | CommandId::MoveFloatLeft
        | CommandId::MoveFloatRight
        | CommandId::MoveFloatUp
        | CommandId::MoveFloatDown
        | CommandId::StackPanes
        | CommandId::EnterCopyMode => true,
        CommandId::RerunTask | CommandId::StopTask | CommandId::InvestigateTaskFailure => {
            focused == PaneSceneKind::Task
        }
        CommandId::StartAgent
        | CommandId::StopAgent
        | CommandId::ApproveAgentAction
        | CommandId::RejectAgentAction => focused == PaneSceneKind::Agent,
        _ => false,
    }
}

pub(crate) fn category_label(category: CommandCategory) -> &'static str {
    match category {
        CommandCategory::Project => "project",
        CommandCategory::Pane => "pane",
        CommandCategory::Task => "task",
        CommandCategory::Agent => "agent",
        CommandCategory::Layout => "layout",
        CommandCategory::Persistence => "persistence",
        CommandCategory::Config => "config",
        CommandCategory::App => "app",
    }
}

#[cfg(test)]
mod tests {
    use mandatum_commands::command_for_id;

    use super::*;
    use crate::keymap::parse_chord;

    fn terminal_view() -> PaletteWorkspaceView {
        PaletteWorkspaceView {
            focused_kind: PaneSceneKind::Terminal,
            focused_pane_label: "shell (pane-1)".to_owned(),
            focused_agent_session_live: false,
            focused_agent_pending_approval: false,
            agent_connector_configured: true,
            agent_panes_exist: false,
            any_agent_waiting: false,
            focused_task_running: false,
            focused_task_failed: false,
            focused_has_live_terminal: true,
            focused_is_floating: false,
            timeline_available: true,
            focused_in_tiled_split: true,
            pane_count: 2,
        }
    }

    fn agent_view() -> PaletteWorkspaceView {
        PaletteWorkspaceView {
            focused_kind: PaneSceneKind::Agent,
            focused_pane_label: "agent (pane-2)".to_owned(),
            focused_agent_session_live: false,
            focused_agent_pending_approval: false,
            agent_panes_exist: true,
            ..terminal_view()
        }
    }

    #[test]
    fn investigate_task_failure_is_discoverable_and_gated_on_a_known_failure() {
        let keymap = Keymap::default();
        let task_view = PaletteWorkspaceView {
            focused_kind: PaneSceneKind::Task,
            focused_pane_label: "checks (pane-2)".to_owned(),
            ..terminal_view()
        };

        let rows = palette_rows("investigate", 0, &task_view, &keymap);
        let investigate = row(&rows, CommandId::InvestigateTaskFailure);
        assert!(!investigate.enabled);
        assert_eq!(
            investigate.entry.detail,
            "focused task has no known failure"
        );

        let failed = PaletteWorkspaceView {
            focused_task_failed: true,
            ..task_view
        };
        let rows = palette_rows("investigate", 0, &failed, &keymap);
        assert!(row(&rows, CommandId::InvestigateTaskFailure).enabled);
    }

    fn row(rows: &[PaletteRow], command_id: CommandId) -> &PaletteRow {
        rows.iter()
            .find(|row| row.command_id == command_id)
            .unwrap_or_else(|| panic!("{command_id:?} must be listed"))
    }

    #[test]
    fn empty_query_lists_every_command_with_context_ranked_first() {
        let keymap = Keymap::default();

        let rows = palette_rows("", 0, &agent_view(), &keymap);
        assert_eq!(rows.len(), BUILT_IN_COMMANDS.len());
        // Agent commands lead on an agent pane, in built-in order.
        let leading: Vec<CommandId> = rows.iter().take(6).map(|row| row.command_id).collect();
        assert_eq!(
            leading,
            vec![
                CommandId::NewAgentPane,
                CommandId::StartAgent,
                CommandId::StopAgent,
                CommandId::ApproveAgentAction,
                CommandId::RejectAgentAction,
                CommandId::FocusNextWaitingAgent,
            ]
        );

        // On a terminal pane, pane commands lead instead.
        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        assert_eq!(rows[0].command_id, CommandId::NewTerminal);
        assert!(
            rows.iter().take(6).all(
                |row| command_for_id(row.command_id).unwrap().category == CommandCategory::Pane
            )
        );
    }

    #[test]
    fn query_filters_by_fuzzy_match_and_keeps_best_match_on_top() {
        let keymap = Keymap::default();
        let rows = palette_rows("approve", 0, &terminal_view(), &keymap);

        assert_eq!(rows[0].command_id, CommandId::ApproveAgentAction);
        assert_eq!(rows[0].entry.match_indices, vec![0, 1, 2, 3, 4, 5, 6]);
        assert!(
            rows.iter()
                .all(|row| row.command_id != CommandId::SplitRight)
        );
    }

    #[test]
    fn impossible_commands_are_greyed_with_the_reason_not_hidden() {
        let keymap = Keymap::default();

        // Approve with no pending approval, on an agent pane.
        let rows = palette_rows("", 0, &agent_view(), &keymap);
        let approve = row(&rows, CommandId::ApproveAgentAction);
        assert!(!approve.enabled);
        assert!(!approve.entry.enabled);
        assert_eq!(approve.entry.detail, "no approval is pending in this pane");

        // Stop agent with nothing running.
        let stop = row(&rows, CommandId::StopAgent);
        assert!(!stop.enabled);
        assert_eq!(
            stop.entry.detail,
            "no agent session is running in this pane"
        );

        // Stop task with no running task, on a task pane.
        let task_view = PaletteWorkspaceView {
            focused_kind: PaneSceneKind::Task,
            focused_task_running: false,
            ..terminal_view()
        };
        let rows = palette_rows("", 0, &task_view, &keymap);
        assert_eq!(
            row(&rows, CommandId::StopTask).entry.detail,
            "task is not running"
        );
        assert!(!row(&rows, CommandId::StopTask).enabled);
        // Restart pane redirects to rerun on task panes.
        assert!(!row(&rows, CommandId::RestartPane).enabled);
        // Rerun is possible there.
        assert!(row(&rows, CommandId::RerunTask).enabled);

        // Everything greyed is still listed: nothing hidden.
        assert_eq!(rows.len(), BUILT_IN_COMMANDS.len());
    }

    #[test]
    fn approval_becomes_available_with_a_pending_request() {
        let keymap = Keymap::default();
        let view = PaletteWorkspaceView {
            focused_agent_session_live: true,
            focused_agent_pending_approval: true,
            ..agent_view()
        };
        let rows = palette_rows("", 0, &view, &keymap);
        assert!(row(&rows, CommandId::ApproveAgentAction).enabled);
        assert!(row(&rows, CommandId::RejectAgentAction).enabled);
        assert!(row(&rows, CommandId::StopAgent).enabled);
    }

    #[test]
    fn entries_show_their_current_keys_from_the_live_keymap() {
        let mut keymap = Keymap::default();
        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        assert_eq!(
            row(&rows, CommandId::SplitRight).entry.key_hint.as_deref(),
            Some("v")
        );
        assert_eq!(row(&rows, CommandId::NewSession).entry.key_hint, None);

        // A rebind and a global chord both show up.
        keymap.palette.rebind(CommandId::SplitRight, 'u');
        keymap.bind_chord(CommandId::SplitRight, parse_chord("ctrl+shift+r").unwrap());
        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        assert_eq!(
            row(&rows, CommandId::SplitRight).entry.key_hint.as_deref(),
            Some("u · ctrl+shift+r")
        );
    }

    #[test]
    fn selected_entry_previews_the_focused_pane_it_affects() {
        let keymap = Keymap::default();
        let rows = palette_rows("close pane", 0, &terminal_view(), &keymap);
        assert_eq!(rows[0].command_id, CommandId::ClosePane);
        assert_eq!(rows[0].entry.detail, "pane — shell (pane-1)");

        // Unselected rows carry no preview.
        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        let close = row(&rows, CommandId::ClosePane);
        assert_eq!(close.entry.detail, "pane");

        // Commands that do not act on the focused pane never preview it.
        let rows = palette_rows("save workspace", 0, &terminal_view(), &keymap);
        assert_eq!(rows[0].command_id, CommandId::SaveWorkspace);
        assert_eq!(rows[0].entry.detail, "persistence");
    }

    #[test]
    fn no_match_yields_an_empty_list() {
        let rows = palette_rows("zzzzzz", 0, &terminal_view(), &Keymap::default());
        assert!(rows.is_empty());
    }

    #[test]
    fn float_and_dock_gate_on_the_floating_state_and_share_the_toggle_key() {
        let keymap = Keymap::default();

        // Tiled pane: Float runs, Dock is greyed with the reason.
        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        assert!(row(&rows, CommandId::FloatPane).enabled);
        let dock = row(&rows, CommandId::DockPane);
        assert!(!dock.enabled);
        assert_eq!(dock.entry.detail, "focused pane is not floating");
        // Dock shows the shared float/dock toggle key.
        assert_eq!(dock.entry.key_hint.as_deref(), Some("f"));

        // Floating pane: the pair flips, and Float names Dock as the way on.
        let floating = PaletteWorkspaceView {
            focused_is_floating: true,
            focused_in_tiled_split: false,
            ..terminal_view()
        };
        let rows = palette_rows("", 0, &floating, &keymap);
        assert!(row(&rows, CommandId::DockPane).enabled);
        let float = row(&rows, CommandId::FloatPane);
        assert!(!float.enabled);
        assert_eq!(
            float.entry.detail,
            "pane is already floating (use Dock pane)"
        );
    }

    #[test]
    fn move_float_commands_gate_on_a_floating_focus() {
        let keymap = Keymap::default();

        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        let left = row(&rows, CommandId::MoveFloatLeft);
        assert!(!left.enabled);
        assert_eq!(
            left.entry.detail,
            "focused pane is not floating (Float pane first)"
        );

        let floating = PaletteWorkspaceView {
            focused_is_floating: true,
            focused_in_tiled_split: false,
            ..terminal_view()
        };
        let rows = palette_rows("", 0, &floating, &keymap);
        for command_id in [
            CommandId::MoveFloatLeft,
            CommandId::MoveFloatRight,
            CommandId::MoveFloatUp,
            CommandId::MoveFloatDown,
        ] {
            assert!(row(&rows, command_id).enabled);
        }
    }

    #[test]
    fn grow_and_shrink_gate_on_having_a_split_to_move() {
        let keymap = Keymap::default();

        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        assert!(row(&rows, CommandId::GrowPane).enabled);
        assert!(row(&rows, CommandId::ShrinkPane).enabled);

        let single = PaletteWorkspaceView {
            focused_in_tiled_split: false,
            ..terminal_view()
        };
        let rows = palette_rows("", 0, &single, &keymap);
        let grow = row(&rows, CommandId::GrowPane);
        assert!(!grow.enabled);
        assert_eq!(
            grow.entry.detail,
            "no split to resize: split the pane first"
        );

        let floating = PaletteWorkspaceView {
            focused_is_floating: true,
            ..terminal_view()
        };
        let rows = palette_rows("", 0, &floating, &keymap);
        assert!(!row(&rows, CommandId::ShrinkPane).enabled);
    }

    #[test]
    fn visibility_commands_gate_on_their_preconditions() {
        let keymap = Keymap::default();

        // Set agent objective needs a focused agent pane.
        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        let objective = row(&rows, CommandId::SetAgentObjective);
        assert!(!objective.enabled);
        assert_eq!(objective.entry.detail, "focused pane is not an agent pane");
        let rows = palette_rows("", 0, &agent_view(), &keymap);
        assert!(row(&rows, CommandId::SetAgentObjective).enabled);

        // The timeline needs a workspace directory; the session map always
        // works.
        let rows = palette_rows("", 0, &terminal_view(), &keymap);
        assert!(row(&rows, CommandId::ShowTimeline).enabled);
        assert!(row(&rows, CommandId::ShowSessionMap).enabled);
        let no_timeline = PaletteWorkspaceView {
            timeline_available: false,
            ..terminal_view()
        };
        let rows = palette_rows("", 0, &no_timeline, &keymap);
        let timeline = row(&rows, CommandId::ShowTimeline);
        assert!(!timeline.enabled);
        assert!(timeline.entry.detail.contains("timeline"));
    }

    #[test]
    fn quit_is_searchable_and_always_available() {
        let keymap = Keymap::default();
        let rows = palette_rows("quit", 0, &terminal_view(), &keymap);
        assert_eq!(rows[0].command_id, CommandId::Quit);
        assert!(rows[0].enabled);
        assert_eq!(rows[0].entry.key_hint.as_deref(), Some("q"));
    }

    #[test]
    fn footer_counts_the_entries_hidden_outside_the_window() {
        assert_eq!(
            palette_footer(0, 0),
            "↑/↓ or ctrl+n/p move · enter run · esc close"
        );
        assert!(palette_footer(0, 4).starts_with("↓ 4 more · "));
        assert!(palette_footer(2, 0).starts_with("↑ 2 more · "));
        assert!(palette_footer(2, 4).starts_with("↑ 2 / ↓ 4 more · "));
    }
}
