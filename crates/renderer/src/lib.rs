//! Terminal renderer for Mandatum's workspace shell.
//!
//! This crate renders core workspace state and optional terminal grid snapshots.
//! It does not dispatch product actions or own PTY/runtime lifecycle.

use mandatum_core::{
    FloatingRect, LayoutNode, PaneId, PaneKind, PaneSpec, Session, SplitAxis, TaskPaneIntent,
    Workspace,
};
use mandatum_terminal_vt::{CellStyle, Color as VtColor, TerminalGrid};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout as RatatuiLayout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

/// A point in the combined scrollback-plus-screen buffer, in absolute
/// coordinates: rows `0..scrollback_len` index history, rows at and beyond
/// `scrollback_len` index the live screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct SelectionPoint {
    pub row: usize,
    pub column: u16,
}

impl SelectionPoint {
    pub fn new(row: usize, column: u16) -> Self {
        Self { row, column }
    }
}

/// Read-only presentation state describing how a terminal pane's grid is being
/// viewed: how far it is scrolled into history, an optional ordered selection
/// span, and an optional copy-mode cursor. The default is "follow live output".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TerminalViewport {
    /// Rows scrolled up from the live bottom. `0` follows live output.
    pub scroll_offset: usize,
    /// Inclusive selection span, pre-ordered so `start <= end` in reading order.
    pub selection: Option<(SelectionPoint, SelectionPoint)>,
    /// Copy-mode cursor position; `Some` only while a pane is in copy mode.
    pub copy_cursor: Option<SelectionPoint>,
}

impl TerminalViewport {
    /// The default viewport: following live output, no selection, no copy cursor.
    pub fn live() -> Self {
        Self::default()
    }

    fn in_copy_mode(&self) -> bool {
        self.copy_cursor.is_some() || self.scroll_offset > 0
    }
}

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
    pub viewport: TerminalViewport,
}

impl<'a> PaneTerminalGrid<'a> {
    /// A pane terminal grid following live output (no scroll/selection).
    pub fn new(pane_id: &'a PaneId, grid: &'a TerminalGrid) -> Self {
        Self {
            pane_id,
            grid,
            viewport: TerminalViewport::live(),
        }
    }

    pub fn with_viewport(
        pane_id: &'a PaneId,
        grid: &'a TerminalGrid,
        viewport: TerminalViewport,
    ) -> Self {
        Self {
            pane_id,
            grid,
            viewport,
        }
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
        self.entry(pane_id).map(|pane| pane.grid)
    }

    pub fn entry(&self, pane_id: &PaneId) -> Option<&PaneTerminalGrid<'a>> {
        self.panes.iter().find(|pane| pane.pane_id == pane_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneTaskRuntime<'a> {
    pub pane_id: &'a PaneId,
    pub status: &'a str,
    pub output: Option<&'a TerminalGrid>,
    pub viewport: TerminalViewport,
}

impl<'a> PaneTaskRuntime<'a> {
    pub fn new(pane_id: &'a PaneId, status: &'a str) -> Self {
        Self {
            pane_id,
            status,
            output: None,
            viewport: TerminalViewport::live(),
        }
    }

    pub fn with_output(pane_id: &'a PaneId, status: &'a str, output: &'a TerminalGrid) -> Self {
        Self {
            pane_id,
            status,
            output: Some(output),
            viewport: TerminalViewport::live(),
        }
    }

    pub fn with_output_viewport(
        pane_id: &'a PaneId,
        status: &'a str,
        output: &'a TerminalGrid,
        viewport: TerminalViewport,
    ) -> Self {
        Self {
            pane_id,
            status,
            output: Some(output),
            viewport,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskRuntimeView<'a> {
    pub panes: &'a [PaneTaskRuntime<'a>],
}

impl<'a> TaskRuntimeView<'a> {
    pub const fn empty() -> Self {
        Self { panes: &[] }
    }

    pub const fn new(panes: &'a [PaneTaskRuntime<'a>]) -> Self {
        Self { panes }
    }

    pub fn entry(&self, pane_id: &PaneId) -> Option<&PaneTaskRuntime<'a>> {
        self.panes.iter().find(|pane| pane.pane_id == pane_id)
    }

    pub fn output_for_pane(&self, pane_id: &PaneId) -> Option<&'a TerminalGrid> {
        self.entry(pane_id).and_then(|pane| pane.output)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimePaneViews<'a> {
    pub terminal_grids: TerminalGridView<'a>,
    pub task_panes: TaskRuntimeView<'a>,
}

impl<'a> RuntimePaneViews<'a> {
    pub const fn empty() -> Self {
        Self {
            terminal_grids: TerminalGridView::empty(),
            task_panes: TaskRuntimeView::empty(),
        }
    }

