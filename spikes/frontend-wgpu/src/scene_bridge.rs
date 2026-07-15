//! The terminal-engine -> scene-contract boundary for the spike.
//!
//! This is the ONLY module that sees both `mandatum-terminal-vt` (the grid) and
//! `mandatum-scene` (the frontend contract). It mirrors, in miniature, what the
//! product app does in `crates/app/src/scene_builder.rs`: window the live grid
//! into a `TerminalSurface` and wrap it in a `WorkspaceScene`. The GPU renderer
//! downstream then consumes only scene types and never names a parser type,
//! which is the clean-adapter conformance the spike is proving.

use mandatum_scene::{
    HeaderScene, PaneContent, PaneId, PaneScene, PaneSceneKind, SceneCell, SceneCellStyle,
    SceneColor, SceneRect, SceneSize, StatusScene, SurfacePosition, TerminalSurface,
    WorkspaceScene,
};
use mandatum_terminal_vt::{CellStyle, Color as VtColor};

use crate::terminal::{Selection, TerminalSession};

pub const SPIKE_PANE_ID: &str = "spike-terminal";

/// Build one frame of workspace scene from the live session. The status string
/// (frontend-owned formatting, carrying fps/latency) is passed through into
/// `WorkspaceScene::status`, exactly the field the renderer reads.
pub fn build_scene(
    session: &TerminalSession,
    selection: Option<Selection>,
    status: &str,
) -> WorkspaceScene {
    let cols = session.cols();
    let rows = session.rows();
    let surface = terminal_surface(session, selection, cols, rows);

    let pane_id = PaneId::new(SPIKE_PANE_ID);
    let shell_name = session.shell_name().to_owned();
    let pane = PaneScene {
        id: pane_id.clone(),
        title: shell_name.clone(),
        kind: PaneSceneKind::Terminal,
        area: SceneRect::new(0, 0, cols, rows),
        focused: true,
        floating: false,
        stacked: false,
        zoomed: false,
        content: PaneContent::Terminal(surface),
    };

    WorkspaceScene {
        // One row below the pane is reserved for the status strip.
        size: SceneSize::new(cols, rows.saturating_add(1)),
        header: HeaderScene {
            // The spike still reserves only the status row. Carry the current
            // header contract so schema drift is caught, but leave its area
            // empty until full workspace chrome becomes a production goal.
            area: SceneRect::new(0, 0, cols, 0),
            workspace_name: "wgpu spike".to_owned(),
            session_name: shell_name.clone(),
            pane_count: 1,
            focused_pane: pane_id.clone(),
            zoomed: false,
            connector_label: "none".to_owned(),
            text: format!("wgpu spike · {shell_name}"),
            attention: Vec::new(),
        },
        panes: vec![pane],
        overlay: None,
        status: StatusScene {
            area: SceneRect::new(0, rows, cols, 1),
            text: status.to_owned(),
        },
        focused_pane: pane_id,
        hit_targets: Vec::new(),
        copy_mode: session.scroll_offset() > 0,
    }
}

/// Window the live grid into a scene surface, following the app's
/// `terminal_surface`: `rows` are the visible cells top-to-bottom, `first_row`
/// is the absolute index of `rows[0]`, and cursor/selection are absolute.
fn terminal_surface(
    session: &TerminalSession,
    selection: Option<Selection>,
    max_width: u16,
    max_height: u16,
) -> TerminalSurface {
    let grid = session.grid();
    let view_rows = usize::from(grid.size().rows().min(max_height));
    let columns = grid.size().columns().min(max_width);
    let total_rows = grid.total_rows();
    let scrollback_len = grid.scrollback_len();

    let max_top = total_rows.saturating_sub(view_rows);
    let first_row = max_top.saturating_sub(session.scroll_offset());

    let rows = (0..view_rows)
        .map(|line| {
            let absolute_row = first_row + line;
            (0..columns)
                .map(|column| {
                    let cell = grid.history_cell(absolute_row, column).unwrap_or_default();
                    SceneCell {
                        character: cell.character(),
                        style: scene_cell_style(cell.style()),
                    }
                })
                .collect()
        })
        .collect();

    let cursor = grid.cursor();
    TerminalSurface {
        rows,
        first_row,
        cursor: cursor.visible().then(|| {
            SurfacePosition::new(scrollback_len + usize::from(cursor.row()), cursor.column())
        }),
        scroll_offset: session.scroll_offset(),
        scrollback_len,
        selection: selection.map(ordered_span),
        copy_cursor: None,
    }
}

/// Normalize a spike selection (isize rows, unordered) into the scene's ordered
/// inclusive `(start <= end)` span of absolute positions.
fn ordered_span(selection: Selection) -> (SurfacePosition, SurfacePosition) {
    let a = (selection.start_row.max(0) as usize, selection.start_col);
    let b = (selection.end_row.max(0) as usize, selection.end_col);
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (
        SurfacePosition::new(lo.0, lo.1),
        SurfacePosition::new(hi.0, hi.1),
    )
}

fn scene_cell_style(style: CellStyle) -> SceneCellStyle {
    SceneCellStyle {
        foreground: scene_color(style.foreground),
        background: scene_color(style.background),
        bold: style.bold,
        dim: style.dim,
        italic: style.italic,
        underline: style.underline,
        inverse: style.inverse,
        hidden: style.hidden,
        strikethrough: style.strikethrough,
    }
}

fn scene_color(color: VtColor) -> SceneColor {
    match color {
        VtColor::Default => SceneColor::Default,
        VtColor::Indexed(index) if index < 16 => SceneColor::Ansi(index),
        VtColor::Indexed(index) => SceneColor::Indexed(index),
        VtColor::Rgb(red, green, blue) => SceneColor::Rgb(red, green, blue),
    }
}
