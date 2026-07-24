use std::sync::Arc;

use mandatum_scene::{
    AgentApprovalPrompt, AgentContent, AgentStatus, ArtifactContent, ArtifactFit, ArtifactState,
    AttentionSegment, CellOccupancy, CellSelection, ContextMenuEntry, ContextMenuOverlay,
    EmptyContent, HeaderScene, HelpEntry, HelpOverlay, HitTarget, HitTargetKind, OverlayScene,
    PaletteEntry, PaletteOverlay, PaneContent, PaneId, PaneScene, PaneSceneKind, PromptOverlay,
    RasterSurface, SceneCell, SceneCellStyle, SceneColor, SceneRect, SceneSize, SearchEntry,
    SearchOverlay, SessionMapOverlay, SessionMapRow, StatusScene, SurfacePosition, TaskContent,
    TerminalSurface, Theme, TimelineEntry, TimelineOverlay, WelcomeEntry, WelcomeOverlay,
    WorkspaceScene, compile_cell_program,
};

#[test]
fn whole_frame_cell_program_preserves_terminal_cell_style_selection_and_copy_cursor() {
    let pane_id = PaneId::new("pane-1");
    let style = SceneCellStyle {
        foreground: SceneColor::Rgb(1, 2, 3),
        background: SceneColor::Indexed(233),
        bold: true,
        dim: true,
        italic: true,
        underline: true,
        inverse: true,
        hidden: true,
        strikethrough: true,
    };
    let scene = WorkspaceScene {
        size: SceneSize::new(6, 5),
        header: HeaderScene {
            area: SceneRect::new(0, 0, 6, 1),
            workspace_name: "Mandatum".to_owned(),
            session_name: "main".to_owned(),
            pane_count: 1,
            focused_pane: pane_id.clone(),
            zoomed: false,
            connector_label: "none".to_owned(),
            text: " head ".to_owned(),
            attention: Vec::new(),
        },
        panes: vec![PaneScene {
            id: pane_id.clone(),
            title: "shell".to_owned(),
            kind: PaneSceneKind::Terminal,
            area: SceneRect::new(0, 1, 6, 3),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content: PaneContent::Terminal(TerminalSurface {
                rows: vec![vec![SceneCell {
                    character: 'X',
                    style,
                }]],
                first_row: 0,
                cursor: Some(SurfacePosition::new(0, 0)),
                scroll_offset: 0,
                scrollback_len: 0,
                selection: Some((SurfacePosition::new(0, 0), SurfacePosition::new(0, 0))),
                copy_cursor: Some(SurfacePosition::new(0, 0)),
            }),
        }],
        overlay: None,
        status: StatusScene {
            area: SceneRect::new(0, 4, 6, 1),
            text: "ready".to_owned(),
        },
        focused_pane: pane_id,
        hit_targets: Vec::new(),
        copy_mode: true,
    };

    let program = compile_cell_program(&scene, &Theme::default());
    let terminal_cell = program
        .cell_at(1, 2)
        .expect("pane-inner terminal cell is present in the whole-frame program");

    assert_eq!(terminal_cell.occupancy, CellOccupancy::Glyph('X'));
    assert_ne!(
        terminal_cell.occupancy,
        CellOccupancy::WideContinuation,
        "ordinary terminal glyphs stay distinct from explicit continuation cells"
    );
    assert_eq!(terminal_cell.style, style);
    assert_eq!(
        terminal_cell.selection,
        Some(CellSelection::Terminal),
        "selection kind is the sole renderer-neutral selection contract"
    );
    assert!(terminal_cell.cursor);
}

