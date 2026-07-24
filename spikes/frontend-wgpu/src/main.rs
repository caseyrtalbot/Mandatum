//! Excluded native/GPU frontend over Mandatum's real workstation host.
//!
//! The spike owns winit, wgpu, clipboard integration, paint scheduling, and
//! latency instrumentation. Product state, PTYs, parsing, commands, recovery,
//! and persistence stay behind `mandatum_app::FrontendHost`.

mod stats;

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use mandatum_app::{AppConfig, FrontendEffect, FrontendHost};
use mandatum_gpu_renderer_spike::{GpuText, NativeTextSettings, SceneCompileError};
use mandatum_scene::{
    SceneSize, WorkspaceScene,
    input::{
        CompositionEvent, InputEvent, Key as InputKey, KeyCode, Modifiers, PointerButton,
        PointerEvent, PointerKind, TextRange,
    },
};
use stats::Samples;
#[cfg(target_os = "macos")]
use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    platform::modifier_supplement::KeyEventExtModifierSupplement,
    window::{Window, WindowId},
};

const INJECT_TOTAL: u32 = 300;
const INJECT_INTERVAL: Duration = Duration::from_micros(33_333);
const HEARTBEAT: Duration = Duration::from_millis(250);
const EVENT_DRAIN_BUDGET: usize = 256;
const IDLE_FRAME_CUTOFF_MS: f64 = 250.0;

#[derive(Clone)]
struct Config {
    exit_after: Option<f64>,
    typing_bench: bool,
    flood: bool,
    scale_after: Option<f64>,
    scale_factor: f32,
    text_settings: NativeTextSettings,
}

fn parse_config() -> Config {
    let defaults = NativeTextSettings::default();
    let mut font_family = defaults.family().to_owned();
    let mut font_size = defaults.font_size();
    let mut config = Config {
        exit_after: None,
        typing_bench: false,
        flood: false,
        scale_after: None,
        scale_factor: 1.5,
        text_settings: defaults,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-after" => {
                config.exit_after = args.next().and_then(|value| value.parse().ok());
            }
            "--typing-bench" => config.typing_bench = true,
            "--flood" => config.flood = true,
            "--scale-after" => {
                config.scale_after = args.next().and_then(|value| parse_scale_delay(&value));
            }
            "--scale-factor" => {
                config.scale_factor = args
                    .next()
                    .and_then(|value| parse_scale_factor(&value))
                    .unwrap_or(config.scale_factor);
            }
            "--font-family" => {
                if let Some(family) = args.next().and_then(|value| parse_font_family(&value)) {
                    font_family = family;
                }
            }
            "--font-size" => {
                if let Some(size) = args.next().and_then(|value| parse_font_size(&value)) {
                    font_size = size;
                }
            }
            other => eprintln!("ignoring unknown arg: {other}"),
        }
    }
    config.text_settings =
        NativeTextSettings::new(font_family, font_size).expect("validated native text arguments");
    config
}

fn parse_scale_delay(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && (0.0..=3600.0).contains(seconds))
}

fn parse_scale_factor(value: &str) -> Option<f32> {
    value
        .parse::<f32>()
        .ok()
        .filter(|scale| scale.is_finite() && (0.25..=8.0).contains(scale))
}

fn parse_font_family(value: &str) -> Option<String> {
    let family = value.trim();
    (!family.is_empty() && family.len() <= 128 && !family.chars().any(char::is_control))
        .then(|| family.to_owned())
}

fn parse_font_size(value: &str) -> Option<f32> {
    value
        .parse::<f32>()
        .ok()
        .filter(|size| size.is_finite() && (6.0..=72.0).contains(size))
}

#[derive(Debug)]
enum UserEvent {
    Wake,
}

enum PlatformAction {
    Input(InputEvent),
    PasteShortcut(InputKey),
    CopyShortcut(InputKey),
    Ignore,
}

#[derive(Default)]
struct PressedPointerButtons {
    left: bool,
    middle: bool,
    right: bool,
}

impl PressedPointerButtons {
    fn set(&mut self, button: PointerButton, pressed: bool) {
        match button {
            PointerButton::Left => self.left = pressed,
            PointerButton::Middle => self.middle = pressed,
            PointerButton::Right => self.right = pressed,
        }
    }

    fn active(&self) -> Option<PointerButton> {
        if self.left {
            Some(PointerButton::Left)
        } else if self.middle {
            Some(PointerButton::Middle)
        } else if self.right {
            Some(PointerButton::Right)
        } else {
            None
        }
    }

