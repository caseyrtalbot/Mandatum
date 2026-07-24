use std::cell::RefCell;

use mandatum_scene::input::PointerButton;

use super::{PressedPointerButtons, parse_text_settings, start_after_preflight};

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
    let settings = parse_text_settings(
        ["--font-family", "Berkeley Mono", "--font-size", "16.5"]
            .into_iter()
            .map(str::to_owned),
    )
    .expect("valid native text settings");
    assert_eq!(settings.family(), "Berkeley Mono");
    assert_eq!(settings.font_size(), 16.5);

    assert!(
        parse_text_settings(["--soak"].into_iter().map(str::to_owned))
            .unwrap_err()
            .contains("unknown native option")
    );
    assert!(
        parse_text_settings(["--font-family", "--soak"].into_iter().map(str::to_owned))
            .unwrap_err()
            .contains("--font-family requires a value")
    );
    assert!(
        parse_text_settings(["--font-size", "500"].into_iter().map(str::to_owned))
            .unwrap_err()
            .contains("between 6 and 72")
    );
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