#[test]
fn mixed_scene_compiles_semantic_chrome_content_and_later_pane_opacity() {
    let theme = Theme {
        name: "cell-program-tracer".to_owned(),
        focus_title: SceneColor::Rgb(10, 11, 12),
        pane_border: SceneColor::Rgb(20, 21, 22),
        pane_title: SceneColor::Rgb(30, 31, 32),
        header: SceneColor::Rgb(40, 41, 42),
        header_background: SceneColor::Rgb(50, 51, 52),
        status: SceneColor::Rgb(60, 61, 62),
        attention: SceneColor::Rgb(70, 71, 72),
        palette_border: SceneColor::Rgb(80, 81, 82),
        overlay_foreground: SceneColor::Rgb(90, 91, 92),
        overlay_background: SceneColor::Rgb(100, 101, 102),
        palette_selection: SceneColor::Rgb(110, 111, 112),
        selection_highlight: SceneColor::Rgb(120, 121, 122),
        agent_running: SceneColor::Rgb(130, 131, 132),
        agent_waiting: SceneColor::Rgb(140, 141, 142),
        agent_failed: SceneColor::Rgb(150, 151, 152),
        agent_complete: SceneColor::Rgb(160, 161, 162),
        agent_idle: SceneColor::Rgb(170, 171, 172),
    };
    let task_id = PaneId::new("task-pane");
    let agent_id = PaneId::new("agent-pane");
    let empty_id = PaneId::new("empty-pane");
    let scene = WorkspaceScene {
        size: SceneSize::new(100, 30),
        header: HeaderScene {
            area: SceneRect::new(0, 0, 100, 1),
            workspace_name: "Mandatum".to_owned(),
            session_name: "main".to_owned(),
            pane_count: 3,
            focused_pane: task_id.clone(),
            zoomed: false,
            connector_label: "fake".to_owned(),
            text: " Mandatum | approval waiting".to_owned(),
            attention: vec![AttentionSegment {
                rect: SceneRect::new(12, 0, 8, 1),
                label: "approval".to_owned(),
                pane: Some(agent_id.clone()),
            }],
        },
        panes: vec![
            PaneScene {
                id: task_id.clone(),
                title: "task".to_owned(),
                kind: PaneSceneKind::Task,
                area: SceneRect::new(0, 1, 40, 12),
                focused: true,
                floating: false,
                stacked: false,
                zoomed: false,
                content: PaneContent::Task(TaskContent {
                    command: "cargo test".to_owned(),
                    cwd_label: "/project".to_owned(),
                    recipe_label: Some("checks".to_owned()),
                    status_label: Some("failed: exit 3".to_owned()),
                    rerun_hint: Some("ctrl+p r".to_owned()),
                    output: None,
                }),
            },
            PaneScene {
                id: agent_id.clone(),
                title: "agent".to_owned(),
                kind: PaneSceneKind::Agent,
                area: SceneRect::new(40, 1, 60, 20),
                focused: false,
                floating: false,
                stacked: false,
                zoomed: false,
                content: PaneContent::Agent(AgentContent {
                    objective: "repair tests".to_owned(),
                    status_label: "waiting for approval".to_owned(),
                    status_role: AgentStatus::WaitingForApproval,
                    pending_approvals: 1,
                    changed_file_count: 0,
                    changed_files: Vec::new(),
                    latest_summary: None,
                    current_action: None,
                    last_error: None,
                    relaunch_hint: None,
                    pending_approval: Some(AgentApprovalPrompt {
                        command: "rm target".to_owned(),
                        cwd: "/project".to_owned(),
                        affected_path: None,
                        risk_label: "medium".to_owned(),
                        risk_basis: "removes build output".to_owned(),
                        key_hint: "y approve / n reject".to_owned(),
                        pulse_on: true,
                    }),
                    output_tail: Vec::new(),
                }),
            },
            // This later pane overlaps both earlier panes. Its opaque blank
            // interior must replace earlier agent glyphs as part of the same
            // scene-ordered cell program.
            PaneScene {
                id: empty_id,
                title: "empty".to_owned(),
                kind: PaneSceneKind::StatusLog,
                area: SceneRect::new(20, 8, 30, 10),
                focused: false,
                floating: true,
                stacked: false,
                zoomed: false,
                content: PaneContent::Empty(EmptyContent {
                    cwd_label: "/e".to_owned(),
                    restart_generation: 2,
                }),
            },
        ],
        overlay: None,
        status: StatusScene {
            area: SceneRect::new(0, 29, 100, 1),
            text: "ready".to_owned(),
        },
        focused_pane: task_id,
        hit_targets: Vec::new(),
        copy_mode: false,
    };

    let program = compile_cell_program(&scene, &theme);

    let header = program.cell_at(1, 0).expect("header base cell");
    assert_eq!(header.occupancy, CellOccupancy::Glyph('M'));
    assert_eq!(
        header.style,
        SceneCellStyle {
            foreground: theme.header,
            background: theme.header_background,
            ..SceneCellStyle::default()
        }
    );

    let attention = program.cell_at(12, 0).expect("attention segment cell");
    assert_eq!(attention.occupancy, CellOccupancy::Glyph('a'));
    assert_eq!(
        attention.style,
        SceneCellStyle {
            foreground: theme.attention,
            background: theme.header_background,
            bold: true,
            ..SceneCellStyle::default()
        }
    );

    let border = program.cell_at(0, 2).expect("task pane border cell");
    assert_eq!(border.occupancy, CellOccupancy::Glyph('│'));
    assert_eq!(border.style.foreground, theme.pane_border);

    let focused_suffix = program.cell_at(9, 1).expect("focused title suffix");
    assert_eq!(focused_suffix.occupancy, CellOccupancy::Glyph('f'));
    assert_eq!(focused_suffix.style.foreground, theme.focus_title);
    assert!(focused_suffix.style.bold);

    let failed_status = program.cell_at(1, 5).expect("failed task status row");
    assert_eq!(failed_status.occupancy, CellOccupancy::Glyph('r'));
    assert_eq!(failed_status.style.foreground, theme.attention);
    assert!(failed_status.style.bold);

    let agent_status = program.cell_at(41, 3).expect("agent status row");
    assert_eq!(agent_status.occupancy, CellOccupancy::Glyph('s'));
    assert_eq!(agent_status.style.foreground, theme.agent_waiting);
    assert!(agent_status.style.bold);

    let approval_header = program.cell_at(41, 6).expect("approval header row");
    assert_eq!(approval_header.occupancy, CellOccupancy::Glyph('a'));
    assert_eq!(approval_header.style.foreground, theme.attention);
    assert!(approval_header.style.bold, "pulse-on emphasizes the header");

    let approval_scope = program.cell_at(41, 7).expect("approval scope row");
    assert_eq!(approval_scope.occupancy, CellOccupancy::Glyph('s'));
    assert_eq!(approval_scope.style.foreground, theme.attention);
    assert!(
        !approval_scope.style.bold,
        "only the pulsing header is bold"
    );

    let empty_detail = program.cell_at(21, 9).expect("Empty detail row");
    assert_eq!(empty_detail.occupancy, CellOccupancy::Glyph('c'));

    let status = program.cell_at(0, 29).expect("status leading cell");
    assert_eq!(status.occupancy, CellOccupancy::Glyph(' '));
    assert_eq!(status.style.foreground, theme.status);

    let opaque_blank = program
        .cell_at(45, 9)
        .expect("later pane owns every covered interior cell");
    assert_eq!(
        opaque_blank.occupancy,
        CellOccupancy::Glyph(' '),
        "the later pane clears the earlier agent glyph at this cell"
    );
}

