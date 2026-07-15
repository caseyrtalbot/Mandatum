//! mandatum-frontend-wgpu-spike
//!
//! A GPU terminal frontend spike over the Mandatum engine crates. It spawns the
//! user's shell through `mandatum-pty`, parses output with
//! `mandatum-terminal-vt`, and renders the grid with wgpu + glyphon. The point
//! is measurement: it instruments input-to-present latency and scroll frame
//! time and prints a JSON summary so the GPU path can be judged on numbers, not
//! vibes.

mod scene_bridge;
mod stats;
mod terminal;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use mandatum_gpu_renderer_spike::{GpuText, UnsupportedScene};
use stats::Samples;
use terminal::{Selection, TerminalSession};

const INJECT_TOTAL: u32 = 300;
const INJECT_INTERVAL: Duration = Duration::from_micros(33_333); // 30/sec
const IDLE_FRAME_CUTOFF_MS: f64 = 250.0;

#[derive(Clone)]
struct Config {
    exit_after: Option<f64>,
    typing_bench: bool,
    flood: bool,
}

fn parse_config() -> Config {
    let mut cfg = Config {
        exit_after: None,
        typing_bench: false,
        flood: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-after" => {
                cfg.exit_after = args.next().and_then(|v| v.parse().ok());
            }
            "--typing-bench" => cfg.typing_bench = true,
            "--flood" => cfg.flood = true,
            other => eprintln!("ignoring unknown arg: {other}"),
        }
    }
    cfg
}

#[derive(Debug)]
enum UserEvent {
    Wake,
}

/// One decoded keyboard action in the shared input path.
enum KeyAction {
    Bytes(Vec<u8>),
    Paste,
    Copy,
    Scroll(isize),
    Ignore,
}

struct App {
    config: Config,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<std::sync::Arc<Window>>,
    gpu: Option<GpuText>,
    session: Option<TerminalSession>,
    clipboard: Option<arboard::Clipboard>,

    // Instrumentation.
    input_to_present: Samples,
    frame_ms: Samples,
    pending_inputs: VecDeque<Instant>,
    dirty_from_pty: bool,
    last_present: Option<Instant>,
    dropped_correlations: u32,

    // Lifecycle.
    start: Instant,
    deadline: Option<Instant>,
    summary_printed: bool,
    fatal_error: Option<String>,

    // Bench + input state.
    injected: u32,
    next_inject: Instant,
    inject_letter: u8,
    modifiers: ModifiersState,
    mouse_px: (f64, f64),
    selecting: bool,
    sel_anchor: Option<(isize, u16)>,
    selection: Option<Selection>,
    live_latency_ms: f64,
    /// Coalesces reader-thread wakes: set by the reader, cleared on `user_event`.
    /// Prevents a byte flood from drowning the loop in proxy events.
    wake_pending: Arc<AtomicBool>,
}

