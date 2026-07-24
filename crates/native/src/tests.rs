use std::cell::RefCell;

use mandatum_scene::input::PointerButton;

use super::{
    FontPreflightOutcome, PressedPointerButtons, apply_renderer_scale_transition,
    launch_after_font_preflight, parse_launch_options, start_after_preflight,
};

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