    pub const fn new(
        terminal_grids: TerminalGridView<'a>,
        task_panes: TaskRuntimeView<'a>,
    ) -> Self {
        Self {
            terminal_grids,
            task_panes,
        }
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
    render_with_runtime_views(
        frame,
        state,
        RuntimePaneViews::new(terminal_grids, TaskRuntimeView::empty()),
    );
}

pub fn render_with_runtime_views(
    frame: &mut Frame<'_>,
    state: RenderState<'_>,
    runtime_views: RuntimePaneViews<'_>,
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
    render_workspace(frame, chunks[1], state.workspace, runtime_views);
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
    runtime_views: RuntimePaneViews<'_>,
) {
    let scene = scene_for_workspace(workspace, area);
    for pane in scene.panes {
        render_pane(frame, pane, workspace.active_session(), runtime_views);
    }
}

fn render_pane(
    frame: &mut Frame<'_>,
    pane_scene: PaneScene,
    session: &Session,
    runtime_views: RuntimePaneViews<'_>,
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

    if matches!(pane.kind(), PaneKind::Terminal { .. })
        && let Some(entry) = runtime_views.terminal_grids.entry(&pane_scene.id)
    {
        let title = pane_title(&pane_scene, entry.viewport.in_copy_mode());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);
        render_terminal_grid(frame, pane_scene.area, block, entry.grid, entry.viewport);
        return;
    }

    if let PaneKind::Task { intent } = pane.kind() {
        let title = pane_title(&pane_scene, false);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);
        render_task_pane(
            frame,
            pane_scene.area,
            block,
            &pane_scene,
            pane,
            intent,
            runtime_views.task_panes.entry(&pane_scene.id),
        );
        return;
    }

    let title = pane_title(&pane_scene, false);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

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

fn render_terminal_grid(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block<'_>,
    grid: &TerminalGrid,
    viewport: TerminalViewport,
) {
    let inner_area = pane_inner_area(area);
    let lines = terminal_grid_lines(grid, viewport, inner_area.width, inner_area.height);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_task_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    block: Block<'_>,
    pane_scene: &PaneScene,
    pane: &PaneSpec,
    intent: &TaskPaneIntent,
    runtime: Option<&PaneTaskRuntime<'_>>,
) {
    let inner_area = pane_inner_area(area);
    let lines = task_pane_lines(
        pane_scene,
        pane,
        intent,
        runtime,
        inner_area.width,
        inner_area.height,
    );
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn task_pane_lines(
    pane_scene: &PaneScene,
    pane: &PaneSpec,
    intent: &TaskPaneIntent,
    runtime: Option<&PaneTaskRuntime<'_>>,
    max_width: u16,
    max_height: u16,
) -> Vec<Line<'static>> {
    if max_height == 0 {
        return Vec::new();
    }

    let mut lines = vec![
        Line::from(format!("{} {}", pane_scene.id, pane_scene.kind)),
        Line::from(format!("title: {}", pane.title())),
        Line::from(format!("command: {}", intent.command)),
        Line::from(format!("cwd: {}", task_cwd_label(pane, intent))),
        Line::from(format!(
            "recipe: {}",
            intent.recipe_id.as_deref().unwrap_or("ad hoc")
        )),
    ];

    match runtime {
        Some(runtime) => {
            lines.push(Line::from(format!("runtime status: {}", runtime.status)));
            if let Some(output) = runtime.output {
                lines.push(Line::from("output:"));
                let output_height = max_height.saturating_sub(lines.len() as u16);
                lines.extend(terminal_grid_lines(
                    output,
                    runtime.viewport,
                    max_width,
                    output_height,
                ));
            } else {
                lines.push(Line::from("output: no live grid attached"));
            }
        }
        None => {
            lines.push(Line::from("runtime status: unavailable"));
            lines.push(Line::from("output: no live runtime attached"));
        }
    }

    lines.truncate(usize::from(max_height));
    lines
}

fn task_cwd_label(pane: &PaneSpec, intent: &TaskPaneIntent) -> String {
    intent
        .cwd
        .as_ref()
        .or_else(|| pane.cwd())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unset".to_owned())
}

fn pane_inner_area(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2).max(1),
        area.height.saturating_sub(2).max(1),
    )
}

