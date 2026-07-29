use mandatum_native_renderer::{
    MAX_NATIVE_PLAN_NODES, NativeFontFace, NativeMaterialRole, NativePlanCommand,
    NativePresentationPlanError, NativeTextMetricRole, NativeTokenColorRole,
    prepare_native_presentation, prepare_scene, prepare_token_sampler,
};
use mandatum_scene::{
    AttentionKind, BackingScale, EmptyContent, HeaderScene, LogicalRect, LogicalSize, OverlayKind,
    OverlayNodePart, OverlayPresentationKind, PaneBadgeKind, PaneContent, PaneId, PaneNodePart,
    PaneScene, PaneSceneKind, PhysicalSize, PresentationNode, PresentationNodeId,
    PresentationNodeRole, PresentationNodeState, PresentationTone, ScenePresentation, SceneRect,
    SceneSize, SemanticKey, StatusScene, TerminalProjection, Theme, TransitionProperty,
    TransitionRole, TransitionTarget, UiColor, ViewportMetrics, WorkflowNodePart, WorkflowRowRole,
    WorkspaceNodePart, WorkspaceScene, compile_cell_program,
};

fn scene(nodes: Vec<PresentationNode>, transitions: Vec<TransitionTarget>) -> WorkspaceScene {
    let pane_id = PaneId::new("pane-a");
    WorkspaceScene {
        size: SceneSize::new(100, 30),
        header: HeaderScene {
            area: SceneRect::new(0, 0, 100, 1),
            workspace_name: "test".into(),
            project_name: "project".into(),
            session_name: "session".into(),
            pane_count: 1,
            focused_pane: pane_id.clone(),
            zoomed: false,
            connector_label: "none".into(),
            text: "test".into(),
            attention: Vec::new(),
        },
        panes: vec![PaneScene {
            content_revision: 0,
            id: pane_id.clone(),
            title: "shell".into(),
            kind: PaneSceneKind::Terminal,
            area: SceneRect::new(0, 1, 100, 28),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content: PaneContent::Empty(EmptyContent {
                cwd_label: ".".into(),
                restart_generation: 0,
            }),
        }],
        overlay: None,
        status: StatusScene {
            area: SceneRect::new(0, 29, 100, 1),
            text: "ready".into(),
        },
        focused_pane: pane_id,
        hit_targets: Vec::new(),
        copy_mode: false,
        text_input: None,
        presentation: ScenePresentation {
            viewport: Some(
                ViewportMetrics::new(
                    LogicalSize::from_units(800 * 64, 600 * 64),
                    PhysicalSize::new(1_600, 1_200),
                    BackingScale::new(2.0).unwrap(),
                    LogicalSize::from_units(8 * 64, 20 * 64),
                )
                .unwrap(),
            ),
            nodes,
            transition_targets: transitions,
            ..ScenePresentation::default()
        },
    }
}

fn node(
    id: PresentationNodeId,
    parent: Option<PresentationNodeId>,
    role: PresentationNodeRole,
    state: PresentationNodeState,
    logical_rect: LogicalRect,
    cell_rect: SceneRect,
) -> PresentationNode {
    PresentationNode {
        id,
        parent,
        role,
        state,
        logical_rect,
        cell_rect: Some(cell_rect),
        terminal_projection: TerminalProjection::CellRegions(vec![cell_rect]),
    }
}