#[test]
fn palette_compiles_one_opaque_styled_cell_program_aligned_with_item_targets() {
    let theme = Theme {
        name: "palette-cell-program-tracer".to_owned(),
        palette_border: SceneColor::Rgb(10, 20, 30),
        overlay_foreground: SceneColor::Rgb(40, 50, 60),
        overlay_background: SceneColor::Rgb(70, 80, 90),
        palette_selection: SceneColor::Rgb(100, 110, 120),
        ..Theme::default()
    };
    let pane_id = PaneId::new("pane-1");
    let overlay_area = SceneRect::new(10, 4, 40, 10);
    let item_target = HitTarget {
        rect: SceneRect::new(11, 6, 38, 1),
        kind: HitTargetKind::PaletteItem(0),
    };
    let scene = WorkspaceScene {
        size: SceneSize::new(60, 20),
        header: HeaderScene {
            area: SceneRect::new(0, 0, 60, 1),
            workspace_name: "Mandatum".to_owned(),
            session_name: "main".to_owned(),
            pane_count: 1,
            focused_pane: pane_id.clone(),
            zoomed: false,
            connector_label: "none".to_owned(),
            text: " Mandatum".to_owned(),
            attention: Vec::new(),
        },
        panes: vec![PaneScene {
            id: pane_id.clone(),
            title: "shell".to_owned(),
            kind: PaneSceneKind::Terminal,
            area: SceneRect::new(0, 1, 60, 18),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content: PaneContent::Terminal(TerminalSurface {
                rows: vec![
                    vec![
                        SceneCell {
                            character: 'X',
                            style: SceneCellStyle::default(),
                        };
                        58
                    ];
                    16
                ],
                first_row: 0,
                cursor: None,
                scroll_offset: 0,
                scrollback_len: 0,
                selection: None,
                copy_cursor: None,
            }),
        }],
        overlay: Some(OverlayScene::Palette(PaletteOverlay {
            area: overlay_area,
            query: String::new(),
            items: vec![
                PaletteEntry {
                    label: "Split pane".to_owned(),
                    detail: "layout".to_owned(),
                    key_hint: Some("v".to_owned()),
                    match_indices: vec![0, 1],
                    enabled: true,
                },
                PaletteEntry {
                    label: "Stop task".to_owned(),
                    detail: "task is not running".to_owned(),
                    key_hint: Some("x".to_owned()),
                    match_indices: Vec::new(),
                    enabled: false,
                },
            ],
            selected: Some(0),
            footer: "enter run · esc close".to_owned(),
        })),
        status: StatusScene {
            area: SceneRect::new(0, 19, 60, 1),
            text: "ready".to_owned(),
        },
        focused_pane: pane_id,
        hit_targets: vec![item_target.clone()],
        copy_mode: false,
    };

    let program = compile_cell_program(&scene, &theme);

    let opaque_surface = program.cell_at(48, 11).expect("opaque palette cell");
    assert_eq!(opaque_surface.occupancy, CellOccupancy::Glyph(' '));
    assert_eq!(
        opaque_surface.style,
        SceneCellStyle {
            foreground: theme.overlay_foreground,
            background: theme.overlay_background,
            ..SceneCellStyle::default()
        },
        "the overlay surface replaces the pane glyph beneath it"
    );

    let border = program.cell_at(10, 5).expect("palette border cell");
    assert_eq!(border.occupancy, CellOccupancy::Glyph('│'));
    assert_eq!(border.style.foreground, theme.palette_border);
    assert_eq!(border.style.background, theme.overlay_background);

    let title = program.cell_at(12, 4).expect("palette title cell");
    assert_eq!(title.occupancy, CellOccupancy::Glyph('C'));
    assert_eq!(title.style.foreground, theme.overlay_foreground);
    assert_eq!(title.style.background, theme.overlay_background);

    let placeholder = program.cell_at(13, 5).expect("empty-query placeholder");
    assert_eq!(placeholder.occupancy, CellOccupancy::Glyph('l'));
    assert!(placeholder.style.dim);
    assert!(!placeholder.cursor);

    assert_eq!(item_target.rect, SceneRect::new(11, 6, 38, 1));
    let matched = program
        .cell_at(item_target.rect.x + 1, item_target.rect.y)
        .expect("matched label cell aligned with PaletteItem target");
    assert_eq!(matched.occupancy, CellOccupancy::Glyph('S'));
    assert!(matched.style.bold);
    assert!(matched.style.underline);
    assert!(matched.style.inverse);
    assert_eq!(matched.style.foreground, theme.palette_selection);
    assert_eq!(matched.selection, Some(CellSelection::Item));

    let key_hint = program.cell_at(24, 6).expect("palette key hint");
    assert_eq!(key_hint.occupancy, CellOccupancy::Glyph('v'));
    assert!(key_hint.style.dim);

    let detail = program.cell_at(27, 6).expect("palette detail");
    assert_eq!(detail.occupancy, CellOccupancy::Glyph('l'));
    assert!(detail.style.dim);

    let disabled = program.cell_at(12, 7).expect("disabled palette row");
    assert_eq!(disabled.occupancy, CellOccupancy::Glyph('S'));
    assert!(disabled.style.dim);
    assert_eq!(disabled.selection, None);

    let footer = program.cell_at(12, 12).expect("palette footer");
    assert_eq!(footer.occupancy, CellOccupancy::Glyph('e'));
    assert!(footer.style.dim);

    let mut typed_scene = scene;
    let Some(OverlayScene::Palette(palette)) = typed_scene.overlay.as_mut() else {
        unreachable!("fixture contains a palette")
    };
    palette.query = "sp".to_owned();
    let typed_program = compile_cell_program(&typed_scene, &theme);
    let query_cursor = typed_program
        .cell_at(15, 5)
        .expect("non-empty query cursor cell");
    assert_eq!(query_cursor.occupancy, CellOccupancy::Glyph(' '));
    assert!(query_cursor.cursor);
}

