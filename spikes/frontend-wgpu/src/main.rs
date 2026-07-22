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
use mandatum_gpu_renderer_spike::{GpuText, UnsupportedScene};
use mandatum_scene::{
    SceneSize,
    input::{
        InputEvent, Key as InputKey, KeyCode, Modifiers, PointerButton, PointerEvent, PointerKind,
    },
};
use stats::Samples;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
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
}

fn parse_config() -> Config {
    let mut config = Config {
        exit_after: None,
        typing_bench: false,
        flood: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-after" => {
                config.exit_after = args.next().and_then(|value| value.parse().ok());
            }
            "--typing-bench" => config.typing_bench = true,
            "--flood" => config.flood = true,
            other => eprintln!("ignoring unknown arg: {other}"),
        }
    }
    config
}

#[derive(Debug)]
enum UserEvent {
    Wake,
}

enum PlatformAction {
    Input(InputEvent),
    Paste,
    Ignore,
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
    mouse_cell: (u16, u16),
}

impl App {
    fn new(config: Config, proxy: EventLoopProxy<UserEvent>, mut app_config: AppConfig) -> Self {
        // Phase 2 proves one fresh terminal slice. Full restore/overlay parity
        // remains a Phase 3 concern, but the real host still owns the policy.
        app_config.restore_on_startup = false;
        let wake_proxy = proxy.clone();
        let host = FrontendHost::new_with_wake_callback(app_config, move || {
            let _ = wake_proxy.send_event(UserEvent::Wake);
        });
        let now = Instant::now();
        Self {
            config,
            host,
            window: None,
            gpu: None,
            clipboard: arboard::Clipboard::new().ok(),
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
            mouse_cell: (0, 0),
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
        Some(SceneSize::new(
            ((width as f32 / gpu.cell_w()).floor() as u16).max(1),
            ((height as f32 / gpu.cell_h()).floor() as u16).max(1),
        ))
    }

    fn resize_host(&mut self) {
        if let Some(size) = self.scene_size() {
            self.host.handle_input(InputEvent::Resize(size));
        }
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
                        let _ = clipboard.set_text(text);
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

    fn render_frame(&mut self) -> Result<(), UnsupportedScene> {
        let Some(size) = self.scene_size() else {
            return Ok(());
        };
        let snapshot = self.host.frame(size);
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        if let Some(present) = gpu.render(&snapshot.scene, &snapshot.theme)? {
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
            "host=FrontendHost present=Fifo(vsync) typing_bench={} flood={} input_samples={} frame_samples={}",
            self.config.typing_bench,
            self.config.flood,
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
        self.host.shutdown();
        self.print_summary();
        event_loop.exit();
        true
    }

    fn update_mouse_cell(&mut self, x: f64, y: f64) {
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

    fn pointer_input(&mut self, kind: PointerKind, button: Option<PointerButton>) {
        let (column, row) = self.mouse_cell;
        let input = InputEvent::Pointer(PointerEvent {
            kind,
            button,
            column,
            row,
            mods: neutral_modifiers(self.modifiers),
        });
        self.send_input(input, false, Instant::now());
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
            Ok(window) => std::sync::Arc::new(window),
            Err(error) => {
                self.fatal_error = Some(format!("no window (headless?): {error}"));
                self.print_summary();
                event_loop.exit();
                return;
            }
        };
        let gpu = match pollster::block_on(GpuText::new(window.clone())) {
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
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize_surface(size.width, size.height);
                }
                self.resize_host();
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.set_scale(scale_factor as f32);
                }
                self.resize_host();
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let now = Instant::now();
                match translate_key(&event.logical_key, self.modifiers) {
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
                    PlatformAction::Paste => {
                        if let Some(clipboard) = &mut self.clipboard
                            && let Ok(text) = clipboard.get_text()
                        {
                            self.send_input(InputEvent::Paste(text), false, now);
                        }
                    }
                    PlatformAction::Ignore => {}
                }
                self.exit_if_requested(event_loop);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.update_mouse_cell(position.x, position.y);
                self.pointer_input(PointerKind::Move, None);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = neutral_button(button) {
                    let kind = match state {
                        ElementState::Pressed => PointerKind::Down,
                        ElementState::Released => PointerKind::Up,
                    };
                    self.pointer_input(kind, Some(button));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -(y * 3.0).round() as i16,
                    MouseScrollDelta::PixelDelta(position) => {
                        let cell_height = self.gpu.as_ref().map_or(16.0, |gpu| gpu.cell_h());
                        -(position.y / f64::from(cell_height)).round() as i16
                    }
                };
                if dy != 0 {
                    self.pointer_input(PointerKind::Wheel { dx: 0, dy }, None);
                }
            }
            WindowEvent::Focused(true) => {
                self.send_input(InputEvent::FocusGained, false, Instant::now());
            }
            WindowEvent::Focused(false) => {
                self.send_input(InputEvent::FocusLost, false, Instant::now());
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.deadline.is_some_and(|deadline| now >= deadline) {
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
        if self.config.typing_bench && self.injected < INJECT_TOTAL {
            next = next.min(self.next_inject);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
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

fn translate_key(key: &Key, modifiers: ModifiersState) -> PlatformAction {
    let mods = neutral_modifiers(modifiers);
    if mods.super_key && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("v")) {
        return PlatformAction::Paste;
    }
    if mods.super_key && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("c")) {
        return PlatformAction::Ignore;
    }

    let code = match key {
        Key::Named(named) => named_key_code(*named, mods.shift),
        Key::Character(value) => value.chars().next().map(KeyCode::Char),
        _ => None,
    };
    code.map_or(PlatformAction::Ignore, |code| {
        PlatformAction::Input(InputEvent::Key(InputKey::new(code, mods)))
    })
}

fn named_key_code(key: NamedKey, shift: bool) -> Option<KeyCode> {
    Some(match key {
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Escape => KeyCode::Escape,
        NamedKey::Backspace => KeyCode::Backspace,
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
        _ => return None,
    })
}

fn run_exit_code(fatal_error: Option<&str>) -> i32 {
    if fatal_error.is_some() { 2 } else { 0 }
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
    app.print_summary();
    let exit_code = run_exit_code(app.fatal_error.as_deref());
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::{run_exit_code, translate_key};
    use mandatum_scene::input::{InputEvent, Key as InputKey, KeyCode, Modifiers};
    use winit::keyboard::{Key, ModifiersState};

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
    fn fatal_runs_return_nonzero_and_clean_runs_return_zero() {
        assert_eq!(run_exit_code(Some("unsupported scene")), 2);
        assert_eq!(run_exit_code(None), 0);
    }
}
