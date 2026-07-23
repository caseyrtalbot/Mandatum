//! Pure layout: core workspace intent plus a frame size in, neutral pane
//! rects out.
//!
//! This owns all pane-rect computation — frontends never compute layout. The
//! percentage math reproduces the geometry the previous ratatui-based
//! renderer produced (cumulative boundaries rounded half-up), so adopting the
//! scene contract changed no pixel.

use mandatum_core::{
    FloatingPane, FloatingRect, LayoutNode, PaneId, Session, SplitAxis, Workspace,
};

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

/// Resolve the core product's default floating-pane intent inside the
/// workspace area for a frame of `size`.
pub fn default_floating_pane_rect(size: SceneSize) -> SceneRect {
    floating_rect(workspace_scene_area(size), &FloatingRect::default())
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

/// The centered execution-timeline overlay rect (larger than the palette:
/// event rows carry timestamps and detail).
pub fn timeline_overlay_rect(size: SceneSize) -> SceneRect {
    centered_rect(80, 70, SceneRect::new(0, 0, size.width, size.height))
}

/// The centered session-map overlay rect.
pub fn session_map_rect(size: SceneSize) -> SceneRect {
    centered_rect(60, 60, SceneRect::new(0, 0, size.width, size.height))
}

/// The centered session-search overlay rect: same footprint as the timeline
/// (result rows carry a source label plus the matched line).
pub fn search_overlay_rect(size: SceneSize) -> SceneRect {
    timeline_overlay_rect(size)
}

/// The centered help overlay rect (tall: it lists the whole keymap).
pub fn help_overlay_rect(size: SceneSize) -> SceneRect {
    centered_rect(70, 80, SceneRect::new(0, 0, size.width, size.height))
}

/// The centered first-run welcome rect, sized to its `line_count` content
/// rows plus the border.
pub fn welcome_rect(size: SceneSize, line_count: u16) -> SceneRect {
    let frame = SceneRect::new(0, 0, size.width, size.height);
    let horizontal = centered_rect(60, 100, frame);
    let height = line_count.saturating_add(2).min(size.height);
    let y = (size.height.saturating_sub(height)) / 2;
    SceneRect::new(horizontal.x, y, horizontal.width, height)
}

/// The centered one-line prompt overlay rect (Set agent objective): 60% of
/// the width, three inner rows (input plus breathing room and footer).
pub fn prompt_rect(size: SceneSize) -> SceneRect {
    let frame = SceneRect::new(0, 0, size.width, size.height);
    let horizontal = centered_rect(60, 100, frame);
    let height = 5.min(size.height);
    let y = (size.height.saturating_sub(height)) / 2;
    SceneRect::new(horizontal.x, y, horizontal.width, height)
}

/// The items visible in a scrolling list given `rows` visible rows: scrolled
/// just far enough to keep the selected item in view. Frontends and
/// hit-target builders share this math so pointer rows and drawn rows can
/// never disagree.
pub fn list_item_window(
    rows: usize,
    item_count: usize,
    selected: Option<usize>,
) -> core::ops::Range<usize> {
    if rows == 0 || item_count == 0 {
        return 0..0;
    }
    let selected = selected.unwrap_or(0).min(item_count - 1);
    let start = (selected + 1).saturating_sub(rows);
    start..(start + rows).min(item_count)
}

/// The palette/timeline items visible inside an overlay's inner rect: the
/// top inner row holds the filter input and the bottom inner row holds the
/// footer, so items get `height - 2` rows.
pub fn palette_item_window(
    inner: SceneRect,
    item_count: usize,
    selected: Option<usize>,
) -> core::ops::Range<usize> {
    list_item_window(
        usize::from(inner.height.saturating_sub(2)),
        item_count,
        selected,
    )
}

/// The session-map rows visible inside its inner rect: only the bottom inner
/// row is reserved (the footer); there is no filter input.
pub fn session_map_item_window(
    inner: SceneRect,
    item_count: usize,
    selected: Option<usize>,
) -> core::ops::Range<usize> {
    list_item_window(
        usize::from(inner.height.saturating_sub(1)),
        item_count,
        selected,
    )
}

/// One draggable split boundary, resolved to cell geometry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeparatorLayout {
    /// Preorder index of the split in the layout tree, matching
    /// `mandatum_core::Layout::set_split_percent`.
    pub split_index: usize,
    pub axis: SplitAxis,
    /// The grabbable strip: the two adjacent pane-border columns (horizontal
    /// splits) or rows (vertical splits) along the boundary.
    pub area: SceneRect,
    /// The full area the split divides, for turning a pointer position into
    /// a ratio while dragging.
    pub split_area: SceneRect,
}

