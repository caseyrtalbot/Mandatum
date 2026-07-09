//! Pure layout: core workspace intent plus a frame size in, neutral pane
//! rects out.
//!
//! This owns all pane-rect computation — frontends never compute layout. The
//! percentage math reproduces the geometry the previous ratatui-based
//! renderer produced (cumulative boundaries rounded half-up), so adopting the
//! scene contract changed no pixel.

use mandatum_core::{FloatingRect, LayoutNode, PaneId, Session, SplitAxis, Workspace};

use crate::geometry::{SceneRect, SceneSize};

/// A laid-out pane: identity, resolved rect, and how the layout placed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneLayout {
    pub pane_id: PaneId,
    pub area: SceneRect,
    pub floating: bool,
    pub stacked: bool,
    pub zoomed: bool,
}

/// The header strip rect at the top of the frame.
pub fn header_rect(size: SceneSize) -> SceneRect {
    let height = if size.height >= 2 { 1 } else { 0 };
    SceneRect::new(0, 0, size.width, height)
}

/// The workspace area between the header and status strips. The middle keeps
/// at least one row while any rows exist, matching the previous renderer's
/// `[Length(1), Min(1), Length(1)]` chunking.
pub fn workspace_scene_area(size: SceneSize) -> SceneRect {
    match size.height {
        0 => SceneRect::new(0, 0, size.width, 0),
        1 => SceneRect::new(0, 0, size.width, 1),
        2 => SceneRect::new(0, 1, size.width, 1),
        height => SceneRect::new(0, 1, size.width, height - 2),
    }
}

/// The status strip rect at the bottom of the frame.
pub fn status_rect(size: SceneSize) -> SceneRect {
    if size.height >= 3 {
        SceneRect::new(0, size.height - 1, size.width, 1)
    } else {
        SceneRect::new(0, size.height, size.width, 0)
    }
}

/// The centered command-palette overlay rect for a frame of `size`.
pub fn palette_overlay_rect(size: SceneSize) -> SceneRect {
    centered_rect(70, 60, SceneRect::new(0, 0, size.width, size.height))
}

/// Resolve the active session's layout tree into pane rects within `area`,
/// tiled panes first, floating panes on top. A zoomed pane takes the whole
/// area without rewriting layout intent.
pub fn layout_panes(workspace: &Workspace, area: SceneRect) -> Vec<PaneLayout> {
    let session = workspace.active_session();
    if area.is_empty() {
        return Vec::new();
    }

    if let Some(zoomed) = session.layout().zoomed()
        && session.pane(zoomed).is_some()
    {
        return vec![PaneLayout {
            pane_id: zoomed.clone(),
            area,
            floating: session.layout().is_floating(zoomed),
            stacked: false,
            zoomed: true,
        }];
    }

    let mut panes = Vec::new();
    collect_layout_panes(session, session.layout().root(), area, false, &mut panes);

    for floating in session.layout().floating() {
        if session.pane(&floating.pane_id).is_some() {
            panes.push(PaneLayout {
                pane_id: floating.pane_id.clone(),
                area: floating_rect(area, &floating.rect),
                floating: true,
                stacked: false,
                zoomed: false,
            });
        }
    }

    panes
}

/// The content area inside a pane's one-cell border.
pub fn pane_inner_rect(area: SceneRect) -> SceneRect {
    SceneRect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2).max(1),
        area.height.saturating_sub(2).max(1),
    )
}

/// The inner content rect of one pane for a frame of `size`, if the pane is
/// visible. This is what runtime PTY sizes follow.
pub fn pane_content_rect(
    workspace: &Workspace,
    size: SceneSize,
    pane_id: &PaneId,
) -> Option<SceneRect> {
    layout_panes(workspace, workspace_scene_area(size))
        .into_iter()
        .find(|pane| &pane.pane_id == pane_id)
        .map(|pane| pane_inner_rect(pane.area))
}

fn collect_layout_panes(
    session: &Session,
    node: &LayoutNode,
    area: SceneRect,
    stacked: bool,
    panes: &mut Vec<PaneLayout>,
) {
    match node {
        LayoutNode::Pane { pane_id } => {
            if session.pane(pane_id).is_some() {
                panes.push(PaneLayout {
                    pane_id: pane_id.clone(),
                    area,
                    floating: false,
                    stacked,
                    zoomed: false,
                });
            }
        }
        LayoutNode::Split {
            axis,
            first_percent,
            first,
            second,
        } => {
            let (first_area, second_area) =
                split_rect(area, *axis, u16::from((*first_percent).min(100)));
            collect_layout_panes(session, first, first_area, stacked, panes);
            collect_layout_panes(session, second, second_area, stacked, panes);
        }
        LayoutNode::Stack {
            active,
            panes: stack_panes,
        } => {
            let visible = stack_panes
                .iter()
                .find(|pane_id| *pane_id == session.focused_pane_id())
                .or_else(|| stack_panes.iter().find(|pane_id| *pane_id == active))
                .or_else(|| stack_panes.first());
            if let Some(pane_id) = visible
                && session.pane(pane_id).is_some()
            {
                panes.push(PaneLayout {
                    pane_id: pane_id.clone(),
                    area,
                    floating: false,
                    stacked: true,
                    zoomed: false,
                });
            }
        }
    }
}