    fn all(&self) -> Vec<PointerButton> {
        [
            (self.left, PointerButton::Left),
            (self.middle, PointerButton::Middle),
            (self.right, PointerButton::Right),
        ]
        .into_iter()
        .filter_map(|(pressed, button)| pressed.then_some(button))
        .collect()
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

struct App {
    config: Config,
    host: FrontendHost,
    window: Option<std::sync::Arc<Window>>,
    gpu: Option<GpuText>,
    clipboard: Option<arboard::Clipboard>,

    input_to_present: Samples,
    frame_ms: Samples,
    pending_inputs: VecDeque<Instant>,
    dirty_from_runtime: bool,
    last_present: Option<Instant>,
    dropped_correlations: u32,

    start: Instant,
    deadline: Option<Instant>,
    next_heartbeat: Instant,
    summary_printed: bool,
    fatal_error: Option<String>,

    injected: u32,
    next_inject: Instant,
    inject_letter: u8,
    modifiers: ModifiersState,
    mouse_pixels: Option<(f64, f64)>,
    mouse_cell: (u16, u16),
    pressed_pointer_buttons: PressedPointerButtons,
    wheel_cell_remainder: (f64, f64),
    scene_presentable: bool,
    scale_probe_at: Option<Instant>,
    scale_probe_applied: bool,
    window_focused: bool,
    ime_allowed: bool,
}

impl App {
    fn new(config: Config, proxy: EventLoopProxy<UserEvent>, app_config: AppConfig) -> Self {
        let wake_proxy = proxy.clone();
        let mut host = FrontendHost::new_with_wake_callback(app_config, move || {
            let _ = wake_proxy.send_event(UserEvent::Wake);
        });
        let now = Instant::now();
        let scale_probe_at = config
            .scale_after
            .map(|seconds| now + Duration::from_secs_f64(seconds));
        let clipboard = match arboard::Clipboard::new() {
            Ok(clipboard) => Some(clipboard),
            Err(error) => {
                host.report_platform_error(format!("clipboard unavailable: {error}"));
                None
            }
        };
        Self {
            config,
            host,
            window: None,
            gpu: None,
            clipboard,
            input_to_present: Samples::new(),
            frame_ms: Samples::new(),
            pending_inputs: VecDeque::new(),
            dirty_from_runtime: false,
            last_present: None,
            dropped_correlations: 0,
            start: now,
            deadline: None,
            next_heartbeat: now + HEARTBEAT,
            summary_printed: false,
            fatal_error: None,
            injected: 0,
            next_inject: now,
            inject_letter: b'a',
            modifiers: ModifiersState::empty(),
            mouse_pixels: None,
            mouse_cell: (0, 0),
            pressed_pointer_buttons: PressedPointerButtons::default(),
            wheel_cell_remainder: (0.0, 0.0),
            scene_presentable: false,
            scale_probe_at,
            scale_probe_applied: false,
            window_focused: false,
            ime_allowed: false,
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn scene_size(&self) -> Option<SceneSize> {
        let gpu = self.gpu.as_ref()?;
        let (width, height) = gpu.surface_size();
        scene_size_from_metrics(width, height, gpu.cell_w(), gpu.cell_h())
    }

    fn resize_host(&mut self) {
        if let Some(size) = self.scene_size() {
            self.host.handle_input(InputEvent::Resize(size));
        }
    }

    fn apply_scale_factor(&mut self, scale_factor: f32) {
        self.scene_presentable = false;
        self.cancel_pointer_gesture();
        self.host.suspend_scene_interaction();
        if let Some(gpu) = &mut self.gpu
            && let Err(error) = gpu.set_scale(scale_factor)
        {
            self.fatal_error = Some(error);
            return;
        }
        self.refresh_mouse_cell();
        self.resize_host();
        self.request_redraw();
    }

    fn send_input(&mut self, input: InputEvent, measured: bool, at: Instant) {
        if measured {
            if self.pending_inputs.len() < 64 {
                self.pending_inputs.push_back(at);
            } else {
                self.dropped_correlations += 1;
            }
        }
        self.host.handle_input(input);
        self.apply_effects();
        self.request_redraw();
    }

    fn apply_effects(&mut self) {
        for effect in self.host.take_effects() {
            match effect {
                FrontendEffect::SetClipboard(text) => {
                    if let Some(clipboard) = &mut self.clipboard {
                        if let Err(error) = clipboard.set_text(text) {
                            self.host
                                .report_platform_error(format!("clipboard write failed: {error}"));
                        }
                    } else {
                        self.host
                            .report_platform_error("clipboard write failed: clipboard unavailable");
                    }
                }
            }
        }
    }

    fn drain_runtime(&mut self) -> bool {
        let drained = self.host.drain_runtime();
        if drained > 0 {
            self.dirty_from_runtime = true;
        }
        self.apply_effects();
        drained == EVENT_DRAIN_BUDGET
    }

    fn render_frame(&mut self) -> Result<(), SceneCompileError> {
        self.scene_presentable = false;
        let Some(size) = self.scene_size() else {
            self.cancel_and_disable_ime();
            self.host.suspend_scene_interaction();
            return Ok(());
        };
        let snapshot = self.host.frame(size);
        if scene_is_suspended_by_tiled_minimum(&snapshot.scene) {
            self.cancel_and_disable_ime();
            self.host.suspend_scene_interaction();
            return Ok(());
        }
        self.sync_ime(&snapshot.scene);
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        if let Some(present) = gpu.render(&snapshot.scene, &snapshot.theme)? {
            self.scene_presentable = true;
            if let Some(last) = self.last_present {
                let frame_ms = present.duration_since(last).as_secs_f64() * 1000.0;
                if frame_ms < IDLE_FRAME_CUTOFF_MS {
                    self.frame_ms.push(frame_ms);
                }
            }
            self.last_present = Some(present);
            if self.dirty_from_runtime {
                if let Some(input) = self.pending_inputs.pop_front() {
                    self.input_to_present
                        .push(present.duration_since(input).as_secs_f64() * 1000.0);
                }
                self.dirty_from_runtime = false;
            }
        } else {
            self.host.suspend_scene_interaction();
        }
        Ok(())
    }

    fn maybe_inject(&mut self, now: Instant) {
        if !self.config.typing_bench {
            return;
        }
        while self.injected < INJECT_TOTAL && now >= self.next_inject {
            if self.injected > 0 && self.injected.is_multiple_of(40) {
                self.send_input(InputEvent::Key(InputKey::ctrl('u')), false, now);
            }
            let letter = self.inject_letter;
            self.inject_letter = if letter >= b'z' { b'a' } else { letter + 1 };
            self.send_input(
                InputEvent::Key(InputKey::plain(KeyCode::Char(char::from(letter)))),
                true,
                now,
            );
            self.injected += 1;
            self.next_inject += INJECT_INTERVAL;
        }
    }

    fn print_summary(&mut self) {
        if self.summary_printed {
            return;
        }
        self.summary_printed = true;
        if let Some(error) = &self.fatal_error {
            println!(
                "{{\"error\":{:?},\"notes\":\"run failed; measurements are incomplete\"}}",
                error
            );
            return;
        }
        let mut notes = format!(
            "host=FrontendHost present=Fifo(vsync) typing_bench={} flood={} scale_probe_applied={} input_samples={} frame_samples={}",
            self.config.typing_bench,
            self.config.flood,
            self.scale_probe_applied,
            self.input_to_present.len(),
            self.frame_ms.len(),
        );
        if self.dropped_correlations > 0 {
            notes.push_str(&format!(
                " dropped_correlations={}",
                self.dropped_correlations
            ));
        }
        notes.push_str(
            " method: neutral input stamped at winit receipt, present timestamped after queue.submit+present; one pending input correlated per runtime-wake-driven present; frame_ms filters idle gaps >=250ms.",
        );
        println!(
            "{{\"input_to_present_ms\":{{\"p50\":{:.2},\"p95\":{:.2},\"max\":{:.2}}},\"frame_ms\":{{\"p50\":{:.2},\"p95\":{:.2}}},\"frames\":{},\"notes\":{:?}}}",
            self.input_to_present.percentile(50.0),
            self.input_to_present.percentile(95.0),
            self.input_to_present.max(),
            self.frame_ms.percentile(50.0),
            self.frame_ms.percentile(95.0),
            self.frame_ms.len(),
            notes,
        );
    }

    fn exit_if_requested(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.host.should_quit() {
            return false;
        }
        self.cancel_and_disable_ime();
        self.host.shutdown();
        self.print_summary();
        event_loop.exit();
        true
    }

    fn update_mouse_cell(&mut self, x: f64, y: f64) {
        self.mouse_pixels = Some((x, y));
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(size) = self.scene_size() else {
            return;
        };
        self.mouse_cell = (
            ((x / f64::from(gpu.cell_w())).floor() as i64)
                .clamp(0, i64::from(size.width.saturating_sub(1))) as u16,
            ((y / f64::from(gpu.cell_h())).floor() as i64)
                .clamp(0, i64::from(size.height.saturating_sub(1))) as u16,
        );
    }

    fn refresh_mouse_cell(&mut self) {
        if let Some((x, y)) = self.mouse_pixels {
            self.update_mouse_cell(x, y);
        }
    }

    fn pointer_input(&mut self, kind: PointerKind, button: Option<PointerButton>) {
        if !self.scene_presentable || self.scene_size().is_none() {
            return;
        }
        self.send_pointer_input(kind, button);
    }

    fn send_pointer_input(&mut self, kind: PointerKind, button: Option<PointerButton>) {
        let redraw = kind != PointerKind::Move || self.host.pointer_move_needs_redraw();
        let (column, row) = self.mouse_cell;
        let input = InputEvent::Pointer(PointerEvent {
            kind,
            button,
            column,
            row,
            mods: neutral_modifiers(self.modifiers),
        });
        self.host.handle_input(input);
        self.apply_effects();
        if redraw {
            self.request_redraw();
        }
    }

    fn pointer_motion(&mut self) {
        match self.pressed_pointer_buttons.active() {
            Some(button) => self.pointer_input(PointerKind::Drag, Some(button)),
            None => self.pointer_input(PointerKind::Move, None),
        }
    }

    fn pointer_button(&mut self, state: ElementState, button: PointerButton) {
        match state {
            ElementState::Pressed => {
                if self.pressed_pointer_buttons.active().is_some() {
                    return;
                }
                self.pressed_pointer_buttons.set(button, true);
                self.pointer_input(PointerKind::Down, Some(button));
            }
            ElementState::Released => {
                if self.pressed_pointer_buttons.all().contains(&button) {
                    self.send_pointer_input(PointerKind::Up, Some(button));
                    self.pressed_pointer_buttons.set(button, false);
                }
            }
        }
    }

    fn pointer_wheel(&mut self, delta: MouseScrollDelta) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                self.wheel_cell_remainder = (0.0, 0.0);
                ((-x).round() as i16, (-y).round() as i16)
            }
            MouseScrollDelta::PixelDelta(position) => {
                self.wheel_cell_remainder.0 += -position.x / f64::from(gpu.cell_w());
                self.wheel_cell_remainder.1 += -position.y / f64::from(gpu.cell_h());
                let dx = self.wheel_cell_remainder.0.trunc();
                let dy = self.wheel_cell_remainder.1.trunc();
                self.wheel_cell_remainder.0 -= dx;
                self.wheel_cell_remainder.1 -= dy;
                (dx as i16, dy as i16)
            }
        };
        if dx != 0 {
            self.pointer_input(PointerKind::Wheel { dx, dy: 0 }, None);
        }
        if dy != 0 {
            self.pointer_input(PointerKind::Wheel { dx: 0, dy }, None);
        }
    }

    fn focus_changed(&mut self, focused: bool) {
        self.window_focused = focused;
        if !focused {
            self.cancel_and_disable_ime();
            self.pressed_pointer_buttons.clear();
            self.modifiers = ModifiersState::empty();
            self.wheel_cell_remainder = (0.0, 0.0);
        }
        let input = if focused {
            InputEvent::FocusGained
        } else {
            InputEvent::FocusLost
        };
        self.send_input(input, false, Instant::now());
    }

    fn sync_ime(&mut self, scene: &WorkspaceScene) {
        let Some(window) = &self.window else {
            return;
        };
        if !self.window_focused {
            return;
        }
        let Some(text_input) = &scene.text_input else {
            if self.ime_allowed {
                window.set_ime_allowed(false);
                self.ime_allowed = false;
            }
            return;
        };
        let Some(gpu) = &self.gpu else {
            return;
        };
        if !self.ime_allowed {
            window.set_ime_allowed(true);
            self.ime_allowed = true;
        }
        window.set_ime_cursor_area(
            PhysicalPosition::new(
                (f32::from(text_input.area.x) * gpu.cell_w()).round() as i32,
                (f32::from(text_input.area.y) * gpu.cell_h()).round() as i32,
            ),
            PhysicalSize::new(
                (f32::from(text_input.area.width.max(1)) * gpu.cell_w()).round() as u32,
                gpu.cell_h().round() as u32,
            ),
        );
    }

    fn cancel_and_disable_ime(&mut self) {
        if self.ime_allowed {
            if let Some(window) = &self.window {
                window.set_ime_allowed(false);
            }
            self.ime_allowed = false;
        }
        self.host
            .handle_input(InputEvent::Composition(CompositionEvent::Cancel));
    }

    fn cancel_pointer_gesture(&mut self) {
        self.pressed_pointer_buttons.clear();
        self.host.cancel_pointer_gesture();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = match event_loop
            .create_window(Window::default_attributes().with_title("Mandatum GPU Host Spike"))
        {
            Ok(window) => {
                #[cfg(target_os = "macos")]
                window.set_option_as_alt(OptionAsAlt::OnlyRight);
                std::sync::Arc::new(window)
            }
            Err(error) => {
                self.fatal_error = Some(format!("no window (headless?): {error}"));
                self.print_summary();
                event_loop.exit();
                return;
            }
        };
        let gpu = match pollster::block_on(GpuText::new(
            window.clone(),
            self.config.text_settings.clone(),
        )) {
            Ok(gpu) => gpu,
            Err(error) => {
                self.fatal_error = Some(error);
                self.print_summary();
                event_loop.exit();
                return;
            }
        };
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.resize_host();

        self.start = Instant::now();
        self.next_heartbeat = self.start + HEARTBEAT;
        self.next_inject = self.start + Duration::from_millis(400);
        self.deadline = self
            .config
            .exit_after
            .map(|seconds| self.start + Duration::from_secs_f64(seconds));
        if self.deadline.is_none() && self.config.typing_bench {
            self.deadline =
                Some(self.next_inject + INJECT_INTERVAL * INJECT_TOTAL + Duration::from_secs(2));
        }
        if self.config.flood {
            self.send_input(
                InputEvent::Paste("seq 1 200000\n".to_owned()),
                false,
                Instant::now(),
            );
        }
        self.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.cancel_and_disable_ime();
                self.host.shutdown();
                self.print_summary();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                let more_pending = self.drain_runtime();
                if self.exit_if_requested(event_loop) {
                    return;
                }
                if let Err(error) = self.render_frame() {
                    self.fatal_error = Some(format!("unsupported GPU spike scene: {error}"));
                    self.cancel_and_disable_ime();
                    self.host.shutdown();
                    self.print_summary();
                    event_loop.exit();
                    return;
                }
                if more_pending {
                    self.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                self.scene_presentable = false;
                self.cancel_pointer_gesture();
                self.host.suspend_scene_interaction();
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize_surface(size.width, size.height);
                }
                self.refresh_mouse_cell();
                self.resize_host();
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.apply_scale_factor(scale_factor as f32);
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Ime(Ime::Disabled)
                if ime_event_is_accepted(self.window_focused, self.ime_allowed) =>
            {
                self.ime_allowed = false;
                self.send_input(
                    InputEvent::Composition(CompositionEvent::Cancel),
                    false,
                    Instant::now(),
                );
            }
            WindowEvent::Ime(ime)
                if ime_event_is_accepted(self.window_focused, self.ime_allowed) =>
            {
                if let Some(composition) = translate_ime(ime) {
                    self.send_input(InputEvent::Composition(composition), false, Instant::now());
                }
            }
            WindowEvent::Ime(_) => {}
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let now = Instant::now();
                let key = key_for_platform_translation(
                    &event.logical_key,
                    &event.key_without_modifiers(),
                    self.modifiers,
                );
                match translate_key(&key, self.modifiers) {
                    PlatformAction::Input(input) => {
                        let measured = matches!(
                            input,
                            InputEvent::Key(InputKey {
                                code: KeyCode::Char(_),
                                mods: Modifiers {
                                    control: false,
                                    alt: false,
                                    super_key: false,
                                    ..
                                },
                            })
                        );
                        self.send_input(input, measured, now);
                    }
                    PlatformAction::PasteShortcut(shortcut) => {
                        if self.host.handles_workspace_key(shortcut) {
                            self.send_input(InputEvent::Key(shortcut), false, now);
                        } else if let Some(clipboard) = &mut self.clipboard {
                            match clipboard.get_text() {
                                Ok(text) => {
                                    self.send_input(InputEvent::Paste(text), false, now);
                                }
                                Err(error) => self.host.report_platform_error(format!(
                                    "clipboard read failed: {error}"
                                )),
                            }
                            self.request_redraw();
                        } else {
                            self.host.report_platform_error(
                                "clipboard read failed: clipboard unavailable",
                            );
                            self.request_redraw();
                        }
                    }
                    PlatformAction::CopyShortcut(shortcut) => {
                        if self.host.handles_workspace_key(shortcut) {
                            self.send_input(InputEvent::Key(shortcut), false, now);
                        } else {
                            self.host.copy_selection();
                            self.apply_effects();
                            self.request_redraw();
                        }
                    }
                    PlatformAction::Ignore => {}
                }
                self.exit_if_requested(event_loop);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.update_mouse_cell(position.x, position.y);
                self.pointer_motion();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = neutral_button(button) {
                    self.pointer_button(state, button);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.pointer_wheel(delta),
            WindowEvent::Focused(focused) => self.focus_changed(focused),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self
            .scale_probe_at
            .is_some_and(|scale_probe_at| now >= scale_probe_at)
        {
            self.scale_probe_at = None;
            self.scale_probe_applied = true;
            self.apply_scale_factor(self.config.scale_factor);
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.cancel_and_disable_ime();
            self.host.shutdown();
            self.print_summary();
            event_loop.exit();
            return;
        }
        if now >= self.next_heartbeat {
            self.host.heartbeat();
            self.next_heartbeat = now + HEARTBEAT;
            self.request_redraw();
        }
        self.maybe_inject(now);

        let mut next = self.deadline.map_or(self.next_heartbeat, |deadline| {
            deadline.min(self.next_heartbeat)
        });
        if let Some(scale_probe_at) = self.scale_probe_at {
            next = next.min(scale_probe_at);
        }
        if self.config.typing_bench && self.injected < INJECT_TOTAL {
            next = next.min(self.next_inject);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.cancel_and_disable_ime();
        self.host.shutdown();
        self.print_summary();
    }
}

fn neutral_modifiers(modifiers: ModifiersState) -> Modifiers {
    Modifiers {
        shift: modifiers.shift_key(),
        control: modifiers.control_key(),
        alt: modifiers.alt_key(),
        super_key: modifiers.super_key(),
    }
}

fn neutral_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Right => Some(PointerButton::Right),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => None,
    }
}

fn scene_size_from_metrics(
    width: u32,
    height: u32,
    cell_width: f32,
    cell_height: f32,
) -> Option<SceneSize> {
    if !cell_width.is_finite()
        || !cell_height.is_finite()
        || cell_width <= 0.0
        || cell_height <= 0.0
    {
        return None;
    }
    let columns = (width as f32 / cell_width).floor() as u16;
    let rows = (height as f32 / cell_height).floor() as u16;
    // One pane needs a 3x3 bordered interior between the one-row header and
    // status strips. Suspend scene production while a minimized/tiny window
    // cannot satisfy that structural contract.
    (columns >= 3 && rows >= 5).then(|| SceneSize::new(columns, rows))
}

fn translate_key(key: &Key, modifiers: ModifiersState) -> PlatformAction {
    let mods = neutral_modifiers(modifiers);
    let exact_platform_shortcut = mods.super_key && !mods.shift && !mods.control && !mods.alt;
    if exact_platform_shortcut
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("v"))
    {
        return PlatformAction::PasteShortcut(InputKey::new(KeyCode::Char('v'), mods));
    }
    if exact_platform_shortcut
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("c"))
    {
        return PlatformAction::CopyShortcut(InputKey::new(KeyCode::Char('c'), mods));
    }
    if let Key::Character(value) = key
        && value.chars().nth(1).is_some()
    {
        return if !mods.control && !mods.alt && !mods.super_key {
            PlatformAction::Input(InputEvent::Composition(CompositionEvent::Commit(
                value.to_string(),
            )))
        } else {
            PlatformAction::Ignore
        };
    }

    let code = match key {
        Key::Named(named) => named_key_code(*named, mods.shift),
        Key::Character(value) => value.chars().next().map(|character| {
            let character = if mods.shift && character.is_ascii_lowercase() {
                character.to_ascii_uppercase()
            } else {
                character
            };
            KeyCode::Char(character)
        }),
        _ => None,
    };
    code.map_or(PlatformAction::Ignore, |code| {
        PlatformAction::Input(InputEvent::Key(InputKey::new(code, mods)))
    })
}