fn remaining_overlay_theme() -> Theme {
    Theme {
        name: "remaining-overlay-cell-program".to_owned(),
        palette_border: SceneColor::Rgb(10, 20, 30),
        overlay_foreground: SceneColor::Rgb(40, 50, 60),
        overlay_background: SceneColor::Rgb(70, 80, 90),
        palette_selection: SceneColor::Rgb(100, 110, 120),
        ..Theme::default()
    }
}

fn scene_with_overlay(overlay: OverlayScene, hit_targets: Vec<HitTarget>) -> WorkspaceScene {
    let pane_id = PaneId::new("pane-1");
    WorkspaceScene {
        size: SceneSize::new(60, 20),
        header: HeaderScene {
            area: SceneRect::new(0, 0, 60, 1),
            workspace_name: "Mandatum".to_owned(),
            session_name: "main".to_owned(),
            pane_count: 1,
            focused_pane: pane_id.clone(),
            zoomed: false,
            connector_label: "none".to_owned(),
            text: " Mandatum".to_owned(),
            attention: Vec::new(),
        },
        panes: vec![PaneScene {
            id: pane_id.clone(),
            title: "shell".to_owned(),
            kind: PaneSceneKind::Terminal,
            area: SceneRect::new(0, 1, 60, 18),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content: PaneContent::Terminal(TerminalSurface {
                rows: vec![
                    vec![
                        SceneCell {
                            character: 'X',
                            style: SceneCellStyle::default(),
                        };
                        58
                    ];
                    16
                ],
                first_row: 0,
                cursor: None,
                scroll_offset: 0,
                scrollback_len: 0,
                selection: None,
                copy_cursor: None,
            }),
        }],
        overlay: Some(overlay),
        status: StatusScene {
            area: SceneRect::new(0, 19, 60, 1),
            text: "ready".to_owned(),
        },
        focused_pane: pane_id,
        hit_targets,
        copy_mode: false,
    }
}

#[test]
fn every_remaining_overlay_variant_uses_the_shared_opaque_shell() {
    let area = SceneRect::new(5, 3, 50, 12);
    let overlays = vec![
        OverlayScene::ContextMenu(ContextMenuOverlay {
            area,
            items: Vec::new(),
            selected: 0,
        }),
        OverlayScene::Timeline(TimelineOverlay {
            area,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            skipped_malformed: 0,
            footer: String::new(),
        }),
        OverlayScene::Search(SearchOverlay {
            area,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            overflow: 0,
            footer: String::new(),
        }),
        OverlayScene::SessionMap(SessionMapOverlay {
            area,
            rows: Vec::new(),
            selected: 0,
            footer: String::new(),
        }),
        OverlayScene::Prompt(PromptOverlay {
            area,
            title: " Prompt ".to_owned(),
            input: String::new(),
            footer: String::new(),
        }),
        OverlayScene::Help(HelpOverlay {
            area,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            footer: String::new(),
        }),
        OverlayScene::Welcome(WelcomeOverlay {
            area,
            introduction: String::new(),
            entries: Vec::new(),
            dismissal: String::new(),
        }),
    ];
    let theme = remaining_overlay_theme();
    for overlay in overlays {
        let program = compile_cell_program(&scene_with_overlay(overlay, Vec::new()), &theme);
        let blank = program.cell_at(53, 12).expect("opaque overlay blank");
        assert_eq!(blank.occupancy, CellOccupancy::Glyph(' '));
        assert_eq!(blank.style.foreground, theme.overlay_foreground);
        assert_eq!(blank.style.background, theme.overlay_background);
        let border = program.cell_at(5, 4).expect("shared overlay border");
        assert_eq!(border.occupancy, CellOccupancy::Glyph('│'));
        assert_eq!(border.style.foreground, theme.palette_border);
        assert_eq!(border.style.background, theme.overlay_background);
    }
}

