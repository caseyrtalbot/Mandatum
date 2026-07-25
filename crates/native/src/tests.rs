use std::cell::RefCell;

use mandatum_scene::input::{PointerButton, PointerKind};

use super::{
    FontPreflightOutcome, PressedPointerButtons, apply_renderer_scale_transition,
    launch_after_font_preflight, logical_pointer_position, native_window_geometry,
    native_window_title, next_native_window_title, parse_launch_options,
    pointer_input_needs_redraw, start_after_preflight,
};

#[test]
fn native_window_geometry_has_intentional_initial_and_minimum_logical_sizes() {
    let geometry = native_window_geometry();

    assert_eq!(
        (geometry.initial.width, geometry.initial.height),
        (1_200.0, 800.0)
    );
    assert_eq!(
        (geometry.minimum.width, geometry.minimum.height),
        (720.0, 480.0)
    );
}

#[test]
fn native_window_title_uses_trimmed_scene_project_label_with_blank_fallback() {
    assert_eq!(native_window_title("mandatum"), "Mandatum — mandatum");
    assert_eq!(
        native_window_title("  active project  "),
        "Mandatum — active project"
    );
    assert_eq!(native_window_title(" \t\n"), "Mandatum");
}

#[test]
fn native_window_title_updates_only_when_scene_project_label_changes() {
    assert_eq!(
        next_native_window_title("Mandatum", "mandatum"),
        Some("Mandatum — mandatum".to_owned())
    );
    assert_eq!(
        next_native_window_title("Mandatum — mandatum", "mandatum"),
        None
    );
}

#[test]
fn pointer_move_redraw_policy_tracks_host_owned_separator_hover() {
    let quiet = (false, None);
    let first_separator = (false, Some(0));
    let second_separator = (false, Some(1));
    let continuous = (true, None);

    assert!(!pointer_input_needs_redraw(PointerKind::Move, quiet, quiet));
    assert!(pointer_input_needs_redraw(
        PointerKind::Move,
        quiet,
        first_separator
    ));
    assert!(pointer_input_needs_redraw(
        PointerKind::Move,
        first_separator,
        quiet
    ));
    assert!(
        !pointer_input_needs_redraw(PointerKind::Move, first_separator, first_separator),
        "motion within the same separator hover target stays quiet"
    );
    assert!(pointer_input_needs_redraw(
        PointerKind::Move,
        first_separator,
        second_separator
    ));
    assert!(pointer_input_needs_redraw(
        PointerKind::Move,
        continuous,
        continuous
    ));
    assert!(pointer_input_needs_redraw(PointerKind::Drag, quiet, quiet));
    assert!(pointer_input_needs_redraw(PointerKind::Down, quiet, quiet));
}

#[test]
fn physical_cursor_position_is_preserved_as_logical_pixels_at_backing_scale() {
    let logical = logical_pointer_position(1_000.0, 600.0, 2.0).expect("valid 2x position");
    assert_eq!((logical.x_pixels(), logical.y_pixels()), (500.0, 300.0));
    assert!(logical_pointer_position(1.0, 1.0, 0.0).is_none());
}

#[test]
fn startup_constructs_host_only_after_window_and_gpu() {
    let order = RefCell::new(Vec::new());
    let result = start_after_preflight(
        || {
            order.borrow_mut().push("window");
            Ok::<_, &'static str>("window")
        },
        |_| {
            order.borrow_mut().push("gpu");
            Ok::<_, &'static str>("gpu")
        },
        || {
            order.borrow_mut().push("host");
            "host"
        },
    )
    .expect("startup succeeds");
    assert_eq!(result, ("window", "gpu", "host"));
    assert_eq!(*order.borrow(), ["window", "gpu", "host"]);
}

#[test]
fn failed_gpu_preflight_never_constructs_host() {
    let mut host_created = false;
    let result = start_after_preflight(
        || Ok::<_, &'static str>("window"),
        |_| Err::<(), _>("no adapter"),
        || {
            host_created = true;
            "host"
        },
    );
    assert_eq!(result, Err("no adapter"));
    assert!(!host_created);
}

#[test]
fn failed_window_preflight_never_constructs_gpu_or_host() {
    let gpu_created = RefCell::new(false);
    let host_created = RefCell::new(false);
    let result = start_after_preflight(
        || Err::<(), _>("no display"),
        |_| {
            *gpu_created.borrow_mut() = true;
            Ok::<_, &'static str>("gpu")
        },
        || {
            *host_created.borrow_mut() = true;
            "host"
        },
    );
    assert_eq!(result, Err("no display"));
    assert!(!*gpu_created.borrow());
    assert!(!*host_created.borrow());
}

#[test]
fn every_gpu_preflight_failure_stops_before_host_creation() {
    for failure in ["no adapter", "surface", "device"] {
        let mut host_created = false;
        let result = start_after_preflight(
            || Ok::<_, &'static str>("window"),
            |_| Err::<(), _>(failure),
            || {
                host_created = true;
                "host"
            },
        );
        assert_eq!(result, Err(failure));
        assert!(!host_created);
    }
}

