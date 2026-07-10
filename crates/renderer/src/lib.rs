//! Ratatui frontend adapter for Mandatum.
//!
//! One entry point: [`render`] draws a [`mandatum_scene::WorkspaceScene`]
//! onto a ratatui frame. This crate computes no layout and never touches the
//! terminal engine or product state — it translates neutral scene types into
//! ratatui widgets, keeping the scene contract the only seam between engine
//! and frontend (L1).

mod pane;
mod surface;

use mandatum_scene::{
    HeaderScene, OverlayScene, PaletteOverlay, SceneRect, WorkspaceScene, layout,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

/// Draw one frame of workspace scene state.
pub fn render(frame: &mut Frame<'_>, scene: &WorkspaceScene) {
    render_header(frame, layout::header_rect(scene.size), &scene.header);
    for pane_scene in &scene.panes {
        pane::render_pane(frame, pane_scene);
    }
    render_status(
        frame,
        layout::status_rect(scene.size),
        scene.status.as_deref(),
    );
    if let Some(OverlayScene::Palette(palette)) = &scene.overlay {
        render_palette(frame, palette);
    }
}

pub(crate) fn to_rect(rect: SceneRect) -> Rect {
    Rect::new(rect.x, rect.y, rect.width, rect.height)
}

fn render_header(frame: &mut Frame<'_>, area: SceneRect, header: &HeaderScene) {
    let zoom = if header.zoomed { " | zoom" } else { "" };
    let title = format!(
        " Mandatum | {} | panes {} | focused {}{} ",
        header.session_name, header.pane_count, header.focused_pane, zoom
    );
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(Color::White).bg(Color::Black)),
        to_rect(area),
    );
}

fn render_status(frame: &mut Frame<'_>, area: SceneRect, status: Option<&str>) {
    let status = status.unwrap_or("ready");
    frame.render_widget(
        Paragraph::new(format!(" {status}")).style(Style::default().fg(Color::Gray)),
        to_rect(area),
    );
}