#[test]
fn list_overlays_preserve_rows_styles_and_hit_target_alignment() {
    let area = SceneRect::new(5, 3, 50, 12);
    let theme = remaining_overlay_theme();

    let context_target = HitTarget {
        rect: SceneRect::new(6, 4, 48, 1),
        kind: HitTargetKind::ContextMenuItem(0),
    };
    let context = scene_with_overlay(
        OverlayScene::ContextMenu(ContextMenuOverlay {
            area,
            items: vec![ContextMenuEntry::new("Open", "ctrl+o")],
            selected: 0,
        }),
        vec![context_target.clone()],
    );
    let program = compile_cell_program(&context, &theme);
    let label = program
        .cell_at(context_target.rect.x + 1, context_target.rect.y)
        .expect("context label inside its hit target");
    assert_eq!(label.occupancy, CellOccupancy::Glyph('O'));
    assert_eq!(label.selection, Some(CellSelection::Item));
    assert!(label.style.inverse);
    let chord = program.cell_at(47, 4).expect("right-aligned context chord");
    assert_eq!(chord.occupancy, CellOccupancy::Glyph('c'));
    assert!(chord.style.dim);

    let timeline_target = HitTarget {
        rect: SceneRect::new(6, 5, 48, 1),
        kind: HitTargetKind::TimelineItem(0),
    };
    let timeline = scene_with_overlay(
        OverlayScene::Timeline(TimelineOverlay {
            area,
            query: "build".to_owned(),
            items: vec![TimelineEntry {
                glyph: "▶".to_owned(),
                when: "2m ago".to_owned(),
                text: "built".to_owned(),
                pane: None,
            }],
            selected: Some(0),
            skipped_malformed: 0,
            footer: "enter jump · esc close".to_owned(),
        }),
        vec![timeline_target.clone()],
    );
    let program = compile_cell_program(&timeline, &theme);
    let glyph = program
        .cell_at(timeline_target.rect.x + 1, timeline_target.rect.y)
        .expect("timeline glyph inside its hit target");
    assert_eq!(glyph.occupancy, CellOccupancy::Glyph('▶'));
    assert_eq!(glyph.selection, Some(CellSelection::Item));
    let timestamp = program.cell_at(13, 5).expect("timeline timestamp");
    assert_eq!(timestamp.occupancy, CellOccupancy::Glyph('2'));
    assert!(timestamp.style.dim);
    assert!(
        program
            .cell_at(13, 4)
            .expect("timeline query cursor")
            .cursor
    );
    assert!(program.cell_at(7, 13).expect("timeline footer").style.dim);

    let search_target = HitTarget {
        rect: SceneRect::new(6, 6, 48, 1),
        kind: HitTargetKind::SearchItem(1),
    };
    let search = scene_with_overlay(
        OverlayScene::Search(SearchOverlay {
            area,
            query: "er".to_owned(),
            items: vec![
                SearchEntry {
                    source: "shell".to_owned(),
                    text: "ERROR".to_owned(),
                    match_indices: vec![0, 1],
                    pane: None,
                },
                SearchEntry {
                    source: "shell".to_owned(),
                    text: "OK".to_owned(),
                    match_indices: vec![0],
                    pane: None,
                },
            ],
            selected: Some(1),
            overflow: 0,
            footer: "enter jump · esc close".to_owned(),
        }),
        vec![search_target.clone()],
    );
    let program = compile_cell_program(&search, &theme);
    let source = program.cell_at(7, 5).expect("first grouped source");
    assert_eq!(source.occupancy, CellOccupancy::Glyph('s'));
    assert!(source.style.dim);
    let elided_source = program.cell_at(7, 6).expect("repeated source elision");
    assert_eq!(elided_source.occupancy, CellOccupancy::Glyph(' '));
    let matched = program
        .cell_at(14, search_target.rect.y)
        .expect("matched result inside its hit target");
    assert_eq!(matched.occupancy, CellOccupancy::Glyph('O'));
    assert!(matched.style.bold);
    assert!(matched.style.underline);
    assert_eq!(matched.selection, Some(CellSelection::Item));

    let map_target = HitTarget {
        rect: SceneRect::new(6, 5, 48, 1),
        kind: HitTargetKind::SessionMapRow(1),
    };
    let map = scene_with_overlay(
        OverlayScene::SessionMap(SessionMapOverlay {
            area,
            rows: vec![
                SessionMapRow {
                    depth: 0,
                    glyph: "◇".to_owned(),
                    label: "main".to_owned(),
                    state: String::new(),
                    focused: false,
                    badges: String::new(),
                },
                SessionMapRow {
                    depth: 1,
                    glyph: ">".to_owned(),
                    label: "shell".to_owned(),
                    state: "running".to_owned(),
                    focused: true,
                    badges: "zoom".to_owned(),
                },
            ],
            selected: 1,
            footer: "enter focus · esc close".to_owned(),
        }),
        vec![map_target.clone()],
    );
    let program = compile_cell_program(&map, &theme);
    let focus = program
        .cell_at(map_target.rect.x, map_target.rect.y)
        .expect("focused map row inside its hit target");
    assert_eq!(focus.occupancy, CellOccupancy::Glyph('●'));
    assert_eq!(focus.selection, Some(CellSelection::Item));
    let state = program.cell_at(18, 5).expect("session-map state");
    assert_eq!(state.occupancy, CellOccupancy::Glyph('r'));
    assert!(state.style.dim);
    assert!(program.cell_at(7, 13).expect("map footer").style.dim);
}

