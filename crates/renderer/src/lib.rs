//! Ratatui frontend adapter for Mandatum.
//!
//! One entry point: [`render`] draws a [`mandatum_scene::WorkspaceScene`]
//! onto a ratatui frame. This crate computes no layout and never touches the
//! terminal engine or product state — it translates neutral scene types into
//! ratatui widgets, keeping the scene contract the only seam between engine
//! and frontend (L1). The scene stays color-semantic; the [`Theme`] resolves
//! each semantic role to a concrete color here in the adapter.

mod overlay;
mod pane;
mod surface;

use mandatum_scene::{
    HeaderScene, OverlayScene, SceneColor, SceneRect, StatusScene, Theme, WorkspaceScene,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Paragraph,
};

/// Draw one frame of workspace scene state with the active theme. The scene
/// carries every strip's area and composed text (`&WorkspaceScene` alone
/// suffices to paint a frame); this adapter only translates to widgets.
pub fn render(frame: &mut Frame<'_>, scene: &WorkspaceScene, theme: &Theme) {
    render_header(frame, &scene.header, theme);
    for pane_scene in &scene.panes {
        pane::render_pane(frame, pane_scene, theme);
    }
    render_status(frame, &scene.status, theme);
    match &scene.overlay {
        Some(OverlayScene::Palette(palette)) => overlay::render_palette(frame, palette, theme),
        Some(OverlayScene::ContextMenu(menu)) => overlay::render_context_menu(frame, menu, theme),
        Some(OverlayScene::Timeline(timeline)) => overlay::render_timeline(frame, timeline, theme),
        Some(OverlayScene::SessionMap(map)) => overlay::render_session_map(frame, map, theme),
        Some(OverlayScene::Prompt(prompt)) => overlay::render_prompt(frame, prompt, theme),
        None => {}
    }
}

pub(crate) fn to_rect(rect: SceneRect) -> Rect {
    Rect::new(rect.x, rect.y, rect.width, rect.height)
}

