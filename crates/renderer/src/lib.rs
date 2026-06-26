//! Terminal renderer for Mandatum's placeholder workspace shell.
//!
//! This crate renders core workspace state and optional terminal grid snapshots.
//! It does not dispatch product actions or own PTY/runtime lifecycle.

use mandatum_core::{
    FloatingRect, LayoutNode, PaneId, PaneKind, PaneSpec, Session, SplitAxis, Workspace,
};
use mandatum_terminal_vt::{CellStyle, GridPosition, TerminalGrid};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout as RatatuiLayout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteItem<'a> {
    pub label: &'a str,
    pub detail: &'a str,
}

impl<'a> PaletteItem<'a> {
    pub fn new(label: &'a str, detail: &'a str) -> Self {
        Self { label, detail }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteView<'a> {
    pub open: bool,
    pub items: &'a [PaletteItem<'a>],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderState<'a> {
    pub workspace: &'a Workspace,
    pub palette: PaletteView<'a>,
    pub status: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneTerminalGrid<'a> {
    pub pane_id: &'a PaneId,
    pub grid: &'a TerminalGrid,
}

impl<'a> PaneTerminalGrid<'a> {
    pub fn new(pane_id: &'a PaneId, grid: &'a TerminalGrid) -> Self {
        Self { pane_id, grid }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalGridView<'a> {
    pub panes: &'a [PaneTerminalGrid<'a>],
}

impl<'a> TerminalGridView<'a> {
    pub const fn empty() -> Self {
        Self { panes: &[] }
    }

    pub const fn new(panes: &'a [PaneTerminalGrid<'a>]) -> Self {
        Self { panes }
    }

    pub fn for_pane(&self, pane_id: &PaneId) -> Option<&'a TerminalGrid> {
        self.panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .map(|pane| pane.grid)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceScene {
    pub panes: Vec<PaneScene>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneScene {
    pub id: PaneId,
    pub title: String,
    pub kind: &'static str,
    pub area: Rect,
    pub focused: bool,
    pub floating: bool,
    pub stacked: bool,
    pub zoomed: bool,
}

pub fn render(frame: &mut Frame<'_>, state: RenderState<'_>) {
    render_with_terminal_grids(frame, state, TerminalGridView::empty());
}

pub fn workspace_scene_area(area: Rect) -> Rect {
    let chunks = RatatuiLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    chunks[1]
}

pub fn pane_content_area(workspace: &Workspace, area: Rect, pane_id: &PaneId) -> Option<Rect> {
    scene_for_workspace(workspace, workspace_scene_area(area))
        .panes
        .into_iter()
        .find(|pane| &pane.id == pane_id)
        .map(|pane| pane_inner_area(pane.area))
}

pub fn render_with_terminal_grids(
    frame: &mut Frame<'_>,
    state: RenderState<'_>,
    terminal_grids: TerminalGridView<'_>,
) {
    let area = frame.area();
    let chunks = RatatuiLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, chunks[0], state.workspace);
    render_workspace(frame, chunks[1], state.workspace, terminal_grids);
    render_status(frame, chunks[2], state.status);

    if state.palette.open {
        render_palette(frame, area, state.palette.items);
    }
}

pub fn scene_for_workspace(workspace: &Workspace, area: Rect) -> WorkspaceScene {
    let session = workspace.active_session();
    if area.width == 0 || area.height == 0 {
        return WorkspaceScene { panes: Vec::new() };
    }

    if let Some(zoomed) = session.layout().zoomed()
        && let Some(pane) = session.pane(zoomed)
    {
        return WorkspaceScene {
            panes: vec![pane_scene(
                pane,
                area,
                session.focused_pane_id(),
                session.layout().is_floating(zoomed),
                false,
                true,
            )],
        };
    }

    let mut panes = Vec::new();
    collect_layout_panes(session, session.layout().root(), area, false, &mut panes);

    for floating in session.layout().floating() {
        if let Some(pane) = session.pane(&floating.pane_id) {
            panes.push(pane_scene(
                pane,
                floating_rect(area, &floating.rect),
                session.focused_pane_id(),
                true,
                false,
                false,
            ));
        }
    }

    WorkspaceScene { panes }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, workspace: &Workspace) {
    let session = workspace.active_session();
    let zoom = if session.layout().zoomed().is_some() {
        " | zoom"
    } else {
        ""
    };
    let title = format!(
        " Mandatum | {} | panes {} | focused {}{} ",
        session.name(),
        session.panes().len(),
        session.focused_pane_id(),
        zoom
    );
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(Color::White).bg(Color::Black)),
        area,
    );
}

fn render_workspace(
    frame: &mut Frame<'_>,
    area: Rect,
    workspace: &Workspace,
    terminal_grids: TerminalGridView<'_>,
) {
    let scene = scene_for_workspace(workspace, area);
    for pane in scene.panes {
        render_pane(frame, pane, workspace.active_session(), terminal_grids);
    }
}