#[test]
fn prompt_help_and_welcome_preserve_input_hierarchy_and_footer() {
    let area = SceneRect::new(5, 3, 50, 12);
    let theme = remaining_overlay_theme();

    let prompt = scene_with_overlay(
        OverlayScene::Prompt(PromptOverlay {
            area,
            title: " Objective ".to_owned(),
            input: "fix".to_owned(),
            footer: "enter save · esc cancel".to_owned(),
        }),
        Vec::new(),
    );
    let program = compile_cell_program(&prompt, &theme);
    assert_eq!(
        program.cell_at(7, 3).expect("prompt title").occupancy,
        CellOccupancy::Glyph('O')
    );
    assert_eq!(
        program.cell_at(8, 4).expect("prompt input").occupancy,
        CellOccupancy::Glyph('f')
    );
    assert!(program.cell_at(11, 4).expect("prompt cursor").cursor);
    assert!(program.cell_at(7, 13).expect("prompt footer").style.dim);

    let help = scene_with_overlay(
        OverlayScene::Help(HelpOverlay {
            area,
            query: "sp".to_owned(),
            items: vec![
                HelpEntry {
                    heading: true,
                    label: "Layout".to_owned(),
                    keys: String::new(),
                },
                HelpEntry {
                    heading: false,
                    label: "Split".to_owned(),
                    keys: "ctrl+s".to_owned(),
                },
            ],
            selected: Some(1),
            footer: "type to filter · esc close".to_owned(),
        }),
        Vec::new(),
    );
    let program = compile_cell_program(&help, &theme);
    assert!(program.cell_at(10, 4).expect("help query cursor").cursor);
    let heading = program.cell_at(7, 5).expect("help heading");
    assert_eq!(heading.occupancy, CellOccupancy::Glyph('L'));
    assert!(heading.style.bold);
    let entry = program.cell_at(9, 6).expect("help entry");
    assert_eq!(entry.occupancy, CellOccupancy::Glyph('S'));
    assert_eq!(entry.selection, Some(CellSelection::Item));
    let keys = program.cell_at(16, 6).expect("help key hint");
    assert_eq!(keys.occupancy, CellOccupancy::Glyph('c'));
    assert!(keys.style.dim);
    assert!(program.cell_at(7, 13).expect("help footer").style.dim);

    let welcome = scene_with_overlay(
        OverlayScene::Welcome(WelcomeOverlay {
            area,
            introduction: "Welcome".to_owned(),
            entries: vec![
                WelcomeEntry {
                    keys: "F1".to_owned(),
                    description: "Help".to_owned(),
                },
                WelcomeEntry {
                    keys: "Ctrl+P".to_owned(),
                    description: "Commands".to_owned(),
                },
            ],
            dismissal: "Press any key".to_owned(),
        }),
        Vec::new(),
    );
    let program = compile_cell_program(&welcome, &theme);
    let intro = program.cell_at(6, 4).expect("welcome introduction");
    assert_eq!(intro.occupancy, CellOccupancy::Glyph('W'));
    assert!(intro.style.bold);
    let key = program.cell_at(8, 6).expect("welcome key");
    assert_eq!(key.occupancy, CellOccupancy::Glyph('F'));
    assert_eq!(key.style.foreground, theme.palette_border);
    assert!(key.style.bold);
    assert_eq!(
        program
            .cell_at(16, 6)
            .expect("welcome description")
            .occupancy,
        CellOccupancy::Glyph('H')
    );
    let dismissal = program.cell_at(6, 9).expect("welcome dismissal");
    assert_eq!(dismissal.occupancy, CellOccupancy::Glyph('P'));
    assert!(dismissal.style.dim);
}

#[test]
fn huge_chrome_and_overlay_rectangles_only_emit_in_frame_cells() {
    let overlay = OverlayScene::Welcome(WelcomeOverlay {
        area: SceneRect::new(2, 1, u16::MAX, u16::MAX),
        introduction: "Welcome".to_owned(),
        entries: Vec::new(),
        dismissal: String::new(),
    });
    let mut scene = scene_with_overlay(overlay, Vec::new());
    scene.size = SceneSize::new(4, 3);
    scene.header.area = SceneRect::new(0, 0, u16::MAX, u16::MAX);
    scene.header.text = "head".to_owned();
    scene.header.attention.clear();
    scene.panes.clear();
    scene.status.area = SceneRect::new(500, 500, u16::MAX, u16::MAX);

    let program = compile_cell_program(&scene, &Theme::default());
    let emitted = program.cells().collect::<Vec<_>>();

    assert!(
        emitted
            .iter()
            .all(|(x, y, _)| *x < scene.size.width && *y < scene.size.height)
    );
    assert!(
        emitted.len() <= usize::from(scene.size.width * scene.size.height) * 4,
        "raw rectangle area must not determine compiler work: emitted {} cells",
        emitted.len()
    );
    assert_eq!(
        program
            .cell_at(3, 2)
            .expect("clipped overlay content")
            .occupancy,
        CellOccupancy::Glyph('W')
    );
}

