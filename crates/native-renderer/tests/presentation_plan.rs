use mandatum_native_renderer::{
    MAX_NATIVE_PLAN_NODES, NativeFontFace, NativeMaterialRole, NativePlanCommand,
    NativePresentationPlanError, NativeTextMetricRole, NativeTokenColorRole,
    prepare_native_presentation, prepare_scene, prepare_token_sampler,
};
use mandatum_scene::{
    BackingScale, EmptyContent, HeaderScene, LogicalRect, LogicalSize, PaneContent, PaneId,
    PaneNodePart, PaneScene, PaneSceneKind, PhysicalSize, PresentationNode, PresentationNodeId,
    PresentationNodeRole, PresentationNodeState, ScenePresentation, SceneRect, SceneSize,
    StatusScene, TerminalProjection, Theme, TransitionProperty, TransitionTarget, ViewportMetrics,
    WorkspaceNodePart, WorkspaceScene, compile_cell_program,
};

fn scene(nodes: Vec<PresentationNode>, transitions: Vec<TransitionTarget>) -> WorkspaceScene {
    let pane_id = PaneId::new("pane-a");
    WorkspaceScene {
        size: SceneSize::new(100, 30),
        header: HeaderScene {
            area: SceneRect::new(0, 0, 100, 1),
            workspace_name: "test".into(),
            session_name: "session".into(),
            pane_count: 1,
            focused_pane: pane_id.clone(),
            zoomed: false,
            connector_label: "none".into(),
            text: "test".into(),
            attention: Vec::new(),
        },
        panes: vec![PaneScene {
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
                Some(pane_id),
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
            property: TransitionProperty::Opacity,
        }],
    );

    let theme = Theme::default();
    let plan = prepare_native_presentation(&scene, &theme).unwrap();
    assert_eq!(plan.material_count(), 2);
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

    assert_eq!(plan.transitions().len(), 1);
    assert_eq!(plan.transitions()[0].node_id, title_id);
    assert_eq!(plan.transitions()[0].property, TransitionProperty::Opacity);
    assert_eq!(plan.transitions()[0].timing, theme.ui.motion.overlay_enter);

    let prepared = prepare_scene(&scene, &theme).unwrap();
    assert_eq!(prepared.presentation_plan(), &plan);
    assert_eq!(
        prepared.cell_program(),
        &compile_cell_program(&scene, &theme),
        "native planning must not rewrite the terminal-parity projection"
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
            property: TransitionProperty::Scale,
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