impl App {
    fn new(config: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        let now = Instant::now();
        App {
            config,
            proxy,
            window: None,
            gpu: None,
            session: None,
            clipboard: arboard::Clipboard::new().ok(),
            input_to_present: Samples::new(),
            frame_ms: Samples::new(),
            pending_inputs: VecDeque::new(),
            dirty_from_pty: false,
            last_present: None,
            dropped_correlations: 0,
            start: now,
            deadline: None,
            summary_printed: false,
            fatal_error: None,
            injected: 0,
            next_inject: now,
            inject_letter: b'a',
            modifiers: ModifiersState::empty(),
            mouse_px: (0.0, 0.0),
            selecting: false,
            sel_anchor: None,
            selection: None,
            live_latency_ms: 0.0,
            wake_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Send bytes to the shell. When `measured`, stamp the receipt time so the
    /// next PTY-driven present can be correlated for latency.
    fn send_input(&mut self, bytes: &[u8], measured: bool, at: Instant) {
        if measured {
            if self.pending_inputs.len() < 64 {
                self.pending_inputs.push_back(at);
            } else {
                self.dropped_correlations += 1;
            }
        }
        if let Some(session) = &mut self.session {
            session.write_input(bytes);
        }
    }

    fn pixel_to_cell(&self, px: f64, py: f64) -> (isize, u16) {
        let (gpu, session) = match (&self.gpu, &self.session) {
            (Some(g), Some(s)) => (g, s),
            _ => return (0, 0),
        };
        let col =
            ((px / gpu.cell_w() as f64).floor() as i64).clamp(0, session.cols() as i64 - 1) as u16;
        let screen_row = (py / gpu.cell_h() as f64).floor() as isize;
        let abs = session.top_absolute_row() + screen_row;
        (abs, col)
    }

    fn status_line(&self) -> String {
        let session = self.session.as_ref().unwrap();
        let scroll = if session.scroll_offset() == 0 {
            "live".to_string()
        } else {
            format!("-{} rows", session.scroll_offset())
        };
        let fps = self.frame_ms.percentile(50.0);
        let fps = if fps > 0.0 { 1000.0 / fps } else { 0.0 };
        format!(
            "{}  {}x{}  {}  fps~{:.0}  lat p50 {:.1}ms p95 {:.1}ms  [scroll:wheel  copy:Cmd+C  paste:Cmd+V]",
            session.shell_name(),
            session.cols(),
            session.rows(),
            scroll,
            fps,
            self.input_to_present.percentile(50.0),
            self.input_to_present.percentile(95.0),
        )
    }

    fn recompute_grid_and_resize(&mut self) {
        let (gpu, session) = match (&mut self.gpu, &mut self.session) {
            (Some(g), Some(s)) => (g, s),
            _ => return,
        };
        let (w, h) = gpu.surface_size();
        let cols = ((w as f32 / gpu.cell_w()).floor() as u16).max(1);
        // Reserve one line for the status strip.
        let rows = ((h as f32 / gpu.cell_h()).floor() as u16)
            .saturating_sub(1)
            .max(1);
        session.resize(cols, rows);
    }

    fn render_frame(&mut self) -> Result<(), UnsupportedScene> {
        if self.session.is_none() || self.gpu.is_none() {
            return Ok(());
        }
        let status = self.status_line();
        let selection = self.selection;
        // Build the frontend-neutral scene, then paint from it. The renderer
        // never sees the session or grid: grid -> scene lives in scene_bridge.
        let scene = {
            let session = self.session.as_ref().unwrap();
            scene_bridge::build_scene(session, selection, &status)
        };
        let gpu = self.gpu.as_mut().unwrap();
        if let Some(present) = gpu.render(&scene)? {
            if let Some(last) = self.last_present {
                let d = present.duration_since(last).as_secs_f64() * 1000.0;
                if d < IDLE_FRAME_CUTOFF_MS {
                    self.frame_ms.push(d);
                }
            }
            self.last_present = Some(present);

            if self.dirty_from_pty {
                if let Some(t_in) = self.pending_inputs.pop_front() {
                    let latency = present.duration_since(t_in).as_secs_f64() * 1000.0;
                    self.input_to_present.push(latency);
                    self.live_latency_ms = latency;
                }
                self.dirty_from_pty = false;
            }
        }
        Ok(())
    }

    fn maybe_inject(&mut self, now: Instant) {
        if !self.config.typing_bench {
            return;
        }
        while self.injected < INJECT_TOTAL && now >= self.next_inject {
            // Every 40 chars, clear the input line (Ctrl+U) so it never wraps;
            // this keeps echo local and the latency measurement clean.
            if self.injected > 0 && self.injected % 40 == 0 {
                self.send_input(&[0x15], false, now);
            }
            let letter = self.inject_letter;
            self.inject_letter = if letter >= b'z' { b'a' } else { letter + 1 };
            self.send_input(&[letter], true, now);
            self.injected += 1;
            self.next_inject += INJECT_INTERVAL;
        }
    }

    fn print_summary(&mut self) {
        if self.summary_printed {
            return;
        }
        self.summary_printed = true;

        if let Some(err) = &self.fatal_error {
            println!(
                "{{\"error\":{:?},\"notes\":\"run failed; measurements are incomplete\"}}",
                err
            );
            return;
        }

        let mut notes = format!(
            "shell={} present=Fifo(vsync) typing_bench={} flood={} input_samples={} frame_samples={}",
            self.session
                .as_ref()
                .map(|s| s.shell_name().to_string())
                .unwrap_or_else(|| "?".into()),
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
            " method: input stamped at event receipt (real or synthetic), present timestamped after queue.submit+present; one pending input correlated per PTY-driven present (FIFO echo assumption); frame_ms is present-to-present interval filtered <250ms.",
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
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = match event_loop
            .create_window(Window::default_attributes().with_title("mandatum-frontend-wgpu-spike"))
        {
            Ok(w) => std::sync::Arc::new(w),
            Err(e) => {
                self.fatal_error = Some(format!("no window (headless?): {e}"));
                self.print_summary();
                event_loop.exit();
                return;
            }
        };

        let gpu = match pollster::block_on(GpuText::new(window.clone())) {
            Ok(g) => g,
            Err(e) => {
                self.fatal_error = Some(e);
                self.print_summary();
                event_loop.exit();
                return;
            }
        };

        // Grid size from the physical surface and measured cell metrics.
        let (w, h) = gpu.surface_size();
        let cols = ((w as f32 / gpu.cell_w()).floor() as u16).max(1);
        let rows = ((h as f32 / gpu.cell_h()).floor() as u16)
            .saturating_sub(1)
            .max(1);

        let proxy = self.proxy.clone();
        let wake_pending = self.wake_pending.clone();
        let session = match TerminalSession::spawn(cols, rows, move || {
            // Coalesce: only send a proxy event on a false->true transition, so a
            // byte flood produces at most one outstanding wake at a time.
            if !wake_pending.swap(true, Ordering::AcqRel) {
                let _ = proxy.send_event(UserEvent::Wake);
            }
        }) {
            Ok(s) => s,
            Err(e) => {
                self.fatal_error = Some(format!("pty spawn failed: {e}"));
                self.print_summary();
                event_loop.exit();
                return;
            }
        };

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.session = Some(session);

        // Establish the run deadline.
        self.start = Instant::now();
        self.next_inject = self.start + Duration::from_millis(400); // let the prompt settle
        self.deadline = self
            .config
            .exit_after
            .map(|s| self.start + Duration::from_secs_f64(s));
        if self.deadline.is_none() && self.config.typing_bench {
            self.deadline =
                Some(self.next_inject + INJECT_INTERVAL * INJECT_TOTAL + Duration::from_secs(2));
        }

        if self.config.flood {
            // Programmatic scroll-flood scenario.
            if let Some(session) = &mut self.session {
                session.write_input(b"seq 1 200000\n");
            }
        }

        self.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        // A wake means the reader has new bytes. Clear the coalescing latch and
        // schedule a redraw; the actual pump+render happens at vsync in
        // RedrawRequested, which keeps rendering paced to the display.
        self.wake_pending.store(false, Ordering::Release);
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.print_summary();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                // Drain all PTY bytes accumulated since the last frame, then
                // render once. While output keeps streaming, re-arm the redraw so
                // the loop runs at the display's refresh rate (one present per
                // vsync coalescing all data for that frame).
                let outcome =
                    self.session
                        .as_mut()
                        .map(|s| s.pump())
                        .unwrap_or(terminal::PumpOutcome {
                            screen_changed: false,
                            more_pending: false,
                        });
                if outcome.screen_changed {
                    self.dirty_from_pty = true;
                }
                if let Err(error) = self.render_frame() {
                    self.fatal_error = Some(format!("unsupported GPU spike scene: {error}"));
                    self.print_summary();
                    event_loop.exit();
                    return;
                }
                if outcome.more_pending {
                    self.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize_surface(size.width, size.height);
                }
                self.recompute_grid_and_resize();
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.set_scale(scale_factor as f32);
                }
                self.recompute_grid_and_resize();
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let now = Instant::now();
                match encode_key(&event, self.modifiers) {
                    KeyAction::Bytes(bytes) => {
                        let measured = is_printable(&bytes);
                        self.selection = None;
                        self.send_input(&bytes, measured, now);
                    }
                    KeyAction::Scroll(delta) => {
                        if let Some(session) = &mut self.session {
                            session.scroll(delta);
                        }
                        self.request_redraw();
                    }
                    KeyAction::Copy => {
                        self.copy_selection();
                    }
                    KeyAction::Paste => {
                        if let Some(clip) = &mut self.clipboard {
                            if let Ok(text) = clip.get_text() {
                                let bytes = text.into_bytes();
                                self.send_input(&bytes, false, now);
                            }
                        }
                    }
                    KeyAction::Ignore => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_px = (position.x, position.y);
                if self.selecting {
                    let focus = self.pixel_to_cell(position.x, position.y);
                    if let Some(anchor) = self.sel_anchor {
                        self.selection = Some(Selection {
                            start_row: anchor.0,
                            start_col: anchor.1,
                            end_row: focus.0,
                            end_col: focus.1,
                        });
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            let anchor = self.pixel_to_cell(self.mouse_px.0, self.mouse_px.1);
                            self.sel_anchor = Some(anchor);
                            self.selecting = true;
                            self.selection = None;
                            self.request_redraw();
                        }
                        ElementState::Released => {
                            self.selecting = false;
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0).round() as isize,
                    MouseScrollDelta::PixelDelta(pos) => {
                        let ch = self.gpu.as_ref().map(|g| g.cell_h()).unwrap_or(16.0) as f64;
                        (pos.y / ch).round() as isize
                    }
                };
                if rows != 0 {
                    if let Some(session) = &mut self.session {
                        session.scroll(rows);
                    }
                    self.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if let Some(deadline) = self.deadline {
            if now >= deadline {
                self.print_summary();
                event_loop.exit();
                return;
            }
        }
        self.maybe_inject(now);

        // Wake at the next scheduled moment (bench tick or deadline).
        let mut next: Option<Instant> = self.deadline;
        if self.config.typing_bench && self.injected < INJECT_TOTAL {
            next = Some(match next {
                Some(d) => d.min(self.next_inject),
                None => self.next_inject,
            });
        }
        match next {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.print_summary();
    }
}

impl App {
    fn copy_selection(&mut self) {
        let text = match (self.selection, &self.session) {
            (Some(sel), Some(session)) => {
                session.text_in_range(sel.start_row, sel.start_col, sel.end_row, sel.end_col)
            }
            _ => return,
        };
        if let Some(clip) = &mut self.clipboard {
            let _ = clip.set_text(text);
        }
    }
}

fn is_printable(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|&b| (0x20..0x7f).contains(&b))
}

/// Translate a winit key event into a terminal action using the same path for
/// real and synthetic input.
fn encode_key(event: &winit::event::KeyEvent, mods: ModifiersState) -> KeyAction {
    let sup = mods.super_key();
    let ctrl = mods.control_key();

    if sup {
        if let Key::Character(s) = &event.logical_key {
            match s.as_str() {
                "v" | "V" => return KeyAction::Paste,
                "c" | "C" => return KeyAction::Copy,
                _ => return KeyAction::Ignore,
            }
        }
        return KeyAction::Ignore;
    }

    match &event.logical_key {
        Key::Named(named) => match named {
            NamedKey::Enter => KeyAction::Bytes(vec![b'\r']),
            NamedKey::Backspace => KeyAction::Bytes(vec![0x7f]),
            NamedKey::Tab => KeyAction::Bytes(vec![b'\t']),
            NamedKey::Escape => KeyAction::Bytes(vec![0x1b]),
            NamedKey::Space => KeyAction::Bytes(vec![b' ']),
            NamedKey::ArrowUp => KeyAction::Bytes(vec![0x1b, b'[', b'A']),
            NamedKey::ArrowDown => KeyAction::Bytes(vec![0x1b, b'[', b'B']),
            NamedKey::ArrowRight => KeyAction::Bytes(vec![0x1b, b'[', b'C']),
            NamedKey::ArrowLeft => KeyAction::Bytes(vec![0x1b, b'[', b'D']),
            NamedKey::Home => KeyAction::Bytes(vec![0x1b, b'[', b'H']),
            NamedKey::End => KeyAction::Bytes(vec![0x1b, b'[', b'F']),
            NamedKey::Delete => KeyAction::Bytes(vec![0x1b, b'[', b'3', b'~']),
            NamedKey::PageUp => KeyAction::Scroll(10),
            NamedKey::PageDown => KeyAction::Scroll(-10),
            _ => KeyAction::Ignore,
        },
        Key::Character(s) => {
            if ctrl {
                if let Some(c) = s.chars().next() {
                    if c.is_ascii_alphabetic() {
                        return KeyAction::Bytes(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
                    }
                }
                return KeyAction::Ignore;
            }
            match &event.text {
                Some(text) => KeyAction::Bytes(text.as_bytes().to_vec()),
                None => KeyAction::Bytes(s.as_bytes().to_vec()),
            }
        }
        _ => match &event.text {
            Some(text) => KeyAction::Bytes(text.as_bytes().to_vec()),
            None => KeyAction::Ignore,
        },
    }
}

fn run_exit_code(fatal_error: Option<&str>) -> i32 {
    if fatal_error.is_some() { 2 } else { 0 }
}

fn main() {
    let config = parse_config();

    // Watchdog: guarantee the process never hangs a no-display or wedged run.
    if let Some(secs) = config.exit_after {
        std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs_f64(secs + 8.0));
                println!(
                    "{{\"error\":\"watchdog fired\",\"notes\":\"event loop did not exit within budget\"}}"
                );
                std::process::exit(1);
            })
            .ok();
    }

    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            println!(
                "{{\"error\":{:?},\"notes\":\"no event loop (headless environment)\"}}",
                e.to_string()
            );
            std::process::exit(2);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(config, proxy);
    if let Err(e) = event_loop.run_app(&mut app) {
        app.fatal_error = Some(format!("event loop error: {e}"));
    }
    app.print_summary();
    let exit_code = run_exit_code(app.fatal_error.as_deref());
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::run_exit_code;

    #[test]
    fn fatal_runs_return_nonzero_and_clean_runs_return_zero() {
        assert_eq!(run_exit_code(Some("unsupported scene")), 2);
        assert_eq!(run_exit_code(None), 0);
    }
}