fn split_rect(area: SceneRect, axis: SplitAxis, first_percent: u16) -> (SceneRect, SceneRect) {
    match axis {
        SplitAxis::Horizontal => {
            let first = percentage_boundary(area.width, first_percent);
            (
                SceneRect::new(area.x, area.y, first, area.height),
                SceneRect::new(area.x + first, area.y, area.width - first, area.height),
            )
        }
        SplitAxis::Vertical => {
            let first = percentage_boundary(area.height, first_percent);
            (
                SceneRect::new(area.x, area.y, area.width, first),
                SceneRect::new(area.x, area.y + first, area.width, area.height - first),
            )
        }
    }
}

/// The cell boundary at `percent` of `length`, rounded half-up.
fn percentage_boundary(length: u16, percent: u16) -> u16 {
    ((u32::from(length) * u32::from(percent) + 50) / 100) as u16
}

fn floating_rect(area: SceneRect, rect: &FloatingRect) -> SceneRect {
    let x = area
        .x
        .saturating_add(rect.x.min(area.width.saturating_sub(1)));
    let y = area
        .y
        .saturating_add(rect.y.min(area.height.saturating_sub(1)));
    let max_width = area.right().saturating_sub(x).max(1);
    let max_height = area.bottom().saturating_sub(y).max(1);
    SceneRect::new(x, y, rect.width.min(max_width), rect.height.min(max_height))
}