fn terminal_grid_lines(
    grid: &TerminalGrid,
    viewport: TerminalViewport,
    max_width: u16,
    max_height: u16,
) -> Vec<Line<'static>> {
    let view_rows = usize::from(grid.size().rows().min(max_height));
    let columns = grid.size().columns().min(max_width);
    let total_rows = grid.total_rows();
    let scrollback_len = grid.scrollback_len();
    let cursor = grid.cursor();

    // Top visible absolute row, clamped so the viewport never runs off the end.
    let max_top = total_rows.saturating_sub(view_rows);
    let first_visible = max_top.saturating_sub(viewport.scroll_offset);

    // The live cursor is only drawn when following live output and not in copy
    // mode (copy mode draws its own cursor instead).
    let show_live_cursor =
        viewport.scroll_offset == 0 && viewport.copy_cursor.is_none() && cursor.visible();
    let live_cursor_row = scrollback_len + usize::from(cursor.row());

    (0..view_rows)
        .map(|line| {
            let absolute_row = first_visible + line;
            let spans = (0..columns)
                .map(|column| {
                    let cell = grid.history_cell(absolute_row, column).unwrap_or_default();
                    let mut style = terminal_cell_style(cell.style());

                    if selection_contains(viewport.selection, absolute_row, column) {
                        style = style.add_modifier(Modifier::REVERSED);
                    }

                    let copy_cursor_here =
                        viewport.copy_cursor == Some(SelectionPoint::new(absolute_row, column));
                    let live_cursor_here = show_live_cursor
                        && absolute_row == live_cursor_row
                        && column == cursor.column();
                    if copy_cursor_here || live_cursor_here {
                        style = style.add_modifier(Modifier::REVERSED);
                    }

                    Span::styled(cell.character().to_string(), style)
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

fn selection_contains(
    selection: Option<(SelectionPoint, SelectionPoint)>,
    row: usize,
    column: u16,
) -> bool {
    let Some((start, end)) = selection else {
        return false;
    };
    let after_start = row > start.row || (row == start.row && column >= start.column);
    let before_end = row < end.row || (row == end.row && column <= end.column);
    after_start && before_end
}

fn terminal_cell_style(style: CellStyle) -> Style {
    let mut cell_style = Style::default();
    if style.foreground != VtColor::Default {
        cell_style = cell_style.fg(map_color(style.foreground));
    }
    if style.background != VtColor::Default {
        cell_style = cell_style.bg(map_color(style.background));
    }
    if style.bold {
        cell_style = cell_style.add_modifier(Modifier::BOLD);
    }
    if style.dim {
        cell_style = cell_style.add_modifier(Modifier::DIM);
    }
    if style.italic {
        cell_style = cell_style.add_modifier(Modifier::ITALIC);
    }
    if style.underline {
        cell_style = cell_style.add_modifier(Modifier::UNDERLINED);
    }
    if style.inverse {
        cell_style = cell_style.add_modifier(Modifier::REVERSED);
    }
    if style.hidden {
        cell_style = cell_style.add_modifier(Modifier::HIDDEN);
    }
    if style.strikethrough {
        cell_style = cell_style.add_modifier(Modifier::CROSSED_OUT);
    }
    cell_style
}

fn map_color(color: VtColor) -> Color {
    match color {
        VtColor::Default => Color::Reset,
        VtColor::Indexed(index) => Color::Indexed(index),
        VtColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
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

fn pane_title(pane: &PaneScene, copy_mode: bool) -> String {
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
    if copy_mode {
        parts.push("copy".to_owned());
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

    use mandatum_core::{CoreAction, PaneId, PaneKind, TaskPaneIntent, Workspace};
    use mandatum_terminal_vt::{TerminalAdapter, TerminalParser, TerminalSize};
    use ratatui::layout::Rect;

    use super::*;

    fn workspace() -> Workspace {
        Workspace::new("Mandatum", PathBuf::from("/tmp/mandatum"))
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
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
    fn restored_workspace_layout_renders_with_same_geometry() {
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
        let area = Rect::new(0, 0, 120, 40);

        assert_eq!(
            scene_for_workspace(&restored, area),
            scene_for_workspace(&workspace, area)
        );
        for pane_id in workspace.active_session().panes().keys() {
            assert_eq!(
                pane_content_area(&restored, area, pane_id),
                pane_content_area(&workspace, area, pane_id)
            );
        }

        let mut zoomed = restored.clone();
        zoomed.apply_action(CoreAction::ToggleZoomFocused).unwrap();
        let zoomed_restored = Workspace::from_json(&zoomed.to_json().unwrap()).unwrap();

        assert_eq!(
            scene_for_workspace(&zoomed_restored, area),
            scene_for_workspace(&zoomed, area)
        );
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
        // A real PTY emits CRLF; the hardened backend treats LF as line feed
        // (column preserved) and CR as carriage return.
        parser.feed(b"sh\r\nok").unwrap();

        let lines = terminal_grid_lines(parser.grid(), TerminalViewport::live(), 8, 2);

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
    fn viewport_scrolls_into_history_and_highlights_selection() {
        let mut parser = TerminalParser::new(TerminalSize::new(4, 2).unwrap());
        parser.feed(b"aaa\r\nbbb\r\nccc\r\nddd").unwrap();
        let grid = parser.grid();
        assert_eq!(grid.scrollback_len(), 2);

        // Scrolling up by one row brings the previous line into view from history.
        let scrolled = TerminalViewport {
            scroll_offset: 1,
            selection: None,
            copy_cursor: None,
        };
        let lines = terminal_grid_lines(grid, scrolled, 4, 2);
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(text, vec!["bbb", "ccc"]);

        // A selection over the top visible row (absolute row 1) reverses its cells.
        let selected = TerminalViewport {
            scroll_offset: 1,
            selection: Some((SelectionPoint::new(1, 0), SelectionPoint::new(1, 2))),
            copy_cursor: Some(SelectionPoint::new(1, 2)),
        };
        let lines = terminal_grid_lines(grid, selected, 4, 2);
        assert!(
            lines[0].spans[0]
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

    #[test]
    fn task_runtime_view_looks_up_status_and_optional_output_by_pane() {
        let pane_id = PaneId::new("pane-1");
        let mut parser = TerminalParser::new(TerminalSize::new(8, 1).unwrap());
        parser.feed(b"ok").unwrap();
        let panes = [PaneTaskRuntime::with_output(
            &pane_id,
            "running: cargo test",
            parser.grid(),
        )];
        let view = TaskRuntimeView::new(&panes);

        assert_eq!(
            view.entry(&pane_id).map(|entry| entry.status),
            Some("running: cargo test")
        );
        assert!(view.output_for_pane(&pane_id).is_some());
        assert!(view.entry(&PaneId::new("pane-2")).is_none());
        assert!(view.output_for_pane(&PaneId::new("pane-2")).is_none());
    }

    #[test]
    fn task_pane_lines_render_intent_with_live_runtime_status_and_output() {
        let mut workspace = workspace();
        let pane_id = workspace.active_session_mut().add_floating_pane(
            "tests",
            PaneKind::Task {
                intent: TaskPaneIntent {
                    recipe_id: Some("test".to_owned()),
                    command: "cargo test".to_owned(),
                    cwd: Some(PathBuf::from("/tmp/project")),
                },
            },
            Some(PathBuf::from("/tmp/project")),
        );
        let pane = workspace.active_session().pane(&pane_id).unwrap();
        let PaneKind::Task { intent } = pane.kind() else {
            panic!("fixture must create a task pane");
        };
        let pane_scene = PaneScene {
            id: pane_id.clone(),
            title: pane.title().to_owned(),
            kind: "task",
            area: Rect::new(0, 0, 40, 12),
            focused: false,
            floating: true,
            stacked: false,
            zoomed: false,
        };
        let mut parser = TerminalParser::new(TerminalSize::new(16, 2).unwrap());
        parser.feed(b"running 1 test\r\nFAILED").unwrap();
        let runtime = PaneTaskRuntime::with_output(&pane_id, "failed: exit 101", parser.grid());

        let lines = task_pane_lines(&pane_scene, pane, intent, Some(&runtime), 40, 10);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(text.iter().any(|line| line == "command: cargo test"));
        assert!(text.iter().any(|line| line == "cwd: /tmp/project"));
        assert!(text.iter().any(|line| line == "recipe: test"));
        assert!(
            text.iter()
                .any(|line| line == "runtime status: failed: exit 101")
        );
        assert!(text.iter().any(|line| line.trim_end() == "running 1 test"));
        assert!(text.iter().any(|line| line.trim_end() == "FAILED"));
        assert!(!text.iter().any(|line| line.contains("Pending")));
    }

    #[test]
    fn task_pane_lines_report_unavailable_when_no_runtime_view_exists() {
        let mut workspace = workspace();
        let pane_id = workspace.active_session_mut().add_floating_pane(
            "build",
            PaneKind::Task {
                intent: TaskPaneIntent {
                    recipe_id: Some("build".to_owned()),
                    command: "cargo build".to_owned(),
                    cwd: None,
                },
            },
            Some(PathBuf::from("/tmp/mandatum")),
        );
        let pane = workspace.active_session().pane(&pane_id).unwrap();
        let PaneKind::Task { intent } = pane.kind() else {
            panic!("fixture must create a task pane");
        };
        let pane_scene = PaneScene {
            id: pane_id,
            title: pane.title().to_owned(),
            kind: "task",
            area: Rect::new(0, 0, 40, 10),
            focused: false,
            floating: true,
            stacked: false,
            zoomed: false,
        };

        let lines = task_pane_lines(&pane_scene, pane, intent, None, 40, 8);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(text.iter().any(|line| line == "cwd: /tmp/mandatum"));
        assert!(
            text.iter()
                .any(|line| line == "runtime status: unavailable")
        );
        assert!(!text.iter().any(|line| line.contains("Running")));
    }
}