#[test]
fn narrow_pane_content_never_overwrites_or_escapes_its_border() {
    for (width, height) in [(1, 6), (2, 6), (6, 1), (6, 2)] {
        let pane_id = PaneId::new(format!("pane-{width}x{height}"));
        let area = SceneRect::new(2, 2, width, height);
        let scene = WorkspaceScene {
            size: SceneSize::new(12, 12),
            header: HeaderScene {
                area: SceneRect::new(0, 0, 0, 0),
                workspace_name: "Mandatum".to_owned(),
                session_name: "main".to_owned(),
                pane_count: 1,
                focused_pane: pane_id.clone(),
                zoomed: false,
                connector_label: "none".to_owned(),
                text: String::new(),
                attention: Vec::new(),
            },
            panes: vec![PaneScene {
                id: pane_id.clone(),
                title: "narrow".to_owned(),
                kind: PaneSceneKind::Terminal,
                area,
                focused: false,
                floating: false,
                stacked: false,
                zoomed: false,
                content: PaneContent::Terminal(TerminalSurface {
                    rows: vec![
                        vec![
                            SceneCell {
                                character: 'X',
                                style: SceneCellStyle::default(),
                            };
                            8
                        ];
                        8
                    ],
                    first_row: 0,
                    cursor: None,
                    scroll_offset: 0,
                    scrollback_len: 0,
                    selection: None,
                    copy_cursor: None,
                }),
            }],
            overlay: None,
            status: StatusScene {
                area: SceneRect::new(0, 0, 0, 0),
                text: String::new(),
            },
            focused_pane: pane_id,
            hit_targets: Vec::new(),
            copy_mode: false,
        };

        let program = compile_cell_program(&scene, &Theme::default());
        assert!(
            program.cells().all(|(x, y, cell)| {
                area.contains(x, y) && cell.occupancy != CellOccupancy::Glyph('X')
            }),
            "{width}x{height} pane content must remain behind its border"
        );
    }
}

#[test]
fn many_full_frame_replacements_compact_to_final_topmost_cells() {
    let overlay = OverlayScene::Welcome(WelcomeOverlay {
        area: SceneRect::new(0, 0, 8, 6),
        introduction: "Final owner".to_owned(),
        entries: Vec::new(),
        dismissal: String::new(),
    });
    let mut scene = scene_with_overlay(overlay, Vec::new());
    scene.size = SceneSize::new(8, 6);
    scene.header.area = SceneRect::new(0, 0, 8, 6);
    scene.status.area = SceneRect::new(0, 0, 8, 6);
    let mut pane = scene.panes[0].clone();
    pane.area = SceneRect::new(0, 0, 8, 6);
    scene.panes = vec![pane; 128];

    let program = compile_cell_program(&scene, &remaining_overlay_theme());

    assert_eq!(
        program.cells().count(),
        usize::from(scene.size.width * scene.size.height),
        "overlap replacements must not remain in final storage"
    );
    assert_eq!(
        program
            .cell_at(1, 1)
            .expect("final overlay owner")
            .occupancy,
        CellOccupancy::Glyph('F')
    );
    assert_eq!(
        program.cells().map(|(x, y, _)| (x, y)).collect::<Vec<_>>(),
        (0..scene.size.height)
            .flat_map(|y| (0..scene.size.width).map(move |x| (x, y)))
            .collect::<Vec<_>>(),
        "final cells iterate deterministically in row-major order"
    );
}