fn translate_ime(ime: Ime) -> Option<CompositionEvent> {
    match ime {
        Ime::Enabled => None,
        Ime::Disabled => Some(CompositionEvent::Cancel),
        Ime::Commit(text) => Some(CompositionEvent::Commit(text)),
        Ime::Preedit(text, cursor) => {
            let cursor = match cursor {
                Some((start, end)) => match TextRange::new(&text, start, end) {
                    Some(range) => Some(range),
                    None => return Some(CompositionEvent::Cancel),
                },
                None => None,
            };
            Some(CompositionEvent::Preedit { text, cursor })
        }
    }
}

fn ime_event_is_accepted(window_focused: bool, ime_allowed: bool) -> bool {
    window_focused && ime_allowed
}

fn named_key_code(key: NamedKey, shift: bool) -> Option<KeyCode> {
    Some(match key {
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Escape => KeyCode::Escape,
        NamedKey::Backspace => KeyCode::Backspace,
        NamedKey::Space => KeyCode::Char(' '),
        NamedKey::Tab if shift => KeyCode::BackTab,
        NamedKey::Tab => KeyCode::Tab,
        NamedKey::ArrowUp => KeyCode::Up,
        NamedKey::ArrowDown => KeyCode::Down,
        NamedKey::ArrowLeft => KeyCode::Left,
        NamedKey::ArrowRight => KeyCode::Right,
        NamedKey::Home => KeyCode::Home,
        NamedKey::End => KeyCode::End,
        NamedKey::PageUp => KeyCode::PageUp,
        NamedKey::PageDown => KeyCode::PageDown,
        NamedKey::Insert => KeyCode::Insert,
        NamedKey::Delete => KeyCode::Delete,
        NamedKey::F1 => KeyCode::Function(1),
        NamedKey::F2 => KeyCode::Function(2),
        NamedKey::F3 => KeyCode::Function(3),
        NamedKey::F4 => KeyCode::Function(4),
        NamedKey::F5 => KeyCode::Function(5),
        NamedKey::F6 => KeyCode::Function(6),
        NamedKey::F7 => KeyCode::Function(7),
        NamedKey::F8 => KeyCode::Function(8),
        NamedKey::F9 => KeyCode::Function(9),
        NamedKey::F10 => KeyCode::Function(10),
        NamedKey::F11 => KeyCode::Function(11),
        NamedKey::F12 => KeyCode::Function(12),
        NamedKey::F13 => KeyCode::Function(13),
        NamedKey::F14 => KeyCode::Function(14),
        NamedKey::F15 => KeyCode::Function(15),
        NamedKey::F16 => KeyCode::Function(16),
        NamedKey::F17 => KeyCode::Function(17),
        NamedKey::F18 => KeyCode::Function(18),
        NamedKey::F19 => KeyCode::Function(19),
        NamedKey::F20 => KeyCode::Function(20),
        NamedKey::F21 => KeyCode::Function(21),
        NamedKey::F22 => KeyCode::Function(22),
        NamedKey::F23 => KeyCode::Function(23),
        NamedKey::F24 => KeyCode::Function(24),
        _ => return None,
    })
}