#[test]
fn plan_retains_exact_semantic_order_bounds_clips_state_and_typed_transition() {
    let workspace_id = PresentationNodeId::workspace(WorkspaceNodePart::Surface);
    let pane_id = PresentationNodeId::pane(PaneId::new("pane-a"), PaneNodePart::Surface);
    let title_id = PresentationNodeId::pane(PaneId::new("pane-a"), PaneNodePart::Title);
    let workspace_rect = LogicalRect::from_units(0, 0, 800 * 64, 600 * 64);
    let pane_rect = LogicalRect::from_units(0, 20 * 64, 800 * 64, 560 * 64);
    let title_rect = LogicalRect::from_units(8 * 64, 24 * 64, 200 * 64, 17 * 64);
    let scene = scene(
        vec![
            node(
                workspace_id.clone(),
                None,
                PresentationNodeRole::Workspace,
                PresentationNodeState::default(),
                workspace_rect,
                SceneRect::new(0, 0, 100, 30),
            ),
            node(
                pane_id.clone(),
                Some(workspace_id.clone()),
                PresentationNodeRole::Pane,
                PresentationNodeState {
                    focused: true,
                    ..PresentationNodeState::default()
                },
                pane_rect,
                SceneRect::new(0, 1, 100, 28),
            ),
            node(
                title_id.clone(),
                Some(pane_id.clone()),
                PresentationNodeRole::PaneTitle,
                PresentationNodeState {
                    focused: true,
                    ..PresentationNodeState::default()
                },
                title_rect,
                SceneRect::new(1, 1, 25, 1),
            ),
        ],
        vec![TransitionTarget {
            node_id: title_id.clone(),
            role: TransitionRole::Overlay,
            property: TransitionProperty::Opacity,
            sequence: 0,
        }],
    );

    let theme = Theme::default();
    let plan = prepare_native_presentation(&scene, &theme).unwrap();
    assert_eq!(plan.material_count(), 3);
    assert_eq!(plan.text_scope_count(), 1);

    let z_orders = plan
        .commands()
        .iter()
        .map(|command| match command {
            NativePlanCommand::BeginClip { z_order, .. }
            | NativePlanCommand::EndClip { z_order, .. } => *z_order,
            NativePlanCommand::Material(material) => material.z_order,
            NativePlanCommand::Text(text) => text.z_order,
        })
        .collect::<Vec<_>>();
    assert!(z_orders.windows(2).all(|pair| pair[0] < pair[1]));

    assert!(matches!(
        &plan.commands()[0],
        NativePlanCommand::BeginClip {
            node_id,
            clip,
            z_order: 0
        } if node_id == &workspace_id && *clip == workspace_rect
    ));
    assert!(matches!(
        &plan.commands()[1],
        NativePlanCommand::Material(material)
            if material.role == NativeMaterialRole::Canvas
                && material.logical_rect == workspace_rect
                && material.clip == workspace_rect
                && material.color == theme.ui.palette.canvas
    ));
    let title = plan
        .commands()
        .iter()
        .find_map(|command| match command {
            NativePlanCommand::Text(text) => Some(text),
            _ => None,
        })
        .unwrap();
    assert_eq!(title.node_id, title_id);
    assert_eq!(title.logical_rect, title_rect);
    assert_eq!(title.clip, title_rect);
    assert_eq!(title.metrics.role, NativeTextMetricRole::PaneTitleFocused);
    assert_eq!(title.metrics.style.face, NativeFontFace::Bold);
    assert_eq!(title.color, theme.ui.palette.focus);
    let title_material = plan
        .commands()
        .iter()
        .find_map(|command| match command {
            NativePlanCommand::Material(material) if material.node_id == title_id => Some(material),
            _ => None,
        })
        .expect("pane title rail receives native chrome material");
    assert_eq!(title_material.role, NativeMaterialRole::ChromeSurface);
    assert_eq!(title_material.color, theme.ui.palette.chrome_surface);
    assert_eq!(title_material.corner_radius_units, 0);
    assert_eq!(title_material.boundary, None);
    assert_eq!(title_material.raised_shadows, None);

    assert_eq!(plan.transitions().len(), 1);
    assert_eq!(plan.transitions()[0].node_id, title_id);
    assert_eq!(plan.transitions()[0].role, TransitionRole::Overlay);
    assert_eq!(plan.transitions()[0].property, TransitionProperty::Opacity);
    assert_eq!(plan.transitions()[0].timing, theme.ui.motion.overlay_enter);
    assert_eq!(
        plan.transitions()[0].exit_timing,
        theme.ui.motion.overlay_exit
    );

    let prepared = prepare_scene(&scene, &theme).unwrap();
    assert_eq!(prepared.presentation_plan(), &plan);
    assert_eq!(
        prepared.cell_program(),
        &compile_cell_program(&scene, &theme),
        "native planning must not rewrite the terminal-parity projection"
    );
}