/// Resolve the active session's split boundaries into draggable separator
/// strips within `area`. Empty while a pane is zoomed (there is nothing to
/// resize on screen).
pub fn layout_separators(workspace: &Workspace, area: SceneRect) -> Vec<SeparatorLayout> {
    let session = workspace.active_session();
    if area.is_empty() {
        return Vec::new();
    }
    if let Some(zoomed) = session.layout().zoomed()
        && session.pane(zoomed).is_some()
    {
        return Vec::new();
    }

    let mut separators = Vec::new();
    let mut next_index = 0;
    collect_separators(
        session.layout().root(),
        area,
        &mut next_index,
        &mut separators,
    );
    separators
}

fn collect_separators(
    node: &LayoutNode,
    area: SceneRect,
    next_index: &mut usize,
    separators: &mut Vec<SeparatorLayout>,
) {
    let LayoutNode::Split {
        axis,
        first_percent,
        first,
        second,
    } = node
    else {
        return;
    };

    // Preorder split indices count every split, even degenerate ones, so the
    // numbering always matches core's `set_split_percent` traversal.
    let split_index = *next_index;
    *next_index += 1;

    let (first_area, second_area) = split_rect(area, *axis, u16::from((*first_percent).min(100)));
    if !first_area.is_empty() && !second_area.is_empty() {
        let strip = match axis {
            SplitAxis::Horizontal => SceneRect::new(
                second_area.x.saturating_sub(1),
                area.y,
                2.min(area.width),
                area.height,
            ),
            SplitAxis::Vertical => SceneRect::new(
                area.x,
                second_area.y.saturating_sub(1),
                area.width,
                2.min(area.height),
            ),
        };
        separators.push(SeparatorLayout {
            split_index,
            axis: *axis,
            area: strip,
            split_area: area,
        });
    }

    collect_separators(first, first_area, next_index, separators);
    collect_separators(second, second_area, next_index, separators);
}