fn render_pane(
    frame: &mut Frame<'_>,
    pane_scene: PaneScene,
    session: &Session,
    terminal_grids: TerminalGridView<'_>,
) {
    let Some(pane) = session.pane(&pane_scene.id) else {
        return;
    };

    let border_style = if pane_scene.focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let title = pane_title(&pane_scene);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    if matches!(pane.kind(), PaneKind::Terminal { .. })
        && let Some(grid) = terminal_grids.for_pane(&pane_scene.id)
    {
        render_terminal_grid(frame, pane_scene.area, block, grid);
        return;
    }

    let cwd = pane
        .cwd()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "cwd: unset".to_owned());
    let content = vec![
        Line::from(format!("{} {}", pane_scene.id, pane_scene.kind)),
        Line::from(format!("title: {}", pane.title())),
        Line::from(format!("cwd: {cwd}")),
        Line::from(format!("restart generation: {}", pane.restart_generation())),
        Line::from("no live PTY grid is attached to this pane"),
    ];

    frame.render_widget(
        Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: true }),
        pane_scene.area,
    );
}

fn render_terminal_grid(frame: &mut Frame<'_>, area: Rect, block: Block<'_>, grid: &TerminalGrid) {
    let inner_area = pane_inner_area(area);
    let lines = terminal_grid_lines(grid, inner_area.width, inner_area.height);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn pane_inner_area(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2).max(1),
        area.height.saturating_sub(2).max(1),
    )
}