#[test]
fn ready_artifact_marks_only_its_final_visible_body_cells() {
    let artifact_id = PaneId::new("artifact-pane");
    let later_id = PaneId::new("later-pane");
    let scene = WorkspaceScene {
        size: SceneSize::new(20, 11),
        header: HeaderScene {
            area: SceneRect::new(0, 0, 20, 1),
            workspace_name: "Mandatum".to_owned(),
            session_name: "main".to_owned(),
            pane_count: 2,
            focused_pane: artifact_id.clone(),
            zoomed: false,
            connector_label: "none".to_owned(),
            text: "artifacts".to_owned(),
            attention: Vec::new(),
        },
        panes: vec![
            PaneScene {
                id: artifact_id.clone(),
                title: "Home".to_owned(),
                kind: PaneSceneKind::Artifact,
                area: SceneRect::new(0, 1, 20, 9),
                focused: true,
                floating: false,
                stacked: false,
                zoomed: false,
                content: PaneContent::Artifact(ArtifactContent {
                    source_label: "shots/home.png".to_owned(),
                    alt_text: "Home page".to_owned(),
                    fit: ArtifactFit::Contain,
                    state: ArtifactState::Ready(RasterSurface {
                        width: 2,
                        height: 1,
                        revision: 7,
                        rgba8: Arc::from([255, 0, 0, 255, 0, 255, 0, 255]),
                    }),
                }),
            },
            PaneScene {
                id: later_id.clone(),
                title: "later".to_owned(),
                kind: PaneSceneKind::StatusLog,
                area: SceneRect::new(10, 5, 8, 4),
                focused: false,
                floating: true,
                stacked: false,
                zoomed: false,
                content: PaneContent::Empty(EmptyContent {
                    cwd_label: "/tmp".to_owned(),
                    restart_generation: 0,
                }),
            },
        ],
        overlay: Some(OverlayScene::Welcome(WelcomeOverlay {
            area: SceneRect::new(1, 6, 6, 3),
            introduction: "topmost".to_owned(),
            entries: Vec::new(),
            dismissal: String::new(),
        })),
        status: StatusScene {
            area: SceneRect::new(0, 10, 20, 1),
            text: "ready".to_owned(),
        },
        focused_pane: artifact_id,
        hit_targets: Vec::new(),
        copy_mode: false,
    };

    let program = compile_cell_program(&scene, &Theme::default());

    assert_eq!(
        program.cell_at(2, 5).and_then(|cell| cell.raster_layer),
        Some(0),
        "ready body cells carry the artifact pane draw index"
    );
    assert_eq!(
        program.cell_at(1, 2).and_then(|cell| cell.raster_layer),
        None,
        "stable labeled detail rows stay cell-only"
    );
    assert_eq!(
        program.cell_at(11, 6).and_then(|cell| cell.raster_layer),
        None,
        "a later pane replaces an earlier artifact marker"
    );
    assert_eq!(
        program.cell_at(2, 6).and_then(|cell| cell.raster_layer),
        None,
        "the topmost overlay replaces an artifact marker"
    );
}

#[test]
fn every_degenerate_overlay_keeps_content_inside_its_true_border() {
    for (width, height) in [(1, 6), (2, 6), (6, 1), (6, 2)] {
        let area = SceneRect::new(2, 2, width, height);
        let overlays = vec![
            OverlayScene::Palette(PaletteOverlay {
                area,
                query: "X".to_owned(),
                items: Vec::new(),
                selected: None,
                footer: "X".to_owned(),
            }),
            OverlayScene::ContextMenu(ContextMenuOverlay {
                area,
                items: vec![ContextMenuEntry::new("X", "X")],
                selected: 0,
            }),
            OverlayScene::Timeline(TimelineOverlay {
                area,
                query: "X".to_owned(),
                items: vec![TimelineEntry {
                    glyph: "X".to_owned(),
                    when: "X".to_owned(),
                    text: "X".to_owned(),
                    pane: None,
                }],
                selected: Some(0),
                skipped_malformed: 0,
                footer: "X".to_owned(),
            }),
            OverlayScene::Search(SearchOverlay {
                area,
                query: "X".to_owned(),
                items: vec![SearchEntry {
                    source: "X".to_owned(),
                    text: "X".to_owned(),
                    match_indices: vec![0],
                    pane: None,
                }],
                selected: Some(0),
                overflow: 0,
                footer: "X".to_owned(),
            }),
            OverlayScene::SessionMap(SessionMapOverlay {
                area,
                rows: vec![SessionMapRow {
                    depth: 0,
                    glyph: "X".to_owned(),
                    label: "X".to_owned(),
                    state: "X".to_owned(),
                    focused: true,
                    badges: "X".to_owned(),
                }],
                selected: 0,
                footer: "X".to_owned(),
            }),
            OverlayScene::Prompt(PromptOverlay {
                area,
                title: " X ".to_owned(),
                input: "X".to_owned(),
                footer: "X".to_owned(),
            }),
            OverlayScene::Help(HelpOverlay {
                area,
                query: "X".to_owned(),
                items: vec![HelpEntry {
                    heading: false,
                    label: "X".to_owned(),
                    keys: "X".to_owned(),
                }],
                selected: Some(0),
                footer: "X".to_owned(),
            }),
            OverlayScene::Welcome(WelcomeOverlay {
                area,
                introduction: "X".to_owned(),
                entries: vec![WelcomeEntry {
                    keys: "X".to_owned(),
                    description: "X".to_owned(),
                }],
                dismissal: "X".to_owned(),
            }),
        ];

        for overlay in overlays {
            let mut scene = scene_with_overlay(overlay, Vec::new());
            scene.header.area = SceneRect::new(0, 0, 0, 0);
            scene.panes.clear();
            scene.status.area = SceneRect::new(0, 0, 0, 0);
            let program = compile_cell_program(&scene, &Theme::default());

            assert!(
                program.cells().all(|(x, y, _)| area.contains(x, y)),
                "{width}x{height} overlay content escaped its border"
            );
            if width <= 2 && height > 2 {
                assert_eq!(
                    program
                        .cell_at(area.x, area.y + 1)
                        .expect("left border")
                        .occupancy,
                    CellOccupancy::Glyph('│')
                );
                if width == 2 {
                    assert_eq!(
                        program
                            .cell_at(area.x + 1, area.y + 1)
                            .expect("right border")
                            .occupancy,
                        CellOccupancy::Glyph('│')
                    );
                }
            }
            if height == 2 && width > 2 {
                assert_eq!(
                    program
                        .cell_at(area.x + 1, area.y + 1)
                        .expect("bottom border")
                        .occupancy,
                    CellOccupancy::Glyph('─')
                );
            }
        }
    }
}