fn run_exit_code(fatal_error: Option<&str>) -> i32 {
    if fatal_error.is_some() { 2 } else { 0 }
}

fn scene_is_suspended_by_tiled_minimum(scene: &WorkspaceScene) -> bool {
    scene.panes.iter().any(|pane| {
        pane_geometry_is_suspended(
            pane.floating,
            pane.area.width,
            pane.area.height,
            scene.size.width,
            scene.size.height,
        )
    })
}

fn pane_geometry_is_suspended(
    floating: bool,
    pane_width: u16,
    pane_height: u16,
    frame_width: u16,
    frame_height: u16,
) -> bool {
    let unusable = pane_width < 3 || pane_height < 3;
    unusable && (!floating || frame_width < 11 || frame_height < 9)
}

fn key_for_platform_translation(
    logical: &Key,
    without_modifiers: &Key,
    modifiers: ModifiersState,
) -> Key {
    if !(modifiers.alt_key() || modifiers.super_key()) {
        return logical.clone();
    }
    if !modifiers.shift_key() {
        return without_modifiers.clone();
    }
    // winit exposes a fully modified logical key and a key with every
    // modifier removed, but no "remove Option, preserve Shift" value. Rebuild
    // the xterm ASCII Shift layer here so macOS Option remains Meta instead of
    // producing alternate/dead characters. Non-ASCII composition stays Phase 5.
    match without_modifiers {
        Key::Character(value) => {
            let shifted: String = value.chars().map(shift_meta_character).collect();
            Key::Character(shifted.into())
        }
        _ => without_modifiers.clone(),
    }
}