fn terminal_grid_lines(grid: &TerminalGrid, max_width: u16, max_height: u16) -> Vec<Line<'static>> {
    let rows = grid.size().rows().min(max_height);
    let columns = grid.size().columns().min(max_width);
    let cursor = grid.cursor();

    (0..rows)
        .map(|row| {
            let spans = (0..columns)
                .map(|column| {
                    let position = GridPosition::new(row, column);
                    let cell = grid.cell(position).copied().unwrap_or_default();
                    let mut style = terminal_cell_style(cell.style());

                    if cursor.visible() && cursor.position() == position {
                        style = style.add_modifier(Modifier::REVERSED);
                    }

                    Span::styled(cell.character().to_string(), style)
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

fn terminal_cell_style(style: CellStyle) -> Style {
    let mut cell_style = Style::default();
    if style.bold {
        cell_style = cell_style.add_modifier(Modifier::BOLD);
    }
    if style.inverse {
        cell_style = cell_style.add_modifier(Modifier::REVERSED);
    }
    cell_style
}

fn render_status(frame: &mut Frame<'_>, area: Rect, status: Option<&str>) {
    let status = status.unwrap_or("ready");
    frame.render_widget(
        Paragraph::new(format!(" {status}")).style(Style::default().fg(Color::Gray)),
        area,
    );
}

fn render_palette(frame: &mut Frame<'_>, area: Rect, items: &[PaletteItem<'_>]) {
    let overlay = centered_rect(70, 60, area);
    frame.render_widget(Clear, overlay);

    let rows = items
        .iter()
        .map(|item| ListItem::new(format!("{}  {}", item.label, item.detail)))
        .collect::<Vec<_>>();
    let list = List::new(rows).block(
        Block::default()
            .title(" Command Palette ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(list, overlay);
}

fn collect_layout_panes(
    session: &Session,
    node: &LayoutNode,
    area: Rect,
    stacked: bool,
    panes: &mut Vec<PaneScene>,
) {
    match node {
        LayoutNode::Pane { pane_id } => {
            if let Some(pane) = session.pane(pane_id) {
                panes.push(pane_scene(
                    pane,
                    area,
                    session.focused_pane_id(),
                    false,
                    stacked,
                    false,
                ));
            }
        }
        LayoutNode::Split {
            axis,
            first_percent,
            first,
            second,
        } => {
            let direction = match axis {
                SplitAxis::Horizontal => Direction::Horizontal,
                SplitAxis::Vertical => Direction::Vertical,
            };
            let first_percent = (*first_percent).min(100);
            let chunks = RatatuiLayout::default()
                .direction(direction)
                .constraints([
                    Constraint::Percentage(first_percent.into()),
                    Constraint::Percentage((100 - first_percent).into()),
                ])
                .split(area);
            collect_layout_panes(session, first, chunks[0], stacked, panes);
            collect_layout_panes(session, second, chunks[1], stacked, panes);
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
                && let Some(pane) = session.pane(pane_id)
            {
                panes.push(pane_scene(
                    pane,
                    area,
                    session.focused_pane_id(),
                    false,
                    true,
                    false,
                ));
            }
        }
    }
}

fn pane_scene(
    pane: &PaneSpec,
    area: Rect,
    focused_pane: &PaneId,
    floating: bool,
    stacked: bool,
    zoomed: bool,
) -> PaneScene {
    PaneScene {
        id: pane.id().clone(),
        title: pane.title().to_owned(),
        kind: pane_kind_label(pane.kind()),
        area,
        focused: pane.id() == focused_pane,
        floating,
        stacked,
        zoomed,
    }
}

fn pane_kind_label(kind: &PaneKind) -> &'static str {
    match kind {
        PaneKind::Terminal { .. } => "terminal",
        PaneKind::Task { .. } => "task",
        PaneKind::Agent { .. } => "agent",
        PaneKind::StatusLog { .. } => "status",
    }
}

fn pane_title(pane: &PaneScene) -> String {
    let mut parts = vec![pane.title.clone()];
    if pane.focused {
        parts.push("focused".to_owned());
    }
    if pane.floating {
        parts.push("floating".to_owned());
    }
    if pane.stacked {
        parts.push("stack".to_owned());
    }
    if pane.zoomed {
        parts.push("zoom".to_owned());
    }
    format!(" {} ", parts.join(" | "))
}

fn floating_rect(area: Rect, rect: &FloatingRect) -> Rect {
    let x = area
        .x
        .saturating_add(rect.x.min(area.width.saturating_sub(1)));
    let y = area
        .y
        .saturating_add(rect.y.min(area.height.saturating_sub(1)));
    let max_width = area.right().saturating_sub(x).max(1);
    let max_height = area.bottom().saturating_sub(y).max(1);
    Rect::new(x, y, rect.width.min(max_width), rect.height.min(max_height))
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = RatatuiLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let horizontal = RatatuiLayout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mandatum_core::{CoreAction, PaneId, Workspace};
    use mandatum_terminal_vt::{TerminalAdapter, TerminalParser, TerminalSize};
    use ratatui::layout::Rect;

    use super::*;

    fn workspace() -> Workspace {
        Workspace::new("Mandatum", PathBuf::from("/tmp/mandatum"))
    }

    #[test]
    fn scene_contains_split_panes_with_focus_from_core_state() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace.apply_action(CoreAction::SplitDown).unwrap();

        let scene = scene_for_workspace(&workspace, Rect::new(0, 0, 120, 40));

        assert_eq!(scene.panes.len(), 3);
        assert_eq!(
            scene
                .panes
                .iter()
                .find(|pane| pane.focused)
                .map(|pane| pane.id.clone()),
            Some(PaneId::new("pane-3"))
        );
        assert!(scene.panes.iter().all(|pane| !pane.floating));
    }

    #[test]
    fn zoomed_pane_uses_full_scene_area_without_rewriting_layout() {
        let mut workspace = workspace();
        workspace.apply_action(CoreAction::SplitRight).unwrap();
        workspace
            .apply_action(CoreAction::ToggleZoomFocused)
            .unwrap();

        let scene = scene_for_workspace(&workspace, Rect::new(5, 6, 80, 20));

        assert_eq!(scene.panes.len(), 1);
        assert_eq!(scene.panes[0].id, PaneId::new("pane-2"));
        assert_eq!(scene.panes[0].area, Rect::new(5, 6, 80, 20));
        assert!(scene.panes[0].zoomed);
    }

    #[test]
    fn floating_panes_use_durable_rects_over_the_tiled_scene() {
        let mut workspace = workspace();
        workspace
            .apply_action(CoreAction::NewTerminal {
                title: "scratch".to_owned(),
                cwd: None,
            })
            .unwrap();

        let scene = scene_for_workspace(&workspace, Rect::new(0, 0, 120, 40));

        assert_eq!(scene.panes.len(), 2);
        let floating = scene.panes.iter().find(|pane| pane.floating).unwrap();
        assert_eq!(floating.id, PaneId::new("pane-2"));
        assert_eq!(floating.area, Rect::new(8, 4, 96, 28));
    }

    #[test]
    fn pane_content_area_matches_scene_border_geometry() {
        let workspace = workspace();

        let content =
            pane_content_area(&workspace, Rect::new(0, 0, 100, 30), &PaneId::new("pane-1"))
                .unwrap();

        assert_eq!(content, Rect::new(1, 2, 98, 26));
    }

    #[test]
    fn terminal_grid_lines_preserve_content_and_cursor() {
        let mut parser = TerminalParser::new(TerminalSize::new(8, 2).unwrap());
        parser.feed(b"sh\nok").unwrap();

        let lines = terminal_grid_lines(parser.grid(), 8, 2);

        assert_eq!(
            lines[0]
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end(),
            "sh"
        );
        assert_eq!(
            lines[1]
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end(),
            "ok"
        );
        assert!(
            lines[1].spans[2]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn terminal_grid_view_looks_up_grid_by_pane() {
        let pane_id = PaneId::new("pane-1");
        let mut parser = TerminalParser::new(TerminalSize::new(4, 1).unwrap());
        parser.feed(b"ok").unwrap();
        let panes = [PaneTerminalGrid::new(&pane_id, parser.grid())];
        let view = TerminalGridView::new(&panes);

        assert!(view.for_pane(&pane_id).is_some());
        assert!(view.for_pane(&PaneId::new("pane-2")).is_none());
    }
}