fn render_palette(frame: &mut Frame<'_>, palette: &PaletteOverlay) {
    let overlay = to_rect(palette.area);
    frame.render_widget(Clear, overlay);

    let rows = palette
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let row = ListItem::new(format!("{}  {}", item.label, item.detail));
            if palette.selected == Some(index) {
                row.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                row
            }
        })
        .collect::<Vec<_>>();
    let list = List::new(rows).block(
        Block::default()
            .title(" Command Palette ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(list, overlay);
}

#[cfg(test)]
mod tests {
    use mandatum_scene::{
        AgentApprovalPrompt, AgentContent, EmptyContent, PaletteEntry, PaneContent, PaneId,
        PaneScene, PaneSceneKind, SceneCell, SceneCellStyle, SceneSize, SurfacePosition,
        TaskContent, TerminalSurface,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn scene(panes: Vec<PaneScene>) -> WorkspaceScene {
        WorkspaceScene {
            size: SceneSize::new(60, 12),
            header: HeaderScene {
                session_name: "main".to_owned(),
                pane_count: panes.len(),
                focused_pane: PaneId::new("pane-1"),
                zoomed: false,
            },
            panes,
            overlay: None,
            status: Some("all good".to_owned()),
            focused_pane: PaneId::new("pane-1"),
            hit_targets: Vec::new(),
            copy_mode: false,
        }
    }

    fn pane(content: PaneContent) -> PaneScene {
        PaneScene {
            id: PaneId::new("pane-1"),
            title: "shell".to_owned(),
            kind: PaneSceneKind::Terminal,
            area: SceneRect::new(0, 1, 40, 10),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content,
        }
    }

    fn text_surface(rows: &[&str]) -> TerminalSurface {
        // Rows padded to a fixed width, as the scene builder produces them.
        TerminalSurface {
            rows: rows
                .iter()
                .map(|row| {
                    (0..4)
                        .map(|column| SceneCell {
                            character: row.chars().nth(column).unwrap_or(' '),
                            style: SceneCellStyle::default(),
                        })
                        .collect()
                })
                .collect(),
            first_row: 0,
            cursor: Some(SurfacePosition::new(1, 2)),
            scroll_offset: 0,
            scrollback_len: 0,
            selection: None,
            copy_cursor: None,
        }
    }

    fn draw(scene: &WorkspaceScene) -> Terminal<TestBackend> {
        let mut terminal =
            Terminal::new(TestBackend::new(scene.size.width, scene.size.height)).unwrap();
        terminal.draw(|frame| render(frame, scene)).unwrap();
        terminal
    }

    fn buffer_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn header_status_and_pane_title_render_scene_fields() {
        let terminal = draw(&scene(vec![pane(PaneContent::Terminal(text_surface(&[
            "sh", "ok",
        ])))]));
        let rows = buffer_rows(&terminal);

        assert!(rows[0].contains("Mandatum | main | panes 1 | focused pane-1"));
        assert!(rows[1].contains("shell | focused"));
        assert!(rows[11].contains("all good"));
    }

    #[test]
    fn zoomed_header_and_default_status_render_fallbacks() {
        let mut zoomed = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        zoomed.header.zoomed = true;
        zoomed.status = None;
        let rows = buffer_rows(&draw(&zoomed));

        assert!(rows[0].contains("| zoom"));
        assert!(rows[11].contains("ready"));
    }

    #[test]
    fn terminal_surface_renders_text_with_cursor_mark() {
        let terminal = draw(&scene(vec![pane(PaneContent::Terminal(text_surface(&[
            "sh", "ok",
        ])))]));
        let rows = buffer_rows(&terminal);
        let buffer = terminal.backend().buffer();

        // Content starts inside the border at (1, 2).
        assert!(rows[2].contains("sh"));
        assert!(rows[3].contains("ok"));
        // Cursor at absolute (1, 2) maps to buffer cell (3, 3).
        assert!(
            buffer
                .cell((3u16, 3u16))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn selection_reverses_cells_and_copy_mode_marks_the_title() {
        let surface = TerminalSurface {
            selection: Some((SurfacePosition::new(0, 0), SurfacePosition::new(0, 1))),
            copy_cursor: Some(SurfacePosition::new(0, 1)),
            ..text_surface(&["sh", "ok"])
        };
        let terminal = draw(&scene(vec![pane(PaneContent::Terminal(surface))]));
        let rows = buffer_rows(&terminal);
        let buffer = terminal.backend().buffer();

        assert!(rows[1].contains("shell | focused | copy"));
        assert!(
            buffer
                .cell((1u16, 2u16))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
        // The live cursor is not drawn while the copy cursor exists.
        assert!(
            !buffer
                .cell((3u16, 3u16))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn task_pane_renders_detail_lines_and_output_surface() {
        let task = pane(PaneContent::Task(TaskContent {
            command: "cargo test".to_owned(),
            cwd_label: "/tmp/project".to_owned(),
            recipe_label: "test".to_owned(),
            status_label: Some("failed: exit 101".to_owned()),
            output: Some(text_surface(&["FAIL"])),
        }));
        let rows = buffer_rows(&draw(&scene(vec![task])));
        let all = rows.join("\n");

        assert!(all.contains("command: cargo test"));
        assert!(all.contains("cwd: /tmp/project"));
        assert!(all.contains("recipe: test"));
        assert!(all.contains("runtime status: failed: exit 101"));
        assert!(all.contains("FAIL"));
    }

    #[test]
    fn waiting_agent_pane_renders_a_distinct_approval_block() {
        let mut agent_pane = pane(PaneContent::Agent(AgentContent {
            objective: "fix the failing test".to_owned(),
            status_label: "waiting for approval".to_owned(),
            pending_approvals: 1,
            changed_file_count: 1,
            changed_files: vec!["src/lib.rs".to_owned()],
            latest_summary: Some("patched".to_owned()),
            current_action: Some("cleaning target".to_owned()),
            pending_approval: Some(AgentApprovalPrompt {
                command: "rm -rf target".to_owned(),
                cwd: "/tmp/project".to_owned(),
                affected_path: Some("target".to_owned()),
                risk_label: "high".to_owned(),
                risk_basis: "removes files (rm)".to_owned(),
                key_hint: "y approve / n reject".to_owned(),
            }),
            output_tail: vec!["$ cargo test".to_owned()],
        }));
        agent_pane.kind = PaneSceneKind::Agent;
        agent_pane.area = mandatum_scene::SceneRect::new(0, 1, 60, 18);
        let mut with_agent = scene(vec![agent_pane]);
        with_agent.size = SceneSize::new(60, 22);
        let terminal = draw(&with_agent);
        let rows = buffer_rows(&terminal);
        let all = rows.join("\n");

        assert!(all.contains("objective: fix the failing test"));
        assert!(all.contains("status: waiting for approval"));
        assert!(all.contains("action: cleaning target"));
        assert!(all.contains("approval required: rm -rf target"));
        assert!(all.contains("risk: high (removes files (rm))"));
        assert!(all.contains("keys: y approve / n reject"));
        // The waiting state is flagged in the pane title.
        assert!(rows[1].contains("approval"));

        // The approval block is visually distinct: its header row is yellow
        // and bold while ordinary detail lines are unstyled.
        let buffer = terminal.backend().buffer();
        let approval_row = (0..buffer.area.height)
            .find(|y| rows[usize::from(*y)].contains("approval required"))
            .expect("approval line rendered");
        let cell = buffer.cell((2u16, approval_row)).unwrap();
        assert_eq!(cell.fg, Color::Yellow);
        assert!(cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn empty_pane_renders_fallback_detail_lines() {
        let empty = pane(PaneContent::Empty(EmptyContent {
            cwd_label: "/tmp/mandatum".to_owned(),
            restart_generation: 1,
        }));
        let rows = buffer_rows(&draw(&scene(vec![empty])));
        let all = rows.join("\n");

        assert!(all.contains("pane-1 terminal"));
        assert!(all.contains("cwd: /tmp/mandatum"));
        assert!(all.contains("restart generation: 1"));
        assert!(all.contains("no live PTY grid is attached"));
    }

    #[test]
    fn palette_overlay_renders_items_over_the_workspace() {
        let mut with_palette = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        with_palette.overlay = Some(OverlayScene::Palette(PaletteOverlay {
            area: layout::palette_overlay_rect(with_palette.size),
            items: vec![
                PaletteEntry::new("Split Right", "layout"),
                PaletteEntry::new("Run Task", "task"),
            ],
            selected: None,
        }));
        let rows = buffer_rows(&draw(&with_palette));
        let all = rows.join("\n");

        assert!(all.contains("Command Palette"));
        assert!(all.contains("Split Right  layout"));
        assert!(all.contains("Run Task  task"));
    }
}