#[test]
fn phase_three_material_family_maps_tiled_floating_focus_separator_and_tone_state() {
    let workspace_id = PresentationNodeId::workspace(WorkspaceNodePart::Surface);
    let tiled_id = PresentationNodeId::pane(PaneId::new("tiled"), PaneNodePart::Surface);
    let floating_id = PresentationNodeId::pane(PaneId::new("floating"), PaneNodePart::Surface);
    let floating_title_id = PresentationNodeId::pane(PaneId::new("floating"), PaneNodePart::Title);
    let badge_id = PresentationNodeId::pane(
        PaneId::new("floating"),
        PaneNodePart::Badge(PaneBadgeKind::Agent),
    );
    let focus_id = PresentationNodeId::pane(PaneId::new("floating"), PaneNodePart::FocusIndicator);
    let separator_idle_id = PresentationNodeId::workspace(WorkspaceNodePart::Separator {
        split_index: 0,
        axis: mandatum_scene::PresentationAxis::Horizontal,
    });
    let separator_hover_id = PresentationNodeId::workspace(WorkspaceNodePart::Separator {
        split_index: 1,
        axis: mandatum_scene::PresentationAxis::Horizontal,
    });
    let separator_drag_id = PresentationNodeId::workspace(WorkspaceNodePart::Separator {
        split_index: 2,
        axis: mandatum_scene::PresentationAxis::Horizontal,
    });
    let attention_id = PresentationNodeId::workspace(WorkspaceNodePart::Attention {
        pane: None,
        kind: AttentionKind::ApprovalWaiting,
    });
    let workspace_rect = LogicalRect::from_units(0, 0, 800 * 64, 600 * 64);
    let scene = scene(
        vec![
            node(
                workspace_id.clone(),
                None,
                PresentationNodeRole::Workspace,
                PresentationNodeState::default(),
                workspace_rect,
                SceneRect::new(0, 0, 100, 30),
            ),
            node(
                tiled_id.clone(),
                Some(workspace_id.clone()),
                PresentationNodeRole::Pane,
                PresentationNodeState::default(),
                LogicalRect::from_units(0, 20 * 64, 400 * 64, 560 * 64),
                SceneRect::new(0, 1, 50, 28),
            ),
            node(
                floating_id.clone(),
                Some(workspace_id.clone()),
                PresentationNodeRole::Pane,
                PresentationNodeState {
                    floating: true,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(120 * 64, 100 * 64, 400 * 64, 300 * 64),
                SceneRect::new(15, 5, 50, 15),
            ),
            node(
                floating_title_id.clone(),
                Some(floating_id.clone()),
                PresentationNodeRole::PaneTitle,
                PresentationNodeState {
                    floating: true,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(120 * 64, 100 * 64, 400 * 64, 17 * 64),
                SceneRect::new(15, 5, 50, 1),
            ),
            node(
                badge_id.clone(),
                Some(floating_id.clone()),
                PresentationNodeRole::PaneBadge(PaneBadgeKind::Agent),
                PresentationNodeState {
                    tone: PresentationTone::AgentIdentity,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(400 * 64, 104 * 64, 80 * 64, 17 * 64),
                SceneRect::new(50, 5, 10, 1),
            ),
            node(
                focus_id.clone(),
                Some(floating_id.clone()),
                PresentationNodeRole::FocusIndicator,
                PresentationNodeState {
                    focused: true,
                    tone: PresentationTone::Focus,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(120 * 64, 100 * 64, 2 * 64, 17 * 64),
                SceneRect::new(15, 5, 1, 1),
            ),
            node(
                separator_idle_id.clone(),
                Some(workspace_id.clone()),
                PresentationNodeRole::Separator,
                PresentationNodeState::default(),
                LogicalRect::from_units(399 * 64, 20 * 64, 64, 560 * 64),
                SceneRect::new(49, 1, 2, 28),
            ),
            node(
                separator_hover_id.clone(),
                Some(workspace_id.clone()),
                PresentationNodeRole::Separator,
                PresentationNodeState {
                    hovered: true,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(499 * 64, 20 * 64, 64, 560 * 64),
                SceneRect::new(62, 1, 2, 28),
            ),
            node(
                separator_drag_id.clone(),
                Some(workspace_id),
                PresentationNodeRole::Separator,
                PresentationNodeState {
                    hovered: true,
                    dragging: true,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(599 * 64, 20 * 64, 64, 560 * 64),
                SceneRect::new(74, 1, 2, 28),
            ),
            node(
                attention_id.clone(),
                Some(PresentationNodeId::workspace(WorkspaceNodePart::Surface)),
                PresentationNodeRole::Attention,
                PresentationNodeState {
                    attention: true,
                    tone: PresentationTone::Waiting,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(600 * 64, 0, 160 * 64, 20 * 64),
                SceneRect::new(75, 0, 20, 1),
            ),
        ],
        Vec::new(),
    );
    let theme = Theme::default();
    let plan = prepare_native_presentation(&scene, &theme).unwrap();
    let material = |id: &PresentationNodeId| {
        plan.commands()
            .iter()
            .find_map(|command| match command {
                NativePlanCommand::Material(material) if &material.node_id == id => Some(material),
                _ => None,
            })
            .expect("node has a material")
    };

    let tiled = material(&tiled_id);
    assert_eq!(tiled.corner_radius_units, 0);
    assert_eq!(tiled.boundary, None);
    assert_eq!(tiled.raised_shadows, None);

    let floating = material(&floating_id);
    assert_eq!(
        floating.corner_radius_units,
        u64::from(theme.ui.radii.floating) * 64
    );
    assert_eq!(
        floating.boundary.unwrap().color,
        theme.ui.palette.border_strong
    );
    assert_eq!(floating.raised_shadows, Some(theme.ui.elevation.raised));
    assert_eq!(floating.clip, workspace_rect);
    assert!(
        plan.commands().iter().all(|command| !matches!(
            command,
            NativePlanCommand::Material(material) if material.node_id == floating_title_id
        )),
        "a rectangular title fill must not overwrite the floating shell's rounded top corners"
    );
    let floating_title_text = plan
        .commands()
        .iter()
        .find_map(|command| match command {
            NativePlanCommand::Text(text) if text.node_id == floating_title_id => Some(text),
            _ => None,
        })
        .expect("floating title keeps its native text scope");
    assert_eq!(
        floating_title_text.cell_rect,
        Some(SceneRect::new(15, 5, 50, 1))
    );

    let identity = theme.ui.palette.agent_identity;
    assert_eq!(material(&badge_id).role, NativeMaterialRole::Badge);
    assert_eq!(
        material(&badge_id).color,
        UiColor::rgba(identity.red, identity.green, identity.blue, 16),
        "chips fill with a low-alpha tone tint over the rail"
    );
    assert_eq!(
        material(&badge_id).boundary,
        None,
        "chips carry no outline stroke"
    );
    let badge_text = plan
        .commands()
        .iter()
        .find_map(|command| match command {
            NativePlanCommand::Text(text) if text.node_id == badge_id => Some(text),
            _ => None,
        })
        .expect("badge receives a typed glyph scope");
    assert_eq!(badge_text.color, theme.ui.palette.agent_identity);
    assert_ne!(material(&badge_id).color, badge_text.color);
    let waiting = theme.ui.palette.waiting;
    assert_eq!(
        material(&attention_id).color,
        UiColor::rgba(waiting.red, waiting.green, waiting.blue, 16)
    );
    assert_eq!(material(&attention_id).boundary, None);
    let attention_text = plan
        .commands()
        .iter()
        .find_map(|command| match command {
            NativePlanCommand::Text(text) if text.node_id == attention_id => Some(text),
            _ => None,
        })
        .expect("attention chip receives a typed glyph scope");
    assert_eq!(attention_text.color, theme.ui.palette.waiting);
    assert_ne!(material(&attention_id).color, attention_text.color);
    assert_eq!(material(&focus_id).role, NativeMaterialRole::Focus);
    assert_eq!(material(&focus_id).color, theme.ui.palette.focus);
    assert_eq!(
        material(&separator_idle_id).role,
        NativeMaterialRole::BorderSubtle
    );
    assert_eq!(
        material(&separator_hover_id).role,
        NativeMaterialRole::BorderStrong
    );
    assert_eq!(material(&separator_drag_id).role, NativeMaterialRole::Focus);
}

#[test]
fn phase_four_overlay_family() {
    fn overlay_node(
        kind: OverlayKind,
        part: OverlayNodePart,
        parent: &PresentationNodeId,
        role: PresentationNodeRole,
        state: PresentationNodeState,
        logical_rect: LogicalRect,
        cell_rect: SceneRect,
    ) -> PresentationNode {
        node(
            PresentationNodeId::overlay(kind, part),
            Some(parent.clone()),
            role,
            state,
            logical_rect,
            cell_rect,
        )
    }

    fn overlay_surface(
        kind: OverlayKind,
        treatment: OverlayPresentationKind,
        workspace_id: &PresentationNodeId,
        logical_rect: LogicalRect,
        cell_rect: SceneRect,
    ) -> PresentationNode {
        node(
            PresentationNodeId::overlay(kind, OverlayNodePart::Surface),
            Some(workspace_id.clone()),
            PresentationNodeRole::Overlay,
            PresentationNodeState {
                overlay_kind: Some(treatment),
                ..PresentationNodeState::default()
            },
            logical_rect,
            cell_rect,
        )
    }

    fn materials_for<'a>(
        plan: &'a mandatum_native_renderer::NativePresentationPlan,
        id: &PresentationNodeId,
    ) -> Vec<&'a mandatum_native_renderer::NativeMaterial> {
        plan.commands()
            .iter()
            .filter_map(|command| match command {
                NativePlanCommand::Material(material) if &material.node_id == id => Some(material),
                _ => None,
            })
            .collect()
    }

    let theme = Theme::default();
    let workspace_id = PresentationNodeId::workspace(WorkspaceNodePart::Surface);
    let viewport_rect = LogicalRect::from_units(0, 0, 800 * 64, 600 * 64);
    let workspace = || {
        node(
            workspace_id.clone(),
            None,
            PresentationNodeRole::Workspace,
            PresentationNodeState::default(),
            viewport_rect,
            SceneRect::new(0, 0, 100, 30),
        )
    };

    let modal_id = PresentationNodeId::overlay(OverlayKind::Palette, OverlayNodePart::Surface);
    let modal = scene(
        vec![
            workspace(),
            overlay_surface(
                OverlayKind::Palette,
                OverlayPresentationKind::Modal,
                &workspace_id,
                LogicalRect::from_units(160 * 64, 100 * 64, 480 * 64, 300 * 64),
                SceneRect::new(20, 5, 60, 15),
            ),
            overlay_node(
                OverlayKind::Palette,
                OverlayNodePart::Title,
                &modal_id,
                PresentationNodeRole::OverlayTitle,
                PresentationNodeState::default(),
                LogicalRect::from_units(160 * 64, 100 * 64, 480 * 64, 20 * 64),
                SceneRect::new(20, 5, 60, 1),
            ),
            overlay_node(
                OverlayKind::Palette,
                OverlayNodePart::Input,
                &modal_id,
                PresentationNodeRole::TextInput,
                PresentationNodeState::default(),
                LogicalRect::from_units(176 * 64, 140 * 64, 448 * 64, 20 * 64),
                SceneRect::new(22, 7, 56, 1),
            ),
            overlay_node(
                OverlayKind::Palette,
                OverlayNodePart::Footer,
                &modal_id,
                PresentationNodeRole::OverlayFooter,
                PresentationNodeState::default(),
                LogicalRect::from_units(176 * 64, 360 * 64, 448 * 64, 20 * 64),
                SceneRect::new(22, 18, 56, 1),
            ),
            node(
                PresentationNodeId::overlay_item(
                    OverlayKind::Palette,
                    SemanticKey::new("selected"),
                ),
                Some(modal_id.clone()),
                PresentationNodeRole::Item,
                PresentationNodeState {
                    selected: true,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(176 * 64, 180 * 64, 448 * 64, 40 * 64),
                SceneRect::new(22, 9, 56, 2),
            ),
        ],
        Vec::new(),
    );
    let modal_plan = prepare_native_presentation(&modal, &theme).unwrap();

    let modal_materials = materials_for(&modal_plan, &modal_id);
    assert_eq!(
        modal_materials
            .iter()
            .map(|material| material.role)
            .collect::<Vec<_>>(),
        vec![
            NativeMaterialRole::ModalScrim,
            NativeMaterialRole::OverlaySurface
        ]
    );
    assert_eq!(modal_materials[0].logical_rect, viewport_rect);
    assert_eq!(modal_materials[0].clip, viewport_rect);
    assert_eq!(modal_materials[0].color, theme.ui.palette.modal_scrim);
    assert_eq!(theme.ui.radii.overlay, 10);
    assert_eq!(
        modal_materials[1].corner_radius_units,
        u64::from(theme.ui.radii.overlay) * 64
    );
    assert_eq!(
        modal_materials[1].raised_shadows,
        Some(theme.ui.elevation.raised)
    );

    for part in [
        OverlayNodePart::Title,
        OverlayNodePart::Input,
        OverlayNodePart::Footer,
    ] {
        let id = PresentationNodeId::overlay(OverlayKind::Palette, part);
        assert_eq!(
            materials_for(&modal_plan, &id)[0].role,
            NativeMaterialRole::OverlayBand
        );
    }

    // The overlay surface strokes its boundary inside its own top edge, and
    // the title band shares that edge; the band must start below the stroke
    // or it paints the top border out.
    let title_id = PresentationNodeId::overlay(OverlayKind::Palette, OverlayNodePart::Title);
    let stroke = u64::from(theme.ui.spacing.tiled_separator.max(1)) * 64;
    assert_eq!(
        materials_for(&modal_plan, &title_id)[0].logical_rect,
        LogicalRect::from_units(
            160 * 64,
            100 * 64 + i64::try_from(stroke).unwrap(),
            480 * 64,
            20 * 64 - stroke
        )
    );
    let input_id = PresentationNodeId::overlay(OverlayKind::Palette, OverlayNodePart::Input);
    assert_eq!(
        materials_for(&modal_plan, &input_id)[0].logical_rect,
        LogicalRect::from_units(176 * 64, 140 * 64, 448 * 64, 20 * 64),
        "bands inside the border keep their node rect"
    );

    let selected_id =
        PresentationNodeId::overlay_item(OverlayKind::Palette, SemanticKey::new("selected"));
    let selected = materials_for(&modal_plan, &selected_id);
    assert_eq!(
        selected
            .iter()
            .map(|material| material.role)
            .collect::<Vec<_>>(),
        vec![
            NativeMaterialRole::Selection,
            NativeMaterialRole::SelectionIndicator
        ]
    );
    assert_eq!(selected[0].color, theme.ui.palette.selection_fill);
    assert!(selected[0].corner_radius_units > 0);
    assert_eq!(theme.ui.selection.leading_indicator_width, 2);
    assert_eq!(selected[1].logical_rect.size.width_units(), 2 * 64);
    assert_eq!(selected[1].color, theme.ui.palette.focus);
    let selected_text = modal_plan
        .commands()
        .iter()
        .find_map(|command| match command {
            NativePlanCommand::Text(text) if text.node_id == selected_id => Some(text),
            _ => None,
        })
        .expect("overlay item has a typed text scope");
    assert_eq!(selected_text.cell_rect, Some(SceneRect::new(22, 9, 56, 2)));

    for (kind, treatment, radius) in [
        (
            OverlayKind::Welcome,
            OverlayPresentationKind::Welcome,
            theme.ui.radii.overlay,
        ),
        (
            OverlayKind::ContextMenu,
            OverlayPresentationKind::ContextMenu,
            theme.ui.radii.context_menu,
        ),
    ] {
        let id = PresentationNodeId::overlay(kind, OverlayNodePart::Surface);
        let plan = prepare_native_presentation(
            &scene(
                vec![
                    workspace(),
                    overlay_surface(
                        kind,
                        treatment,
                        &workspace_id,
                        LogicalRect::from_units(240 * 64, 160 * 64, 320 * 64, 200 * 64),
                        SceneRect::new(30, 8, 40, 10),
                    ),
                ],
                Vec::new(),
            ),
            &theme,
        )
        .unwrap();
        let surface = materials_for(&plan, &id);
        assert_eq!(surface.len(), 1);
        assert_eq!(surface[0].role, NativeMaterialRole::OverlaySurface);
        assert_eq!(surface[0].corner_radius_units, u64::from(radius) * 64);
        assert_eq!(surface[0].raised_shadows, Some(theme.ui.elevation.raised));
        assert!(plan.commands().iter().all(|command| !matches!(
            command,
            NativePlanCommand::Material(material)
                if material.role == NativeMaterialRole::ModalScrim
        )));
    }
    assert_eq!(theme.ui.radii.context_menu, 8);
}

#[test]
fn phase_five_workflow_family_maps_typed_regions_without_parsing_text() {
    let theme = Theme::default();
    let workspace_id = PresentationNodeId::workspace(WorkspaceNodePart::Surface);
    let pane_id = PresentationNodeId::pane(PaneId::new("pane-a"), PaneNodePart::Surface);
    let failure_id = PresentationNodeId::pane(
        PaneId::new("pane-a"),
        PaneNodePart::Workflow(WorkflowNodePart::Failure),
    );
    let approval_id = PresentationNodeId::pane(
        PaneId::new("pane-a"),
        PaneNodePart::Workflow(WorkflowNodePart::Approval),
    );
    let status_badge_id = PresentationNodeId::pane(
        PaneId::new("pane-a"),
        PaneNodePart::Workflow(WorkflowNodePart::Status),
    );
    let console_id = PresentationNodeId::pane(
        PaneId::new("pane-a"),
        PaneNodePart::Workflow(WorkflowNodePart::Console),
    );
    let canvas_id = PresentationNodeId::pane(
        PaneId::new("pane-a"),
        PaneNodePart::Workflow(WorkflowNodePart::ArtifactCanvas),
    );
    let workspace = scene(
        vec![
            node(
                workspace_id.clone(),
                None,
                PresentationNodeRole::Workspace,
                PresentationNodeState::default(),
                LogicalRect::from_units(0, 0, 800 * 64, 600 * 64),
                SceneRect::new(0, 0, 100, 30),
            ),
            node(
                pane_id.clone(),
                Some(workspace_id),
                PresentationNodeRole::Pane,
                PresentationNodeState::default(),
                LogicalRect::from_units(0, 20 * 64, 800 * 64, 560 * 64),
                SceneRect::new(0, 1, 100, 28),
            ),
            node(
                failure_id.clone(),
                Some(pane_id.clone()),
                PresentationNodeRole::Workflow(WorkflowRowRole::Callout),
                PresentationNodeState {
                    attention: true,
                    tone: PresentationTone::Failure,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(8 * 64, 60 * 64, 400 * 64, 20 * 64),
                SceneRect::new(1, 3, 50, 1),
            ),
            node(
                approval_id.clone(),
                Some(pane_id.clone()),
                PresentationNodeRole::Workflow(WorkflowRowRole::Callout),
                PresentationNodeState {
                    attention: true,
                    tone: PresentationTone::Waiting,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(8 * 64, 80 * 64, 400 * 64, 80 * 64),
                SceneRect::new(1, 4, 50, 4),
            ),
            node(
                status_badge_id.clone(),
                Some(pane_id.clone()),
                PresentationNodeRole::WorkflowStatusBadge,
                PresentationNodeState {
                    tone: PresentationTone::Waiting,
                    ..PresentationNodeState::default()
                },
                LogicalRect::from_units(8 * 64, 40 * 64, 120 * 64, 20 * 64),
                SceneRect::new(1, 2, 15, 1),
            ),
            node(
                console_id.clone(),
                Some(pane_id.clone()),
                PresentationNodeRole::Workflow(WorkflowRowRole::Console),
                PresentationNodeState::default(),
                LogicalRect::from_units(8 * 64, 160 * 64, 784 * 64, 20 * 64),
                SceneRect::new(1, 8, 98, 1),
            ),
            node(
                canvas_id.clone(),
                Some(pane_id.clone()),
                PresentationNodeRole::ArtifactCanvas,
                PresentationNodeState::default(),
                LogicalRect::from_units(8 * 64, 180 * 64, 784 * 64, 380 * 64),
                SceneRect::new(1, 9, 98, 19),
            ),
        ],
        Vec::new(),
    );
    let plan = prepare_native_presentation(&workspace, &theme).unwrap();
    let material = |id: &PresentationNodeId| {
        plan.commands()
            .iter()
            .find_map(|command| match command {
                NativePlanCommand::Material(material) if &material.node_id == id => Some(material),
                _ => None,
            })
            .expect("typed region material")
    };
    let text = |id: &PresentationNodeId| {
        plan.commands()
            .iter()
            .find_map(|command| match command {
                NativePlanCommand::Text(text) if &text.node_id == id => Some(text),
                _ => None,
            })
            .expect("typed region text")
    };

    assert_eq!(
        material(&failure_id).role,
        NativeMaterialRole::WorkflowCallout
    );
    assert_eq!(
        material(&failure_id).boundary.unwrap().color,
        theme.ui.palette.failure
    );
    assert_eq!(
        material(&approval_id).boundary.unwrap().color,
        theme.ui.palette.waiting
    );
    assert!(
        plan.commands().iter().all(|command| !matches!(
            command,
            NativePlanCommand::Material(material) if material.node_id == status_badge_id
        )),
        "the status word paints as tone-colored text, not a chip container"
    );
    assert_eq!(text(&status_badge_id).color, theme.ui.palette.waiting);
    assert_eq!(
        material(&console_id).role,
        NativeMaterialRole::WorkflowConsole
    );
    assert_eq!(
        text(&console_id).metrics.role,
        NativeTextMetricRole::Terminal
    );
    assert_eq!(
        material(&canvas_id).role,
        NativeMaterialRole::ArtifactCanvas
    );
    assert!(
        plan.commands()
            .iter()
            .filter_map(|command| match command {
                NativePlanCommand::Material(material) => Some(material),
                _ => None,
            })
            .all(|material| {
                material.node_id != pane_id
                    || (material.color != theme.ui.palette.failure
                        && material.color != theme.ui.palette.waiting)
            }),
        "failure and approval never tint the whole pane"
    );
}

#[test]
fn plan_rejects_clip_escape_unknown_transition_and_resource_overflow() {
    let root_id = PresentationNodeId::workspace(WorkspaceNodePart::Surface);
    let child_id = PresentationNodeId::pane(PaneId::new("pane-a"), PaneNodePart::Title);
    let escaped = scene(
        vec![
            node(
                root_id.clone(),
                None,
                PresentationNodeRole::Workspace,
                PresentationNodeState::default(),
                LogicalRect::from_units(0, 0, 100 * 64, 100 * 64),
                SceneRect::new(0, 0, 100, 30),
            ),
            node(
                child_id.clone(),
                Some(root_id),
                PresentationNodeRole::PaneTitle,
                PresentationNodeState::default(),
                LogicalRect::from_units(90 * 64, 10 * 64, 20 * 64, 20 * 64),
                SceneRect::new(1, 1, 10, 1),
            ),
        ],
        Vec::new(),
    );
    assert_eq!(
        prepare_native_presentation(&escaped, &Theme::default()),
        Err(NativePresentationPlanError::ChildEscapesParentClip)
    );

    let unknown_transition = scene(
        vec![node(
            child_id,
            None,
            PresentationNodeRole::PaneTitle,
            PresentationNodeState::default(),
            LogicalRect::from_units(0, 0, 20 * 64, 20 * 64),
            SceneRect::new(1, 1, 10, 1),
        )],
        vec![TransitionTarget {
            node_id: PresentationNodeId::workspace(WorkspaceNodePart::Status),
            role: TransitionRole::Selection,
            property: TransitionProperty::Scale,
            sequence: 0,
        }],
    );
    assert_eq!(
        prepare_native_presentation(&unknown_transition, &Theme::default()),
        Err(NativePresentationPlanError::TransitionReferencesMissingNode)
    );

    let repeated = node(
        PresentationNodeId::workspace(WorkspaceNodePart::Surface),
        None,
        PresentationNodeRole::Workspace,
        PresentationNodeState::default(),
        LogicalRect::from_units(0, 0, 800 * 64, 600 * 64),
        SceneRect::new(0, 0, 100, 30),
    );
    let oversized = scene(vec![repeated; MAX_NATIVE_PLAN_NODES + 1], Vec::new());
    assert_eq!(
        prepare_native_presentation(&oversized, &Theme::default()),
        Err(NativePresentationPlanError::ResourceLimit {
            resource: "presentation nodes",
            actual: MAX_NATIVE_PLAN_NODES + 1,
            maximum: MAX_NATIVE_PLAN_NODES,
        })
    );
}

#[test]
fn token_sampler_uses_direct_ui_colors_in_stable_clipped_order() {
    let theme = Theme::default();
    let bounds = LogicalRect::from_units(4 * 64, 6 * 64, 600 * 64, 280 * 64);
    let sampler = prepare_token_sampler(&theme, bounds).unwrap();

    assert_eq!(sampler.bounds(), bounds);
    assert_eq!(sampler.swatches().len(), 17);
    assert_eq!(sampler.swatches()[0].role, NativeTokenColorRole::Canvas);
    assert_eq!(sampler.swatches()[0].color, theme.ui.palette.canvas);
    assert_eq!(
        sampler.swatches()[16].role,
        NativeTokenColorRole::ModalScrim
    );
    assert_eq!(sampler.swatches()[16].color, theme.ui.palette.modal_scrim);
    for (index, swatch) in sampler.swatches().iter().enumerate() {
        assert_eq!(swatch.z_order, index as u32);
        assert_eq!(swatch.logical_rect, swatch.clip);
        assert!(bounds.contains(swatch.logical_rect.origin));
        assert!(swatch.logical_rect.right_units() <= bounds.right_units());
        assert!(swatch.logical_rect.bottom_units() <= bounds.bottom_units());
    }

    assert_eq!(
        prepare_token_sampler(&theme, LogicalRect::from_units(0, 0, 10, 10)),
        Err(NativePresentationPlanError::TokenSamplerBoundsTooSmall)
    );
}

#[test]
fn chip_tone_fills_keep_label_contrast_and_neutral_badges_stay_plain_text() {
    // WCAG relative luminance and contrast, matching the theme crate's own
    // resolved_ui_contrast math for opaque colors.
    fn luminance(color: UiColor) -> f64 {
        fn channel(value: u8) -> f64 {
            let value = f64::from(value) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.212_6 * channel(color.red)
            + 0.715_2 * channel(color.green)
            + 0.072_2 * channel(color.blue)
    }
    // Linear-space source-over, matching the GPU blend of chip fills on the
    // sRGB surface (V1 linearizes material colors before ALPHA_BLENDING), so
    // the contrast assertions model the composite that actually renders.
    fn blend(over: UiColor, base: UiColor) -> UiColor {
        fn decode(value: u8) -> f64 {
            let encoded = f64::from(value) / 255.0;
            if encoded <= 0.04045 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        }
        fn encode(value: f64) -> u8 {
            let clamped = value.clamp(0.0, 1.0);
            let encoded = if clamped <= 0.003_130_8 {
                clamped * 12.92
            } else {
                1.055 * clamped.powf(1.0 / 2.4) - 0.055
            };
            (encoded * 255.0).round() as u8
        }
        let alpha = f64::from(over.alpha) / 255.0;
        let channel =
            |over: u8, base: u8| encode(alpha * decode(over) + (1.0 - alpha) * decode(base));
        UiColor::rgb(
            channel(over.red, base.red),
            channel(over.green, base.green),
            channel(over.blue, base.blue),
        )
    }
    fn contrast_milli(foreground: UiColor, background: UiColor) -> u32 {
        let lighter = luminance(foreground).max(luminance(background));
        let darker = luminance(foreground).min(luminance(background));
        (((lighter + 0.05) / (darker + 0.05)) * 1_000.0).floor() as u32
    }

    let workspace_id = PresentationNodeId::workspace(WorkspaceNodePart::Surface);
    // Every tone a product chip can carry: pane badges map to
    // AgentIdentity/Waiting/Neutral, attention chips to Waiting/Failure, and
    // status words (Running/Complete/...) render without a container. Focus
    // is deliberately absent — no chip ships with it, and the light theme's
    // focus-on-chrome pair has no contrast headroom left for a tint.
    let tones = [
        (PaneBadgeKind::Task, PresentationTone::Running),
        (PaneBadgeKind::Agent, PresentationTone::Waiting),
        (PaneBadgeKind::Artifact, PresentationTone::Failure),
        (PaneBadgeKind::Zoomed, PresentationTone::Complete),
        (PaneBadgeKind::Floating, PresentationTone::AgentIdentity),
        (PaneBadgeKind::Stacked, PresentationTone::Neutral),
    ];
    let mut nodes = vec![node(
        workspace_id.clone(),
        None,
        PresentationNodeRole::Workspace,
        PresentationNodeState::default(),
        LogicalRect::from_units(0, 0, 800 * 64, 600 * 64),
        SceneRect::new(0, 0, 100, 30),
    )];
    for (index, (kind, tone)) in tones.iter().enumerate() {
        nodes.push(node(
            PresentationNodeId::pane(PaneId::new("pane-a"), PaneNodePart::Badge(*kind)),
            Some(workspace_id.clone()),
            PresentationNodeRole::PaneBadge(*kind),
            PresentationNodeState {
                tone: *tone,
                ..PresentationNodeState::default()
            },
            LogicalRect::from_units((10 + index as i64 * 90) * 64, 30 * 64, 80 * 64, 17 * 64),
            SceneRect::new(1 + index as u16 * 8, 1, 6, 1),
        ));
    }

    for name in Theme::BUILTIN_NAMES {
        let theme = Theme::builtin(name).unwrap();
        let minimum = if *name == "mandatum-high-contrast" {
            7_000
        } else {
            4_500
        };
        let plan = prepare_native_presentation(&scene(nodes.clone(), Vec::new()), &theme).unwrap();
        for (kind, tone) in &tones {
            let id = PresentationNodeId::pane(PaneId::new("pane-a"), PaneNodePart::Badge(*kind));
            let fill = plan.commands().iter().find_map(|command| match command {
                NativePlanCommand::Material(material) if material.node_id == id => Some(material),
                _ => None,
            });
            let text = plan
                .commands()
                .iter()
                .find_map(|command| match command {
                    NativePlanCommand::Text(text) if text.node_id == id => Some(text),
                    _ => None,
                })
                .expect("every badge keeps its typed glyph scope");
            if *tone == PresentationTone::Neutral {
                assert!(
                    fill.is_none(),
                    "{name}: a neutral badge is plain muted text without a container"
                );
                assert_eq!(text.color, theme.ui.palette.text_muted);
                continue;
            }
            if *name == "mandatum-high-contrast" {
                // High-contrast drops the container: no visible tint can hold
                // the 7:1 text bar, so chips render as plain tone text and the
                // contrast pairs below are the raw text-on-rail pairs already
                // asserted by the theme's own resolved_ui_contrast gate.
                assert!(
                    fill.is_none(),
                    "{name}: high-contrast chips render without a tinted container"
                );
                for (surface, rail) in [
                    ("chrome", theme.ui.palette.chrome_surface),
                    ("pane", theme.ui.palette.pane_surface),
                    ("overlay", theme.ui.palette.overlay_surface),
                ] {
                    let actual = contrast_milli(text.color, rail);
                    assert!(
                        actual >= minimum,
                        "{name}: chip label loses contrast on its bare {surface} rail \
                         ({actual} < {minimum})"
                    );
                }
                continue;
            }
            let fill = fill.expect("stateful chips keep a tone-tinted fill");
            assert!(
                fill.color.alpha < 64,
                "{name}: chip fills stay a low-alpha tint, got alpha {}",
                fill.color.alpha
            );
            assert_eq!(fill.boundary, None);
            for (surface, rail) in [
                ("chrome", theme.ui.palette.chrome_surface),
                ("pane", theme.ui.palette.pane_surface),
                ("overlay", theme.ui.palette.overlay_surface),
            ] {
                let blended = blend(fill.color, rail);
                let actual = contrast_milli(text.color, blended);
                assert!(
                    actual >= minimum,
                    "{name}: chip label loses contrast over its tinted {surface} rail \
                     ({actual} < {minimum})"
                );
            }
        }
    }
}
