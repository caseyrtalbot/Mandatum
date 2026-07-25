use mandatum_scene::{
    BackingScale, LogicalPoint, LogicalRect, LogicalSize, PhysicalSize, PresentationNodeId,
    SceneRect, SceneSize, ViewportMetrics, WorkspaceNodePart,
    input::{AccessibilityAction, AccessibilityActionEvent},
};

fn metrics(scale: f64) -> ViewportMetrics {
    ViewportMetrics::new(
        LogicalSize::from_pixels(800.0, 480.0).unwrap(),
        PhysicalSize::new((800.0 * scale) as u32, (480.0 * scale) as u32),
        BackingScale::new(scale).unwrap(),
        LogicalSize::from_pixels(8.0, 16.0).unwrap(),
    )
    .unwrap()
}

#[test]
fn viewport_metrics_derive_identical_cell_and_logical_geometry_at_1x_and_2x() {
    for scale in [1.0, 2.0] {
        let viewport = metrics(scale);
        assert_eq!(viewport.scene_size(), SceneSize::new(100, 30));
        assert_eq!(
            viewport.logical_rect_for_cells(SceneRect::new(2, 3, 10, 4)),
            LogicalRect::new(
                LogicalPoint::from_pixels(16.0, 48.0).unwrap(),
                LogicalSize::from_pixels(80.0, 64.0).unwrap(),
            )
        );

        let mapping = viewport
            .logical_point_to_cell(
                SceneRect::new(2, 3, 10, 4),
                LogicalPoint::from_pixels(95.98, 111.98).unwrap(),
            )
            .unwrap();
        assert_eq!(mapping, (9, 3));
        assert_eq!(
            viewport.logical_point_to_cell(
                SceneRect::new(2, 3, 10, 4),
                LogicalPoint::from_pixels(96.0, 112.0).unwrap(),
            ),
            None,
            "logical rectangles are half-open"
        );
    }
}

#[test]
fn viewport_metrics_reject_invalid_scale_and_incoherent_physical_size() {
    assert!(BackingScale::new(0.0).is_err());
    assert!(BackingScale::new(-1.0).is_err());
    assert!(BackingScale::new(f64::NAN).is_err());
    assert!(BackingScale::new(f64::INFINITY).is_err());

    let logical = LogicalSize::from_pixels(800.0, 480.0).unwrap();
    let cells = LogicalSize::from_pixels(8.0, 16.0).unwrap();
    let scale = BackingScale::new(2.0).unwrap();
    assert!(
        ViewportMetrics::new(logical, PhysicalSize::new(1601, 960), scale, cells).is_ok(),
        "one physical pixel of scale disagreement is tolerated"
    );
    assert!(
        ViewportMetrics::new(logical, PhysicalSize::new(1602, 960), scale, cells).is_err(),
        "more than one physical pixel is rejected"
    );
}

#[test]
fn accessibility_actions_are_typed_and_bound_to_a_scene_revision_and_node() {
    let node_id = PresentationNodeId::workspace(WorkspaceNodePart::Status);
    let event = AccessibilityActionEvent {
        scene_revision: 41,
        node_id: node_id.clone(),
        action: AccessibilityAction::SetText("cargo test".to_owned()),
    };

    assert_eq!(event.scene_revision, 41);
    assert_eq!(event.node_id, node_id);
    assert_eq!(
        event.action,
        AccessibilityAction::SetText("cargo test".to_owned())
    );
}