fn shift_meta_character(character: char) -> char {
    match character {
        '`' => '~',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        ascii if ascii.is_ascii_lowercase() => ascii.to_ascii_uppercase(),
        other => other,
    }
}

fn main() {
    let config = parse_config();
    if let Some(seconds) = config.exit_after {
        std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs_f64(seconds + 8.0));
                println!(
                    "{{\"error\":\"watchdog fired\",\"notes\":\"event loop did not exit within budget\"}}"
                );
                std::process::exit(1);
            })
            .ok();
    }

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            println!(
                "{{\"error\":{:?},\"notes\":\"no event loop (headless environment)\"}}",
                error.to_string()
            );
            std::process::exit(2);
        }
    };
    let app_config = match AppConfig::from_current_dir() {
        Ok(config) => config,
        Err(error) => {
            println!(
                "{{\"error\":{:?},\"notes\":\"could not construct the real workstation host\"}}",
                error.to_string()
            );
            std::process::exit(2);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(config, proxy, app_config);
    if let Err(error) = event_loop.run_app(&mut app) {
        app.fatal_error = Some(format!("event loop error: {error}"));
    }
    // `process::exit` skips Drop, so finalize explicitly before deriving a
    // nonzero process status from any recoverable event-loop failure.
    app.cancel_and_disable_ime();
    app.host.shutdown();
    app.print_summary();
    let exit_code = run_exit_code(app.fatal_error.as_deref());
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PlatformAction, PressedPointerButtons, ime_event_is_accepted, key_for_platform_translation,
        pane_geometry_is_suspended, parse_font_family, parse_font_size, parse_scale_delay,
        parse_scale_factor, run_exit_code, scene_size_from_metrics, translate_ime, translate_key,
    };
    use mandatum_scene::input::{
        CompositionEvent, InputEvent, Key as InputKey, KeyCode, Modifiers, TextRange,
    };
    use winit::{
        event::Ime,
        keyboard::{Key, ModifiersState, NamedKey},
    };

    #[test]
    fn winit_key_is_neutral_before_it_reaches_the_host() {
        let key = Key::Character("p".into());
        let modifiers = ModifiersState::CONTROL;
        let super::PlatformAction::Input(input) = translate_key(&key, modifiers) else {
            panic!("control key did not become neutral input");
        };
        assert_eq!(
            input,
            InputEvent::Key(InputKey::new(KeyCode::Char('p'), Modifiers::CTRL))
        );
    }

    #[test]
    fn ime_events_translate_without_paste_or_scalar_truncation() {
        assert!(ime_event_is_accepted(true, true));
        assert!(!ime_event_is_accepted(false, true));
        assert!(!ime_event_is_accepted(true, false));
        assert_eq!(translate_ime(Ime::Enabled), None);
        assert_eq!(
            translate_ime(Ime::Preedit("e\u{301}".into(), Some((0, 3)))),
            Some(CompositionEvent::Preedit {
                text: "e\u{301}".into(),
                cursor: Some(TextRange { start: 0, end: 3 }),
            })
        );
        assert_eq!(
            translate_ime(Ime::Commit("啊不👩\u{200d}💻".into())),
            Some(CompositionEvent::Commit("啊不👩\u{200d}💻".into()))
        );
        assert_eq!(translate_ime(Ime::Disabled), Some(CompositionEvent::Cancel));
        assert_eq!(
            translate_ime(Ime::Preedit("é".into(), Some((1, 2)))),
            Some(CompositionEvent::Cancel),
            "invalid UTF-8 byte boundaries fail closed"
        );
        let PlatformAction::Input(input) = translate_key(
            &Key::Character("e\u{301}👩\u{200d}💻".into()),
            ModifiersState::empty(),
        ) else {
            panic!("multi-scalar logical key did not become neutral input");
        };
        assert_eq!(
            input,
            InputEvent::Composition(CompositionEvent::Commit("e\u{301}👩\u{200d}💻".into()))
        );
        assert!(matches!(
            translate_key(&Key::Character("ab".into()), ModifiersState::CONTROL),
            PlatformAction::Ignore
        ));
    }

    #[test]
    fn fatal_runs_return_nonzero_and_clean_runs_return_zero() {
        assert_eq!(run_exit_code(Some("unsupported scene")), 2);
        assert_eq!(run_exit_code(None), 0);
        assert_eq!(parse_scale_delay("2"), Some(2.0));
        assert_eq!(parse_scale_delay("-1"), None);
        assert_eq!(parse_scale_delay("NaN"), None);
        assert_eq!(parse_scale_factor("1.5"), Some(1.5));
        assert_eq!(parse_scale_factor("0"), None);
        assert_eq!(parse_scale_factor("inf"), None);
        assert_eq!(
            parse_font_family("Berkeley Mono").as_deref(),
            Some("Berkeley Mono")
        );
        assert_eq!(parse_font_family(" \n "), None);
        assert_eq!(parse_font_size("15.5"), Some(15.5));
        assert_eq!(parse_font_size("5"), None);
        assert_eq!(parse_font_size("73"), None);
        assert_eq!(parse_font_size("NaN"), None);
    }

    #[test]
    fn native_key_translation_covers_backtab_alt_super_and_extended_functions() {
        let cases = [
            (
                Key::Named(NamedKey::Tab),
                ModifiersState::SHIFT,
                InputKey::new(
                    KeyCode::BackTab,
                    Modifiers {
                        shift: true,
                        ..Modifiers::NONE
                    },
                ),
            ),
            (
                Key::Character("x".into()),
                ModifiersState::ALT,
                InputKey::new(KeyCode::Char('x'), Modifiers::ALT),
            ),
            (
                Key::Character("x".into()),
                ModifiersState::ALT | ModifiersState::SHIFT,
                InputKey::new(
                    KeyCode::Char('X'),
                    Modifiers {
                        alt: true,
                        shift: true,
                        ..Modifiers::NONE
                    },
                ),
            ),
            (
                Key::Character("!".into()),
                ModifiersState::ALT | ModifiersState::SHIFT,
                InputKey::new(
                    KeyCode::Char('!'),
                    Modifiers {
                        alt: true,
                        shift: true,
                        ..Modifiers::NONE
                    },
                ),
            ),
            (
                Key::Named(NamedKey::Space),
                ModifiersState::empty(),
                InputKey::plain(KeyCode::Char(' ')),
            ),
            (
                Key::Named(NamedKey::F24),
                ModifiersState::empty(),
                InputKey::plain(KeyCode::Function(24)),
            ),
        ];
        for (platform, modifiers, expected) in cases {
            let PlatformAction::Input(InputEvent::Key(actual)) =
                translate_key(&platform, modifiers)
            else {
                panic!("native key did not become neutral input");
            };
            assert_eq!(actual, expected);
        }

        let PlatformAction::PasteShortcut(shortcut) =
            translate_key(&Key::Character("v".into()), ModifiersState::SUPER)
        else {
            panic!("Command+V did not retain its neutral key for chord preflight");
        };
        assert_eq!(
            shortcut,
            InputKey::new(
                KeyCode::Char('v'),
                Modifiers {
                    super_key: true,
                    ..Modifiers::NONE
                }
            )
        );

        let PlatformAction::Input(InputEvent::Key(modified_super)) = translate_key(
            &Key::Character("C".into()),
            ModifiersState::SUPER | ModifiersState::SHIFT,
        ) else {
            panic!("modified Command+C incorrectly used the native copy fallback");
        };
        assert_eq!(
            modified_super,
            InputKey::new(
                KeyCode::Char('C'),
                Modifiers {
                    shift: true,
                    super_key: true,
                    ..Modifiers::NONE
                }
            )
        );

        assert_eq!(
            key_for_platform_translation(
                &Key::Character("¡".into()),
                &Key::Character("1".into()),
                ModifiersState::ALT | ModifiersState::SHIFT,
            ),
            Key::Character("!".into())
        );

        let PlatformAction::CopyShortcut(shortcut) =
            translate_key(&Key::Character("c".into()), ModifiersState::SUPER)
        else {
            panic!("Command+C did not retain its neutral key for chord preflight");
        };
        assert_eq!(
            shortcut,
            InputKey::new(
                KeyCode::Char('c'),
                Modifiers {
                    super_key: true,
                    ..Modifiers::NONE
                }
            )
        );
    }

    #[test]
    fn pressed_pointer_state_distinguishes_drag_from_motion_and_resets() {
        let mut buttons = PressedPointerButtons::default();
        assert_eq!(buttons.active(), None);
        buttons.set(mandatum_scene::input::PointerButton::Left, true);
        assert_eq!(
            buttons.active(),
            Some(mandatum_scene::input::PointerButton::Left)
        );
        buttons.set(mandatum_scene::input::PointerButton::Left, false);
        assert_eq!(buttons.active(), None);
        buttons.set(mandatum_scene::input::PointerButton::Right, true);
        assert_eq!(
            buttons.all(),
            vec![mandatum_scene::input::PointerButton::Right]
        );
        buttons.clear();
        assert_eq!(buttons.active(), None);
    }

    #[test]
    fn pixel_metrics_suspend_tiny_frames_and_recompute_grid_after_scale() {
        assert_eq!(
            scene_size_from_metrics(800, 600, 10.0, 20.0),
            Some(mandatum_scene::SceneSize::new(80, 30))
        );
        assert_eq!(
            scene_size_from_metrics(800, 600, 20.0, 40.0),
            Some(mandatum_scene::SceneSize::new(40, 15))
        );
        assert_eq!(scene_size_from_metrics(20, 40, 10.0, 20.0), None);
        assert_eq!(scene_size_from_metrics(800, 600, 0.0, 20.0), None);
        assert!(pane_geometry_is_suspended(false, 2, 3, 80, 24));
        assert!(!pane_geometry_is_suspended(false, 3, 3, 80, 24));
        assert!(pane_geometry_is_suspended(true, 2, 3, 10, 8));
        assert!(!pane_geometry_is_suspended(true, 2, 3, 80, 24));
    }
}