#[test]
fn native_font_options_are_strict_and_lab_flags_are_rejected() {
    let options = parse_launch_options(
        ["--font-family", "Berkeley Mono", "--font-size", "16.5"]
            .into_iter()
            .map(str::to_owned),
    )
    .expect("valid native launch options");
    assert_eq!(options.font_request.requested_family(), "Berkeley Mono");
    assert_eq!(options.font_request.size(), 16.5);
    assert!(!options.font_info);

    assert!(
        parse_launch_options(["--soak"].into_iter().map(str::to_owned))
            .unwrap_err()
            .contains("unknown native option")
    );
    assert!(
        parse_launch_options(["--font-family", "--soak"].into_iter().map(str::to_owned))
            .unwrap_err()
            .contains("--font-family requires a value")
    );
    assert!(
        launch_after_font_preflight(
            ["--font-size", "500"].into_iter().map(str::to_owned),
            |_| Ok(())
        )
        .unwrap_err()
        .contains("between 6 and 72")
    );
}

#[test]
fn font_info_is_stable_headless_json_and_default_is_bundled_jetbrains_mono_13() {
    let options = parse_launch_options(["--font-info"].into_iter().map(str::to_owned))
        .expect("valid options");
    assert!(options.font_info);
    assert_eq!(options.font_request.requested_family(), "JetBrains Mono");
    assert_eq!(options.font_request.size(), 13.0);

    let downstream_constructed = RefCell::new(false);
    let FontPreflightOutcome::Info(json) =
        launch_after_font_preflight(["--font-info"].into_iter().map(str::to_owned), |_| {
            *downstream_constructed.borrow_mut() = true;
            Ok(())
        })
        .expect("headless font preflight")
    else {
        panic!("--font-info must stop before application launch");
    };
    assert!(!*downstream_constructed.borrow());
    assert_eq!(
        json,
        r#"{"source":"bundled","requested_family":"JetBrains Mono","size":13.0,"faces":{"regular":"JetBrainsMono-Regular","bold":"JetBrainsMono-Bold","italic":"JetBrainsMono-Italic","bold_italic":"JetBrainsMono-BoldItalic"}}"#
    );
}

#[test]
fn invalid_explicit_font_fails_during_font_preflight() {
    let downstream_constructed = RefCell::new(false);
    let error = launch_after_font_preflight(
        ["--font-family", "monospace"]
            .into_iter()
            .map(str::to_owned),
        |_| {
            *downstream_constructed.borrow_mut() = true;
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.contains("generic font family"));
    assert!(
        !*downstream_constructed.borrow(),
        "font failure must prevent AppConfig/event-loop/window/GPU/host launch construction"
    );
}

#[test]
fn scale_transition_refreshes_the_physical_surface_before_scene_reflow() {
    #[derive(Default)]
    struct RendererProbe {
        scale: f32,
        surface: (u32, u32),
        order: Vec<&'static str>,
    }

    let mut renderer = RendererProbe::default();
    apply_renderer_scale_transition(
        &mut renderer,
        2.0,
        (1_600, 1_264),
        |renderer, scale| {
            renderer.order.push("scale");
            renderer.scale = scale;
            Ok(())
        },
        |renderer, width, height| {
            renderer.order.push("surface");
            renderer.surface = (width, height);
        },
    )
    .expect("valid transition");

    assert_eq!(renderer.scale, 2.0);
    assert_eq!(renderer.surface, (1_600, 1_264));
    assert_eq!(renderer.order, ["scale", "surface"]);

    let mut rejected = RendererProbe::default();
    assert_eq!(
        apply_renderer_scale_transition(
            &mut rejected,
            f32::NAN,
            (1_600, 1_264),
            |renderer, _| {
                renderer.order.push("scale");
                Err("invalid scale".to_owned())
            },
            |renderer, width, height| {
                renderer.order.push("surface");
                renderer.surface = (width, height);
            },
        ),
        Err("invalid scale".to_owned())
    );
    assert_eq!(rejected.order, ["scale"]);
    assert_eq!(rejected.surface, (0, 0));
}

#[test]
fn pressed_pointer_state_distinguishes_drag_and_resets() {
    let mut buttons = PressedPointerButtons::default();
    assert_eq!(buttons.active(), None);
    assert!(!buttons.begin(PointerButton::Left, false));
    assert_eq!(buttons.active(), None);
    assert!(buttons.begin(PointerButton::Left, true));
    assert!(!buttons.begin(PointerButton::Right, true));
    assert_eq!(buttons.active(), Some(PointerButton::Left));
    assert_eq!(buttons.all(), [PointerButton::Left]);
    buttons.clear();
    assert_eq!(buttons.active(), None);
    assert!(buttons.all().is_empty());
}

#[test]
fn live_slice_displayed_route_launches_the_native_product() {
    let script = include_str!("../../../examples/live-slice/run.sh");
    assert!(
        script.contains("exec cargo run -q -p mandatum-native --bin mandatum-native"),
        "the live-slice displayed route must launch the native product"
    );
    assert!(
        !script.contains("exec cargo run -q -p mandatum-app"),
        "the displayed live slice must not silently fall back to the terminal adapter"
    );
}