/// Resolve a theme color to a ratatui color. The standard ANSI range maps to
/// named colors (so themes address the terminal palette the way users
/// expect); everything else passes through directly.
pub(crate) fn theme_color(color: SceneColor) -> Color {
    match color {
        SceneColor::Default => Color::Reset,
        SceneColor::Ansi(0) => Color::Black,
        SceneColor::Ansi(1) => Color::Red,
        SceneColor::Ansi(2) => Color::Green,
        SceneColor::Ansi(3) => Color::Yellow,
        SceneColor::Ansi(4) => Color::Blue,
        SceneColor::Ansi(5) => Color::Magenta,
        SceneColor::Ansi(6) => Color::Cyan,
        SceneColor::Ansi(7) => Color::Gray,
        SceneColor::Ansi(8) => Color::DarkGray,
        SceneColor::Ansi(9) => Color::LightRed,
        SceneColor::Ansi(10) => Color::LightGreen,
        SceneColor::Ansi(11) => Color::LightYellow,
        SceneColor::Ansi(12) => Color::LightBlue,
        SceneColor::Ansi(13) => Color::LightMagenta,
        SceneColor::Ansi(14) => Color::LightCyan,
        SceneColor::Ansi(15) => Color::White,
        SceneColor::Ansi(index) | SceneColor::Indexed(index) => Color::Indexed(index),
        SceneColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}

/// A foreground style for a theme color, leaving `Default` unstyled.
pub(crate) fn theme_fg(color: SceneColor) -> Style {
    match color {
        SceneColor::Default => Style::default(),
        color => Style::default().fg(theme_color(color)),
    }
}

/// Paint the attention strip: the scene's composed text, then each
/// attention segment restyled in the theme's attention color at the rect
/// the scene resolved for it.
fn render_header(frame: &mut Frame<'_>, header: &HeaderScene, theme: &Theme) {
    if header.area.is_empty() {
        return;
    }
    let base = Style::default()
        .fg(theme_color(theme.header))
        .bg(theme_color(theme.header_background));
    frame.render_widget(
        Paragraph::new(header.text.clone()).style(base),
        to_rect(header.area),
    );
    for segment in &header.attention {
        if segment.rect.is_empty() {
            continue;
        }
        frame.render_widget(
            Paragraph::new(segment.label.clone()).style(
                Style::default()
                    .fg(theme_color(theme.attention))
                    .bg(theme_color(theme.header_background))
                    .add_modifier(Modifier::BOLD),
            ),
            to_rect(segment.rect),
        );
    }
}

fn render_status(frame: &mut Frame<'_>, status: &StatusScene, theme: &Theme) {
    if status.area.is_empty() {
        return;
    }
    frame.render_widget(
        Paragraph::new(format!(" {}", status.text)).style(theme_fg(theme.status)),
        to_rect(status.area),
    );
}

#[cfg(test)]
mod tests {
    use mandatum_scene::{
        AgentApprovalPrompt, AgentContent, AgentStatus, AttentionSegment, ContextMenuEntry,
        ContextMenuOverlay, EmptyContent, PaletteEntry, PaletteOverlay, PaneContent, PaneId,
        PaneScene, PaneSceneKind, PromptOverlay, SceneCell, SceneCellStyle, SceneSize,
        SessionMapOverlay, SessionMapRow, SurfacePosition, TaskContent, TerminalSurface,
        TimelineEntry, TimelineOverlay, layout,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn scene(panes: Vec<PaneScene>) -> WorkspaceScene {
        let pane_count = panes.len();
        WorkspaceScene {
            size: SceneSize::new(60, 12),
            header: header(&format!(
                " Mandatum | main · {pane_count} pane(s) · agent: fake"
            )),
            panes,
            overlay: None,
            status: StatusScene {
                area: SceneRect::new(0, 11, 60, 1),
                text: "all good".to_owned(),
            },
            focused_pane: PaneId::new("pane-1"),
            hit_targets: Vec::new(),
            copy_mode: false,
        }
    }

    fn header(text: &str) -> HeaderScene {
        HeaderScene {
            area: SceneRect::new(0, 0, 60, 1),
            workspace_name: "Mandatum".to_owned(),
            session_name: "main".to_owned(),
            pane_count: 1,
            focused_pane: PaneId::new("pane-1"),
            zoomed: false,
            connector_label: "fake".to_owned(),
            text: text.to_owned(),
            attention: Vec::new(),
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
        draw_with_theme(scene, &Theme::default())
    }

    fn draw_with_theme(scene: &WorkspaceScene, theme: &Theme) -> Terminal<TestBackend> {
        let mut terminal =
            Terminal::new(TestBackend::new(scene.size.width, scene.size.height)).unwrap();
        terminal.draw(|frame| render(frame, scene, theme)).unwrap();
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

        // The strips paint the scene's composed text verbatim at the
        // scene's areas: nothing is derived in the frontend.
        assert!(rows[0].contains("Mandatum | main · 1 pane(s) · agent: fake"));
        assert!(rows[1].contains("shell | focused"));
        assert!(rows[11].contains("all good"));
    }

    #[test]
    fn attention_segments_restyle_the_header_at_their_scene_rects() {
        let mut with_attention = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        let text = " Mandatum | 1 approval waiting · pane-2";
        let label = "1 approval waiting · pane-2";
        let start = (text.chars().count() - label.chars().count()) as u16;
        with_attention.header.text = text.to_owned();
        with_attention.header.attention = vec![AttentionSegment {
            rect: SceneRect::new(start, 0, label.chars().count() as u16, 1),
            label: label.to_owned(),
            pane: Some(PaneId::new("pane-2")),
        }];
        let terminal = draw(&with_attention);
        let rows = buffer_rows(&terminal);
        let buffer = terminal.backend().buffer();

        assert!(rows[0].contains(label));
        // The segment takes the theme's attention color (yellow in
        // mandatum-dark) and bold; the base text keeps the header color.
        let segment_cell = buffer.cell((start, 0u16)).unwrap();
        assert_eq!(segment_cell.fg, Color::Yellow);
        assert!(segment_cell.modifier.contains(Modifier::BOLD));
        let base_cell = buffer.cell((1u16, 0u16)).unwrap();
        assert_eq!(base_cell.fg, Color::White);
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
            status_role: AgentStatus::WaitingForApproval,
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
        // The scene carries the status area; keep it on the bottom row of
        // the resized frame.
        with_agent.status.area = SceneRect::new(0, 21, 60, 1);
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
    fn theme_resolves_semantic_roles_to_different_concrete_colors() {
        let workspace = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);

        // The focused pane border takes the theme's focus color: yellow in
        // mandatum-dark, blue in mandatum-light.
        let dark = draw_with_theme(&workspace, &Theme::default());
        let dark_border = dark.backend().buffer().cell((0u16, 1u16)).unwrap().fg;
        assert_eq!(dark_border, Color::Yellow);

        let light = draw_with_theme(&workspace, &Theme::builtin("mandatum-light").unwrap());
        let light_border = light.backend().buffer().cell((0u16, 1u16)).unwrap().fg;
        assert_eq!(light_border, Color::Blue);

        // Inline overrides land on the drawn cells too.
        let custom = Theme {
            focus_border: mandatum_scene::SceneColor::Rgb(10, 20, 30),
            ..Theme::default()
        };
        let overridden = draw_with_theme(&workspace, &custom);
        assert_eq!(
            overridden.backend().buffer().cell((0u16, 1u16)).unwrap().fg,
            Color::Rgb(10, 20, 30)
        );
    }

    #[test]
    fn agent_status_line_takes_the_status_role_color() {
        let mut agent_pane = pane(PaneContent::Agent(AgentContent {
            objective: "fix the failing test".to_owned(),
            status_label: "running".to_owned(),
            status_role: AgentStatus::Running,
            pending_approvals: 0,
            changed_file_count: 0,
            changed_files: Vec::new(),
            latest_summary: None,
            current_action: None,
            pending_approval: None,
            output_tail: Vec::new(),
        }));
        agent_pane.kind = PaneSceneKind::Agent;
        let terminal = draw(&scene(vec![agent_pane]));
        let rows = buffer_rows(&terminal);
        let buffer = terminal.backend().buffer();

        let status_row = (0..buffer.area.height)
            .find(|y| rows[usize::from(*y)].contains("status: running"))
            .expect("status line rendered");
        let cell = buffer.cell((2u16, status_row)).unwrap();
        assert_eq!(cell.fg, Color::Green);
        assert!(cell.modifier.contains(Modifier::BOLD));
    }

    fn palette_scene(
        query: &str,
        items: Vec<PaletteEntry>,
        selected: Option<usize>,
    ) -> WorkspaceScene {
        let mut with_palette = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        with_palette.size = SceneSize::new(80, 20);
        with_palette.overlay = Some(OverlayScene::Palette(PaletteOverlay {
            area: layout::palette_overlay_rect(with_palette.size),
            query: query.to_owned(),
            items,
            selected,
            footer: "type to search · enter run · esc close".to_owned(),
        }));
        with_palette
    }

    #[test]
    fn timeline_overlay_renders_entries_times_filter_and_footer() {
        let mut with_timeline = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        with_timeline.size = SceneSize::new(90, 24);
        with_timeline.overlay = Some(OverlayScene::Timeline(TimelineOverlay {
            area: layout::timeline_overlay_rect(with_timeline.size),
            query: "task".to_owned(),
            items: vec![
                TimelineEntry {
                    glyph: "✗".to_owned(),
                    when: "2m ago".to_owned(),
                    text: "task pane-2 failed: exit 3: sh ./flaky-check.sh".to_owned(),
                    pane: Some(PaneId::new("pane-2")),
                },
                TimelineEntry {
                    glyph: "▶".to_owned(),
                    when: "3m ago".to_owned(),
                    text: "task pane-2 started: sh ./flaky-check.sh".to_owned(),
                    pane: Some(PaneId::new("pane-2")),
                },
            ],
            selected: Some(0),
            skipped_malformed: 1,
            footer: "enter jump · esc close · 1 malformed line(s) skipped".to_owned(),
        }));
        let terminal = draw(&with_timeline);
        let all = buffer_rows(&terminal).join("\n");

        assert!(all.contains("Timeline"));
        assert!(all.contains("> task"));
        assert!(all.contains("✗"));
        assert!(all.contains("2m ago"));
        assert!(all.contains("failed: exit 3"));
        assert!(all.contains("1 malformed line(s) skipped"));
    }

    #[test]
    fn session_map_overlay_renders_the_tree_with_states_and_badges() {
        let mut with_map = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        with_map.size = SceneSize::new(90, 24);
        with_map.overlay = Some(OverlayScene::SessionMap(SessionMapOverlay {
            area: layout::session_map_rect(with_map.size),
            rows: vec![
                SessionMapRow {
                    depth: 0,
                    glyph: "▸".to_owned(),
                    label: "session-1 · main · 2 pane(s) (active)".to_owned(),
                    state: String::new(),
                    focused: false,
                    badges: String::new(),
                },
                SessionMapRow {
                    depth: 1,
                    glyph: "❯".to_owned(),
                    label: "pane-1 shell".to_owned(),
                    state: "running".to_owned(),
                    focused: true,
                    badges: "zoom".to_owned(),
                },
            ],
            selected: 1,
            footer: "↑/↓ move · enter focus · esc close".to_owned(),
        }));
        let terminal = draw(&with_map);
        let all = buffer_rows(&terminal).join("\n");

        assert!(all.contains("Sessions"));
        assert!(all.contains("session-1 · main · 2 pane(s) (active)"));
        assert!(all.contains("●  ❯ pane-1 shell"));
        assert!(all.contains("running"));
        assert!(all.contains("[zoom]"));
        assert!(all.contains("enter focus"));
    }

    #[test]
    fn prompt_overlay_renders_title_input_and_footer() {
        let mut with_prompt = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        with_prompt.size = SceneSize::new(90, 24);
        with_prompt.overlay = Some(OverlayScene::Prompt(PromptOverlay {
            area: layout::prompt_rect(with_prompt.size),
            title: " Set agent objective — pane-3 ".to_owned(),
            input: "review the failing tests".to_owned(),
            footer: "enter save · esc cancel".to_owned(),
        }));
        let terminal = draw(&with_prompt);
        let all = buffer_rows(&terminal).join("\n");

        assert!(all.contains("Set agent objective — pane-3"));
        assert!(all.contains("> review the failing tests"));
        assert!(all.contains("enter save"));
    }

    #[test]
    fn context_menu_renders_rows_selection_and_right_aligned_hints() {
        let mut with_menu = scene(vec![pane(PaneContent::Terminal(text_surface(&["sh"])))]);
        with_menu.overlay = Some(OverlayScene::ContextMenu(ContextMenuOverlay {
            area: SceneRect::new(10, 2, 26, 5),
            items: vec![
                ContextMenuEntry::new("Zoom pane", "ctrl+p z"),
                ContextMenuEntry::new("Close pane", "ctrl+p x"),
                ContextMenuEntry::new("Copy selection", ""),
            ],
            selected: 1,
        }));
        let terminal = draw(&with_menu);
        let rows = buffer_rows(&terminal);
        let buffer = terminal.backend().buffer();

        // Rows render inside the border with their hints right-aligned.
        assert!(rows[3].contains("Zoom pane"));
        assert!(rows[4].contains("Close pane"));
        assert!(rows[5].contains("Copy selection"));
        // Columns are char positions, not byte offsets (border glyphs are
        // multibyte).
        let hint_byte = rows[3].rfind("ctrl+p z").expect("hint rendered");
        let hint_end = rows[3][..hint_byte].chars().count() + "ctrl+p z".chars().count();
        let inner_right = 10 + 26 - 2; // one border column + one padding cell
        assert_eq!(hint_end as u16, inner_right);

        // The selected row is reversed; unselected rows are not.
        let selected_cell = buffer.cell((12u16, 4u16)).unwrap();
        assert!(selected_cell.modifier.contains(Modifier::REVERSED));
        let unselected_cell = buffer.cell((12u16, 3u16)).unwrap();
        assert!(!unselected_cell.modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn palette_overlay_renders_query_items_hints_and_footer() {
        let mut split = PaletteEntry::new("Split pane right", "layout");
        split.key_hint = Some("v".to_owned());
        let with_palette = palette_scene(
            "spl",
            vec![split, PaletteEntry::new("Run task", "task")],
            Some(0),
        );
        let rows = buffer_rows(&draw(&with_palette));
        let all = rows.join("\n");

        assert!(all.contains("Command Palette"));
        assert!(all.contains("> spl"));
        assert!(all.contains("Split pane right  v  layout"));
        assert!(all.contains("Run task  task"));
        assert!(all.contains("type to search · enter run · esc close"));
    }

    #[test]
    fn palette_marks_matches_selection_and_greyed_entries() {
        let mut split = PaletteEntry::new("Split pane right", "layout");
        split.match_indices = vec![0, 1, 2];
        let mut stop = PaletteEntry::new("Stop task", "task is not running");
        stop.enabled = false;
        let with_palette = palette_scene("spl", vec![split, stop], Some(0));
        let terminal = draw(&with_palette);
        let rows = buffer_rows(&terminal);
        let buffer = terminal.backend().buffer();

        let find_row = |needle: &str| {
            (0..buffer.area.height)
                .find(|y| rows[usize::from(*y)].contains(needle))
                .unwrap_or_else(|| panic!("row containing {needle:?} rendered"))
        };

        // Cell columns are char positions, not byte offsets (the border
        // glyphs are multibyte).
        let char_column = |row: u16, needle: char| {
            rows[usize::from(row)]
                .chars()
                .position(|character| character == needle)
                .unwrap() as u16
        };

        // Matched label chars are bold+underlined; unmatched chars are not.
        let split_row = find_row("Split pane right");
        let label_start = char_column(split_row, 'S');
        let matched = buffer.cell((label_start, split_row)).unwrap();
        assert!(matched.modifier.contains(Modifier::BOLD));
        assert!(matched.modifier.contains(Modifier::UNDERLINED));
        let unmatched = buffer.cell((label_start + 4, split_row)).unwrap();
        assert!(!unmatched.modifier.contains(Modifier::BOLD));

        // The selected row is reversed.
        assert!(matched.modifier.contains(Modifier::REVERSED));

        // A greyed entry renders dim, with its reason as the detail text.
        let stop_row = find_row("Stop task");
        assert!(rows[usize::from(stop_row)].contains("task is not running"));
        let greyed = buffer.cell((char_column(stop_row, 'S'), stop_row)).unwrap();
        assert!(greyed.modifier.contains(Modifier::DIM));
        assert!(!greyed.modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn empty_palette_input_placeholder_names_the_fast_path_and_its_escape() {
        let with_palette = palette_scene("", vec![PaletteEntry::new("Run task", "task")], Some(0));
        let rows = buffer_rows(&draw(&with_palette));
        let all = rows.join("\n");

        assert!(all.contains("letters run their key"));
        assert!(all.contains("shift+letter to search"));
    }

    #[test]
    fn palette_with_no_matches_says_so_and_keeps_the_footer() {
        let with_palette = palette_scene("zzz", Vec::new(), None);
        let rows = buffer_rows(&draw(&with_palette));
        let all = rows.join("\n");

        assert!(all.contains("no matching commands"));
        assert!(all.contains("type to search"));
    }
}