fn centered_rect(percent_x: u16, percent_y: u16, area: SceneRect) -> SceneRect {
    let margin_x = (100 - percent_x) / 2;
    let margin_y = (100 - percent_y) / 2;
    let left = percentage_boundary(area.width, margin_x);
    let right = percentage_boundary(area.width, margin_x + percent_x);
    let top = percentage_boundary(area.height, margin_y);
    let bottom = percentage_boundary(area.height, margin_y + percent_y);
    SceneRect::new(area.x + left, area.y + top, right - left, bottom - top)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mandatum_core::{CoreAction, Workspace};

    use super::*;

    fn workspace() -> Workspace {
        Workspace::new("Mandatum", PathBuf::from("/tmp/mandatum"))
    }

    fn rect_for(panes: &[PaneLayout], pane_id: &str) -> SceneRect {
        panes
            .iter()
            .find(|pane| pane.pane_id == PaneId::new(pane_id))
            .map(|pane| pane.area)
            .expect("pane must be laid out")
    }

    #[test]
    fn splits_tile_the_area_deterministically() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();

        let panes = layout_panes(&workspace, SceneRect::new(0, 0, 120, 40));

        assert_eq!(panes.len(), 3);
        assert_eq!(rect_for(&panes, "pane-1"), SceneRect::new(0, 0, 60, 40));
        assert_eq!(rect_for(&panes, "pane-2"), SceneRect::new(60, 0, 60, 20));
        assert_eq!(rect_for(&panes, "pane-3"), SceneRect::new(60, 20, 60, 20));
        assert!(panes.iter().all(|pane| !pane.floating && !pane.zoomed));
    }

    #[test]
    fn odd_length_splits_round_half_up_matching_the_previous_renderer() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();

        // ratatui's Percentage(50)/Percentage(50) split of width 101 is 51/50.
        let panes = layout_panes(&workspace, SceneRect::new(0, 0, 101, 10));
        assert_eq!(rect_for(&panes, "pane-1"), SceneRect::new(0, 0, 51, 10));
        assert_eq!(rect_for(&panes, "pane-2"), SceneRect::new(51, 0, 50, 10));
    }

    #[test]
    fn zoomed_pane_uses_full_area_without_rewriting_layout() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace
            .apply_action(CoreAction::ToggleZoomFocused)
            .unwrap();

        let panes = layout_panes(&workspace, SceneRect::new(5, 6, 80, 20));

        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, PaneId::new("pane-2"));
        assert_eq!(panes[0].area, SceneRect::new(5, 6, 80, 20));
        assert!(panes[0].zoomed);
    }

    #[test]
    fn floating_panes_use_durable_rects_over_the_tiled_layout() {
        let mut workspace = workspace();
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "scratch".to_owned(),
                cwd: None,
            })
            .unwrap();

        let panes = layout_panes(&workspace, SceneRect::new(0, 0, 120, 40));

        assert_eq!(panes.len(), 2);
        let floating = panes.iter().find(|pane| pane.floating).unwrap();
        assert_eq!(floating.pane_id, PaneId::new("pane-2"));
        assert_eq!(floating.area, SceneRect::new(8, 4, 96, 28));
    }

    #[test]
    fn stacks_show_the_focused_pane_in_the_shared_rect() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::FocusPrevious).unwrap();
        workspace
            .apply_action(CoreAction::StackFocusedWithNext)
            .unwrap();

        let area = SceneRect::new(0, 0, 80, 24);
        let panes = layout_panes(&workspace, area);

        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, PaneId::new("pane-1"));
        assert_eq!(panes[0].area, area);
        assert!(panes[0].stacked);
    }

    #[test]
    fn restored_workspace_layout_produces_same_geometry() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();
        workspace.apply_action(CoreAction::FocusPrevious).unwrap();
        workspace
            .apply_action(CoreAction::StackFocusedWithNext)
            .unwrap();
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "scratch".to_owned(),
                cwd: Some(PathBuf::from("/tmp/mandatum")),
            })
            .unwrap();

        let restored = Workspace::from_json(&workspace.to_json().unwrap()).unwrap();
        let area = SceneRect::new(0, 0, 120, 40);
        let size = SceneSize::new(120, 40);

        assert_eq!(
            layout_panes(&restored, area),
            layout_panes(&workspace, area)
        );
        for pane_id in workspace.active_session().panes().keys() {
            assert_eq!(
                pane_content_rect(&restored, size, pane_id),
                pane_content_rect(&workspace, size, pane_id)
            );
        }

        let mut zoomed = restored.clone();
        zoomed.apply_action(CoreAction::ToggleZoomFocused).unwrap();
        let zoomed_restored = Workspace::from_json(&zoomed.to_json().unwrap()).unwrap();

        assert_eq!(
            layout_panes(&zoomed_restored, area),
            layout_panes(&zoomed, area)
        );
    }

    #[test]
    fn pane_content_rect_matches_border_geometry() {
        let workspace = workspace();

        let content =
            pane_content_rect(&workspace, SceneSize::new(100, 30), &PaneId::new("pane-1")).unwrap();

        assert_eq!(content, SceneRect::new(1, 2, 98, 26));
    }

    #[test]
    fn frame_chunks_match_the_previous_renderer_on_small_heights() {
        // Parity values captured from ratatui's [Length(1), Min(1), Length(1)]
        // vertical chunking: the middle keeps at least one row.
        assert_eq!(
            workspace_scene_area(SceneSize::new(80, 0)),
            SceneRect::new(0, 0, 80, 0)
        );
        assert_eq!(
            workspace_scene_area(SceneSize::new(80, 1)),
            SceneRect::new(0, 0, 80, 1)
        );
        assert_eq!(
            workspace_scene_area(SceneSize::new(80, 2)),
            SceneRect::new(0, 1, 80, 1)
        );
        assert_eq!(
            workspace_scene_area(SceneSize::new(80, 24)),
            SceneRect::new(0, 1, 80, 22)
        );
        assert_eq!(header_rect(SceneSize::new(80, 1)).height, 0);
        assert_eq!(header_rect(SceneSize::new(80, 24)).height, 1);
        assert_eq!(
            status_rect(SceneSize::new(80, 24)),
            SceneRect::new(0, 23, 80, 1)
        );
        assert_eq!(status_rect(SceneSize::new(80, 2)).height, 0);
    }

    #[test]
    fn palette_overlay_rect_matches_the_previous_centered_math() {
        // Parity values captured from the previous renderer's centered
        // percentage layout (70% x 60%).
        assert_eq!(
            palette_overlay_rect(SceneSize::new(120, 40)),
            SceneRect::new(18, 8, 84, 24)
        );
        assert_eq!(
            palette_overlay_rect(SceneSize::new(100, 30)),
            SceneRect::new(15, 6, 70, 18)
        );
        assert_eq!(
            palette_overlay_rect(SceneSize::new(101, 31)),
            SceneRect::new(15, 6, 71, 19)
        );
        assert_eq!(
            palette_overlay_rect(SceneSize::new(9, 5)),
            SceneRect::new(1, 1, 7, 3)
        );
    }
}
