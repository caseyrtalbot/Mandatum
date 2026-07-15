//! The attention strip: aggregate what needs the user's eyes right now
//! (approvals waiting, failed tasks, blocked/failed agents) into header
//! segments with resolved rects, so a stranger reads session state from the
//! top line alone. When nothing needs attention the strip shows calm
//! session facts — never blank, never noisy.

use mandatum_core::{AgentStatus, PaneId, PaneKind, Session};
use mandatum_scene::{AttentionSegment, HeaderScene, SceneRect};

use crate::app_state::AppState;

/// One aggregated attention condition before rect resolution.
struct AttentionItem {
    label: String,
    pane: Option<PaneId>,
}

/// How the strip names a pane: its user-facing title ("checks failed ·
/// checks" tells the user WHICH check at a glance), falling back to the id
/// when the pane is gone. Ids stay in the timeline and session map, where
/// audit needs them.
fn pane_name(session: &Session, pane_id: &PaneId) -> String {
    session
        .pane(pane_id)
        .map(|pane| pane.title().to_owned())
        .unwrap_or_else(|| pane_id.to_string())
}

/// Aggregate the active session's attention conditions, in fixed severity
/// order: approvals waiting, failed tasks, blocked/failed agents.
fn attention_items(state: &AppState, session: &Session) -> Vec<AttentionItem> {
    let mut items = Vec::new();

    let waiting: Vec<&PaneId> = session
        .panes()
        .iter()
        .filter(|(_, pane)| {
            matches!(
                pane.kind(),
                PaneKind::Agent { intent } if intent.status == AgentStatus::WaitingForApproval
            )
        })
        .map(|(pane_id, _)| pane_id)
        .collect();
    if let Some(first) = waiting.first() {
        let noun = if waiting.len() == 1 {
            "approval"
        } else {
            "approvals"
        };
        items.push(AttentionItem {
            label: format!(
                "{} {noun} waiting · {}",
                waiting.len(),
                pane_name(session, first)
            ),
            pane: Some((*first).clone()),
        });
    }

    let failed_tasks: Vec<&PaneId> = session
        .panes()
        .iter()
        .filter(|(pane_id, pane)| {
            matches!(pane.kind(), PaneKind::Task { .. })
                && state.task_failure_status(pane_id).is_some()
        })
        .map(|(pane_id, _)| pane_id)
        .collect();
    if let Some(first) = failed_tasks.first() {
        let noun = if failed_tasks.len() == 1 {
            "task"
        } else {
            "tasks"
        };
        items.push(AttentionItem {
            label: format!(
                "{} {noun} failed · {}",
                failed_tasks.len(),
                pane_name(session, first)
            ),
            pane: Some((*first).clone()),
        });
    }

    let stuck_agents = session
        .panes()
        .iter()
        .filter(|(_, pane)| {
            matches!(
                pane.kind(),
                PaneKind::Agent { intent }
                    if matches!(intent.status, AgentStatus::Blocked | AgentStatus::Failed)
            )
        })
        .count();
    if stuck_agents > 0 {
        let noun = if stuck_agents == 1 { "agent" } else { "agents" };
        items.push(AttentionItem {
            label: format!("{stuck_agents} {noun} blocked/failed"),
            pane: None,
        });
    }

    items
}

/// Build the header scene for one frame: the composed strip text plus
/// attention segments at their exact char offsets, so frontends paint the
/// text and restyle the segments without recomputing anything.
pub(crate) fn header_scene(state: &AppState, area: SceneRect) -> HeaderScene {
    let session = state.workspace().active_session();
    let zoomed = session.layout().zoomed().is_some();
    let items = attention_items(state, session);

    let mut text = format!(" {} |", state.workspace().name());
    let mut attention = Vec::with_capacity(items.len());
    if items.is_empty() {
        text.push_str(&format!(
            " {} · {} pane(s) · agent: {}",
            session.name(),
            session.panes().len(),
            state.agent_connector_label(),
        ));
    } else {
        for (index, item) in items.into_iter().enumerate() {
            if index > 0 {
                text.push_str(" ·");
            }
            text.push(' ');
            let start = text.chars().count() as u16;
            text.push_str(&item.label);
            let width = item.label.chars().count() as u16;
            let x = area.x.saturating_add(start);
            let clamped_width = width.min(area.right().saturating_sub(x));
            attention.push(AttentionSegment {
                rect: SceneRect::new(x, area.y, clamped_width, area.height.min(1)),
                label: item.label,
                pane: item.pane,
            });
        }
    }
    if zoomed {
        text.push_str(" · zoom");
    }

    HeaderScene {
        area,
        workspace_name: state.workspace().name().to_owned(),
        session_name: session.name().to_owned(),
        pane_count: session.panes().len(),
        focused_pane: session.focused_pane_id().clone(),
        zoomed,
        connector_label: state.agent_connector_label().to_owned(),
        text,
        attention,
    }
}