/// The context-menu overlay rect: anchored at the pointer, sized to its
/// content, and clamped inside the frame.
pub fn context_menu_rect(
    anchor_column: u16,
    anchor_row: u16,
    width: u16,
    height: u16,
    size: SceneSize,
) -> SceneRect {
    let width = width.min(size.width);
    let height = height.min(size.height);
    let x = anchor_column.min(size.width.saturating_sub(width));
    let y = anchor_row.min(size.height.saturating_sub(height));
    SceneRect::new(x, y, width, height)
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

    // Floats paint (and hit-test) in list order, so the focused float is
    // moved last: focusing a buried float — via the attention strip, the
    // session map, or focus cycling — must raise it, never leave the user
    // deciding an approval on a pane they cannot see. The stable sort keeps
    // the durable order among unfocused floats.
    let mut floats: Vec<&FloatingPane> = session
        .layout()
        .floating()
        .iter()
        .filter(|floating| session.pane(&floating.pane_id).is_some())
        .collect();
    floats.sort_by_key(|floating| &floating.pane_id == session.focused_pane_id());
    for floating in floats {
        panes.push(PaneLayout {
            pane_id: floating.pane_id.clone(),
            area: floating_rect(area, &floating.rect),
            floating: true,
            stacked: false,
            zoomed: false,
        });
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
    fn separators_carry_preorder_split_identity_and_boundary_strips() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();

        let area = SceneRect::new(0, 0, 120, 40);
        let separators = layout_separators(&workspace, area);

        assert_eq!(separators.len(), 2);
        // Split 0: the root horizontal split at column 60 — the strip covers
        // the two adjacent border columns.
        assert_eq!(separators[0].split_index, 0);
        assert_eq!(separators[0].axis, SplitAxis::Horizontal);
        assert_eq!(separators[0].area, SceneRect::new(59, 0, 2, 40));
        assert_eq!(separators[0].split_area, area);
        // Split 1: the nested vertical split of the right half at row 20.
        assert_eq!(separators[1].split_index, 1);
        assert_eq!(separators[1].axis, SplitAxis::Vertical);
        assert_eq!(separators[1].area, SceneRect::new(60, 19, 60, 2));
        assert_eq!(separators[1].split_area, SceneRect::new(60, 0, 60, 40));

        // The identity matches core's preorder addressing: adjusting split 1
        // moves the nested boundary the separator described.
        workspace
            .apply_action(CoreAction::SetSplitRatio {
                split_index: 1,
                first_percent: 25,
            })
            .unwrap();
        let moved = layout_separators(&workspace, area);
        assert_eq!(moved[1].area, SceneRect::new(60, 9, 60, 2));
    }

    #[test]
    fn zoom_suppresses_separators() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace
            .apply_action(CoreAction::ToggleZoomFocused)
            .unwrap();

        assert!(layout_separators(&workspace, SceneRect::new(0, 0, 120, 40)).is_empty());
    }

    #[test]
    fn context_menu_rect_clamps_inside_the_frame() {
        let size = SceneSize::new(100, 30);
        assert_eq!(
            context_menu_rect(10, 5, 24, 8, size),
            SceneRect::new(10, 5, 24, 8)
        );
        // Near the bottom-right corner the menu shifts up and left.
        assert_eq!(
            context_menu_rect(95, 28, 24, 8, size),
            SceneRect::new(76, 22, 24, 8)
        );
        // A menu larger than the frame is clipped to it.
        assert_eq!(
            context_menu_rect(0, 0, 200, 60, size),
            SceneRect::new(0, 0, 100, 30)
        );
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
    fn default_floating_pane_rect_resolves_the_product_80x24_viewport() {
        assert_eq!(
            default_floating_pane_rect(SceneSize::new(80, 24)),
            SceneRect::new(8, 5, 72, 18)
        );
    }

    #[test]
    fn default_floating_pane_rect_clamps_inside_a_small_viewport() {
        assert_eq!(
            default_floating_pane_rect(SceneSize::new(6, 3)),
            SceneRect::new(5, 1, 1, 1)
        );
    }

    // Focusing a buried float must raise it: the focused float is always
    // last in paint order (frontends draw panes in list order, and hit
    // testing scans in reverse), so an approval prompt can never sit hidden
    // behind another float while its keys are live.
    #[test]
    fn focused_floating_pane_is_raised_to_the_top_of_the_float_stack() {
        let mut workspace = workspace();
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "first float".to_owned(),
                cwd: None,
            })
            .unwrap();
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "second float".to_owned(),
                cwd: None,
            })
            .unwrap();

        // Focus the older (durably lower) float; it must paint last.
        workspace
            .apply_action(CoreAction::FocusPane {
                pane_id: PaneId::new("pane-2"),
            })
            .unwrap();
        let panes = layout_panes(&workspace, SceneRect::new(0, 0, 120, 40));
        assert_eq!(panes.last().unwrap().pane_id, PaneId::new("pane-2"));

        // Focus moving to the other float raises that one instead.
        workspace
            .apply_action(CoreAction::FocusPane {
                pane_id: PaneId::new("pane-3"),
            })
            .unwrap();
        let panes = layout_panes(&workspace, SceneRect::new(0, 0, 120, 40));
        assert_eq!(panes.last().unwrap().pane_id, PaneId::new("pane-3"));

        // A focused tiled pane leaves the durable float order untouched.
        workspace
            .apply_action(CoreAction::FocusPane {
                pane_id: PaneId::new("pane-1"),
            })
            .unwrap();
        let panes = layout_panes(&workspace, SceneRect::new(0, 0, 120, 40));
        let float_ids: Vec<_> = panes
            .iter()
            .filter(|pane| pane.floating)
            .map(|pane| pane.pane_id.clone())
            .collect();
        assert_eq!(
            float_ids,
            vec![PaneId::new("pane-2"), PaneId::new("pane-3")]
        );
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

    #[test]
    fn visibility_overlay_rects_center_and_clamp() {
        let size = SceneSize::new(100, 30);
        // Timeline: centered 80% x 70%.
        assert_eq!(timeline_overlay_rect(size), SceneRect::new(10, 5, 80, 21));
        // Session map: centered 60% x 60%.
        assert_eq!(session_map_rect(size), SceneRect::new(20, 6, 60, 18));
        // Prompt: centered 60% wide, five rows tall.
        assert_eq!(prompt_rect(size), SceneRect::new(20, 12, 60, 5));
        // A tiny frame clamps the prompt height.
        assert_eq!(prompt_rect(SceneSize::new(10, 3)).height, 3);
    }

    #[test]
    fn session_map_window_reserves_only_the_footer_row() {
        let inner = SceneRect::new(1, 1, 40, 10); // 9 item rows
        assert_eq!(session_map_item_window(inner, 24, Some(0)), 0..9);
        assert_eq!(session_map_item_window(inner, 24, Some(9)), 1..10);
        assert_eq!(session_map_item_window(inner, 4, Some(3)), 0..4);
        assert_eq!(session_map_item_window(inner, 0, None), 0..0);
    }

    #[test]
    fn palette_item_window_reserves_input_and_footer_rows_and_tracks_selection() {
        let inner = SceneRect::new(1, 1, 40, 10); // 8 item rows
        assert_eq!(palette_item_window(inner, 24, Some(0)), 0..8);
        assert_eq!(palette_item_window(inner, 24, None), 0..8);
        // The selected item is always inside the window.
        assert_eq!(palette_item_window(inner, 24, Some(7)), 0..8);
        assert_eq!(palette_item_window(inner, 24, Some(8)), 1..9);
        assert_eq!(palette_item_window(inner, 24, Some(23)), 16..24);
        // Out-of-range selection clamps to the last item.
        assert_eq!(palette_item_window(inner, 24, Some(99)), 16..24);
        // Fewer items than rows and degenerate heights.
        assert_eq!(palette_item_window(inner, 3, Some(2)), 0..3);
        assert_eq!(
            palette_item_window(SceneRect::new(1, 1, 40, 2), 24, Some(0)),
            0..0
        );
        assert_eq!(palette_item_window(inner, 0, None), 0..0);
    }
}
