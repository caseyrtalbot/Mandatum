//! Native Mandatum product shell.
//!
//! Product state, runtimes, persistence, and scene composition stay behind
//! `FrontendHost`. This package owns only winit lifecycle/input, clipboard
//! integration, redraw scheduling, and native renderer recovery. Measurement,
//! stress, and synthetic fault tooling remain under `spikes/frontend-wgpu`.

mod input;

use std::time::{Duration, Instant};

use input::{
    PlatformAction, ime_event_is_accepted, key_for_platform_translation, neutral_button,
    neutral_modifiers, scene_is_suspended_by_tiled_minimum, translate_ime, translate_key,
    viewport_metrics_from_renderer,
};
use mandatum_app::{AppConfig, FrontendEffect, FrontendHost};
use mandatum_native_renderer::{
    BUNDLED_FAMILY, DEFAULT_FONT_SIZE, FontRequest, GpuRenderError, GpuRenderOutcome,
    GpuStartupError, GpuStartupErrorKind, GpuText, ResolvedFontProfile, cycle_candidate_families,
};
use mandatum_scene::{
    LogicalPoint, SceneSize, ViewportMetrics, WorkspaceScene,
    input::{CompositionEvent, InputEvent, PointerButton, PointerEvent, PointerKind},
};
#[cfg(target_os = "macos")]
use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseScrollDelta, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::ModifiersState,
    platform::modifier_supplement::KeyEventExtModifierSupplement,
    window::{Window, WindowId},
};

const HEARTBEAT: Duration = Duration::from_millis(250);
const EVENT_DRAIN_BUDGET: usize = 16;

trait VisualClock {
    fn now(&self) -> Instant;
}

struct MonotonicVisualClock;

impl VisualClock for MonotonicVisualClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

fn next_scheduled_deadline(heartbeat: Instant, animation: Option<Instant>) -> Instant {
    animation.map_or(heartbeat, |animation| animation.min(heartbeat))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NativeWindowGeometry {
    initial: LogicalSize<f64>,
    minimum: LogicalSize<f64>,
}

fn native_window_geometry() -> NativeWindowGeometry {
    NativeWindowGeometry {
        initial: LogicalSize::new(1_200.0, 800.0),
        minimum: LogicalSize::new(720.0, 480.0),
    }
}

fn native_window_title(project_label: &str) -> String {
    let project_label = project_label.trim();
    if project_label.is_empty() {
        "Mandatum".to_owned()
    } else {
        format!("Mandatum — {project_label}")
    }
}

fn next_native_window_title(current: &str, project_label: &str) -> Option<String> {
    let next = native_window_title(project_label);
    (next != current).then_some(next)
}

fn pointer_input_needs_redraw(
    kind: PointerKind,
    before: (bool, Option<usize>),
    after: (bool, Option<usize>),
) -> bool {
    kind != PointerKind::Move || before.0 || after.0 || before.1 != after.1
}

fn logical_pointer_position(x: f64, y: f64, backing_scale: f32) -> Option<LogicalPoint> {
    if !backing_scale.is_finite() || backing_scale <= 0.0 {
        return None;
    }
    let backing_scale = f64::from(backing_scale);
    LogicalPoint::from_pixels(x / backing_scale, y / backing_scale).ok()
}

/// Flags stay optional so an absent one defers to `[font]` in config rather
/// than overwriting it with the built-in default.
#[derive(Clone, Debug, Default, PartialEq)]
struct NativeLaunchOptions {
    font_family: Option<String>,
    font_size: Option<f32>,
    font_info: bool,
}

/// The `[font]` section of the loaded config, already validated for shape by
/// `mandatum_app::config` (non-empty family, finite positive size).
#[derive(Clone, Debug, Default, PartialEq)]
struct ConfiguredFont {
    family: Option<String>,
    size: Option<f32>,
}

fn parse_launch_options(
    args: impl IntoIterator<Item = String>,
) -> Result<NativeLaunchOptions, String> {
    let mut options = NativeLaunchOptions::default();
    let mut args = args.into_iter();
    while let Some(option) = args.next() {
        match option.as_str() {
            "--font-family" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--font-family requires a value".to_owned())?;
                if value.starts_with("--") {
                    return Err("--font-family requires a value".to_owned());
                }
                options.font_family = Some(value);
            }
            "--font-size" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--font-size requires a value".to_owned())?;
                options.font_size = Some(
                    value
                        .parse::<f32>()
                        .map_err(|_| format!("invalid --font-size value: {value}"))?,
                );
            }
            "--font-info" => options.font_info = true,
            _ => return Err(format!("unknown native option: {option}")),
        }
    }
    Ok(options)
}

/// CLI flag wins over the config value, which wins over the built-in default.
fn font_request(options: &NativeLaunchOptions, configured: &ConfiguredFont) -> FontRequest {
    let size = options
        .font_size
        .or(configured.size)
        .unwrap_or(DEFAULT_FONT_SIZE);
    match options
        .font_family
        .clone()
        .or_else(|| configured.family.clone())
    {
        Some(family) => FontRequest::installed(family, size),
        None => FontRequest::BundledDefault { size },
    }
}

/// A flag is explicit per-launch intent, so an unusable one still fails the
/// launch. A config value must never block launch (an uninstalled family or
/// an out-of-range size would otherwise brick a double-clicked app bundle),
/// so it degrades to the flag-or-default font and returns a warning for the
/// startup status line.
fn resolve_launch_font(
    options: &NativeLaunchOptions,
    configured: &ConfiguredFont,
) -> Result<(ResolvedFontProfile, Option<String>), String> {
    let error = match ResolvedFontProfile::resolve(font_request(options, configured)) {
        Ok(profile) => return Ok((profile, None)),
        Err(error) => error.to_string(),
    };
    let config_contributed = (options.font_family.is_none() && configured.family.is_some())
        || (options.font_size.is_none() && configured.size.is_some());
    if !config_contributed {
        return Err(error);
    }
    let profile = ResolvedFontProfile::resolve(font_request(options, &ConfiguredFont::default()))
        .map_err(|error| error.to_string())?;
    Ok((profile, Some(format!("config [font] ignored: {error}"))))
}

#[derive(Debug)]
enum FontPreflightOutcome<T> {
    Info(String),
    Launch(T),
}

fn launch_after_font_preflight<T>(
    args: impl IntoIterator<Item = String>,
    configured: &ConfiguredFont,
    construct_launch: impl FnOnce(Box<ResolvedFontProfile>, Option<String>) -> Result<T, String>,
) -> Result<FontPreflightOutcome<T>, String> {
    let options = parse_launch_options(args)?;
    let (profile, warning) = resolve_launch_font(&options, configured)?;
    if options.font_info {
        let json = serde_json::to_string(profile.info())
            .map_err(|error| format!("could not encode --font-info: {error}"))?;
        return Ok(FontPreflightOutcome::Info(json));
    }

    construct_launch(Box::new(profile), warning).map(FontPreflightOutcome::Launch)
}

#[derive(Debug)]
enum UserEvent {
    Wake,
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

    fn begin(&mut self, button: PointerButton, admitted: bool) -> bool {
        if !admitted || self.active().is_some() {
            return false;
        }
        self.set(button, true);
        true
    }
}

/// Construct product state only after every native rendering prerequisite.
///
/// `create_host` is the sole `FrontendHost`/`AppState`/runtime creation seam,
/// so any window or GPU error returns without restore or PTY side effects.
fn start_after_preflight<W, G, H, E>(
    create_window: impl FnOnce() -> Result<W, E>,
    create_gpu: impl FnOnce(&W) -> Result<G, E>,
    create_host: impl FnOnce() -> H,
) -> Result<(W, G, H), E> {
    let window = create_window()?;
    let gpu = create_gpu(&window)?;
    let host = create_host();
    Ok((window, gpu, host))
}

fn apply_renderer_scale_transition<R>(
    renderer: &mut R,
    scale_factor: f32,
    physical_size: (u32, u32),
    set_scale: impl FnOnce(&mut R, f32) -> Result<(), String>,
    resize_surface: impl FnOnce(&mut R, u32, u32),
) -> Result<(), String> {
    set_scale(renderer, scale_factor)?;
    resize_surface(renderer, physical_size.0, physical_size.1);
    Ok(())
}

struct App {
    app_config: Option<AppConfig>,
    font_profile: Box<ResolvedFontProfile>,
    wake_proxy: EventLoopProxy<UserEvent>,
    host: Option<FrontendHost>,
    window: Option<std::sync::Arc<Window>>,
    window_title: String,
    gpu: Option<GpuText>,
    visual_clock: Box<dyn VisualClock>,
    clipboard: Option<arboard::Clipboard>,
    next_heartbeat: Instant,
    fatal_error: Option<String>,
    consecutive_surface_recoveries: u8,
    consecutive_device_recoveries: u8,
    modifiers: ModifiersState,
    mouse_pixels: Option<(f64, f64)>,
    mouse_logical: Option<LogicalPoint>,
    mouse_cell: (u16, u16),
    pressed_pointer_buttons: PressedPointerButtons,
    wheel_cell_remainder: (f64, f64),
    scene_presentable: bool,
    window_focused: bool,
    ime_allowed: bool,
}

impl App {
    fn new(
        proxy: EventLoopProxy<UserEvent>,
        app_config: AppConfig,
        font_profile: Box<ResolvedFontProfile>,
    ) -> Self {
        Self {
            app_config: Some(app_config),
            font_profile,
            wake_proxy: proxy,
            host: None,
            window: None,
            window_title: "Mandatum".to_owned(),
            gpu: None,
            visual_clock: Box::new(MonotonicVisualClock),
            clipboard: None,
            next_heartbeat: Instant::now() + HEARTBEAT,
            fatal_error: None,
            consecutive_surface_recoveries: 0,
            consecutive_device_recoveries: 0,
            modifiers: ModifiersState::empty(),
            mouse_pixels: None,
            mouse_logical: None,
            mouse_cell: (0, 0),
            pressed_pointer_buttons: PressedPointerButtons::default(),
            wheel_cell_remainder: (0.0, 0.0),
            scene_presentable: false,
            window_focused: false,
            ime_allowed: false,
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn visual_now(&self) -> Instant {
        self.visual_clock.now()
    }

    fn host(&self) -> &FrontendHost {
        self.host
            .as_ref()
            .expect("FrontendHost exists after GPU preflight")
    }

    fn host_mut(&mut self) -> &mut FrontendHost {
        self.host
            .as_mut()
            .expect("FrontendHost exists after GPU preflight")
    }

    fn shutdown_host(&mut self) {
        if let Some(host) = &mut self.host {
            host.shutdown();
        }
    }

    fn fail(&mut self, message: impl Into<String>) {
        let message = message.into();
        eprintln!("mandatum-native: {message}");
        self.fatal_error = Some(message);
    }

    fn scene_size(&self) -> Option<SceneSize> {
        self.viewport_metrics().map(ViewportMetrics::scene_size)
    }

    fn viewport_metrics(&self) -> Option<ViewportMetrics> {
        let gpu = self.gpu.as_ref()?;
        let (width, height) = gpu.surface_size();
        viewport_metrics_from_renderer(width, height, gpu.scale(), gpu.cell_w(), gpu.cell_h())
    }

    fn resize_host(&mut self) {
        if let Some(size) = self.scene_size() {
            self.host_mut().handle_input(InputEvent::Resize(size));
        }
    }

    fn apply_scale_factor(&mut self, scale_factor: f32) {
        self.scene_presentable = false;
        self.cancel_pointer_gesture();
        self.host_mut().suspend_scene_interaction();
        let physical_size = self
            .window
            .as_ref()
            .expect("scale changes require a live window")
            .inner_size();
        if let Some(gpu) = &mut self.gpu
            && let Err(error) = apply_renderer_scale_transition(
                gpu,
                scale_factor,
                (physical_size.width, physical_size.height),
                GpuText::set_scale,
                GpuText::resize_surface,
            )
        {
            self.fail(format!("invalid display scale: {error}"));
            return;
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.snap_presentation_motion();
        }
        self.refresh_mouse_cell();
        self.resize_host();
        self.request_redraw();
    }

    fn send_input(&mut self, input: InputEvent) {
        let generation = self.host().scene_generation();
        self.host_mut().handle_input(input);
        self.apply_effects();
        if self.host().scene_generation() != generation {
            self.request_redraw();
        }
    }

    fn apply_effects(&mut self) {
        let effects = self.host_mut().take_effects();
        for effect in effects {
            match effect {
                FrontendEffect::SetClipboard(text) => {
                    if let Some(clipboard) = &mut self.clipboard {
                        if let Err(error) = clipboard.set_text(text) {
                            self.host_mut()
                                .report_platform_error(format!("clipboard write failed: {error}"));
                        }
                    } else {
                        self.host_mut()
                            .report_platform_error("clipboard write failed: clipboard unavailable");
                    }
                }
                FrontendEffect::ApplyFont { family, size } => self.apply_font(family, size),
            }
        }
    }

    /// Re-declare the live font truth to the shared state: resolved family,
    /// unscaled size, and the families the appearance overlay can offer.
    fn declare_font_facts(&mut self) {
        let family = self.font_profile.family().to_owned();
        let size = self
            .gpu
            .as_ref()
            .map(GpuText::base_font_size)
            .unwrap_or_else(|| self.font_profile.size());
        let families = cycle_candidate_families(self.font_profile.database());
        self.host_mut().set_font_facts(family, size, families);
    }

    /// Apply a requested font live. A size change reuses the cheap metric
    /// path; a family change resolves a fresh profile with launch-config
    /// degrade semantics — failure keeps the current font and warns in the
    /// status line instead of tearing anything down. Either way the actual
    /// resolved state is re-declared afterwards, so the overlay's rows stay
    /// truthful.
    fn apply_font(&mut self, family: String, size: f32) {
        let mut metrics_changed = false;
        if family != self.font_profile.family() {
            let request = if family == BUNDLED_FAMILY {
                FontRequest::BundledDefault { size }
            } else {
                FontRequest::installed(family.clone(), size)
            };
            match ResolvedFontProfile::resolve(request) {
                Ok(profile) => {
                    let applied = match self.gpu.as_mut() {
                        Some(gpu) => pollster::block_on(gpu.apply_font_profile(profile.clone())),
                        None => Ok(()),
                    };
                    match applied {
                        Ok(()) => {
                            *self.font_profile = profile;
                            metrics_changed = true;
                        }
                        Err(error) => {
                            self.host_mut()
                                .report_platform_error(format!("font not applied: {error}"));
                        }
                    }
                }
                Err(error) => {
                    self.host_mut()
                        .report_platform_error(format!("font not applied: {error}"));
                }
            }
        }
        if let Some(gpu) = self.gpu.as_mut() {
            let before = gpu.base_font_size();
            match gpu.set_base_font_size(size) {
                Ok(()) => {
                    if (before - size).abs() >= f32::EPSILON {
                        metrics_changed = true;
                    }
                }
                Err(error) => {
                    self.host_mut()
                        .report_platform_error(format!("font size not applied: {error}"));
                }
            }
        }
        self.declare_font_facts();
        if metrics_changed {
            // Cell metrics changed: the same choreography as a display-scale
            // transition keeps pointer, motion, and PTY geometry coherent.
            self.scene_presentable = false;
            self.cancel_pointer_gesture();
            self.host_mut().suspend_scene_interaction();
            if let Some(gpu) = &mut self.gpu {
                gpu.snap_presentation_motion();
            }
            self.refresh_mouse_cell();
            self.resize_host();
            self.request_redraw();
        }
    }

    /// Apply one bounded slice of runtime work. The drain is bounded by both
    /// an event budget and a wall-clock deadline, and re-sends this loop's
    /// wake itself for anything it leaves queued, so the caller only has to
    /// decide whether the slice changed the scene.
    fn drain_runtime(&mut self) -> bool {
        let generation = self.host().scene_generation();
        let _ = self.host_mut().drain_runtime_bounded(EVENT_DRAIN_BUDGET);
        self.apply_effects();
        self.host().scene_generation() != generation
    }

    fn render_frame(&mut self) -> Result<(), GpuRenderError> {
        self.scene_presentable = false;
        let Some(viewport) = self.viewport_metrics() else {
            self.cancel_and_disable_ime();
            self.host_mut().suspend_scene_interaction();
            return Ok(());
        };
        let snapshot = self.host_mut().frame_with_viewport(viewport);
        self.sync_window_title(&snapshot.scene);
        if scene_is_suspended_by_tiled_minimum(&snapshot.scene) {
            self.cancel_and_disable_ime();
            self.host_mut().suspend_scene_interaction();
            return Ok(());
        }
        self.sync_ime(&snapshot.scene);
        let visual_now = self.visual_now();
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let outcome = match gpu.render_at(&snapshot.scene, &snapshot.theme, visual_now) {
            Ok(outcome) => outcome,
            Err(GpuRenderError::DeviceLost { .. }) => {
                self.consecutive_device_recoveries =
                    self.consecutive_device_recoveries.saturating_add(1);
                if self.consecutive_device_recoveries > 3 {
                    self.fail("GPU device recovery exceeded three consecutive attempts");
                    return Ok(());
                }
                match pollster::block_on(gpu.recreate_device()) {
                    Ok(()) => {
                        self.scene_presentable = false;
                        self.host_mut().suspend_scene_interaction();
                        self.resize_host();
                        self.request_redraw();
                    }
                    Err(error) => {
                        self.fail(format!("GPU device recreation failed: {error}"));
                    }
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        match outcome {
            GpuRenderOutcome::Presented { .. } => {
                self.scene_presentable = !self
                    .gpu
                    .as_ref()
                    .is_some_and(GpuText::pointer_geometry_is_moving);
                if !self.scene_presentable {
                    self.host_mut().suspend_scene_interaction();
                }
                self.consecutive_surface_recoveries = 0;
                self.consecutive_device_recoveries = 0;
            }
            GpuRenderOutcome::Skipped { .. } => {
                self.host_mut().suspend_scene_interaction();
            }
            GpuRenderOutcome::SurfaceReconfigured { .. } => {
                self.host_mut().suspend_scene_interaction();
                self.consecutive_surface_recoveries =
                    self.consecutive_surface_recoveries.saturating_add(1);
                if self.consecutive_surface_recoveries > 8 {
                    self.fail("GPU surface recovery exceeded eight consecutive attempts");
                } else {
                    self.request_redraw();
                }
            }
        }
        Ok(())
    }

    fn exit_if_requested(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.host().should_quit() {
            return false;
        }
        self.cancel_and_disable_ime();
        self.shutdown_host();
        event_loop.exit();
        true
    }

    fn update_mouse_cell(&mut self, x: f64, y: f64) {
        self.mouse_pixels = Some((x, y));
        let Some(gpu) = &self.gpu else {
            self.mouse_logical = None;
            return;
        };
        self.mouse_logical = logical_pointer_position(x, y, gpu.scale());
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

    fn sync_window_title(&mut self, scene: &WorkspaceScene) {
        let Some(title) = next_native_window_title(&self.window_title, &scene.header.project_name)
        else {
            return;
        };
        if let Some(window) = &self.window {
            window.set_title(&title);
        }
        self.window_title = title;
    }

    fn pointer_input(&mut self, kind: PointerKind, button: Option<PointerButton>) {
        if !self.scene_presentable || self.scene_size().is_none() {
            return;
        }
        self.send_pointer_input(kind, button);
    }

    fn send_pointer_input(&mut self, kind: PointerKind, button: Option<PointerButton>) {
        let redraw_before = self.host().pointer_move_redraw_state();
        let (column, row) = self.mouse_cell;
        let mods = neutral_modifiers(self.modifiers);
        let pointer = PointerEvent {
            kind,
            button,
            column,
            row,
            mods,
        };
        if let Some(logical_position) = self.mouse_logical {
            self.host_mut()
                .handle_pointer_at_logical(pointer, logical_position);
        } else {
            self.host_mut().handle_input(InputEvent::Pointer(pointer));
        }
        let redraw_after = self.host().pointer_move_redraw_state();
        self.apply_effects();
        if pointer_input_needs_redraw(kind, redraw_before, redraw_after) {
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
                let admitted = self.scene_presentable && self.scene_size().is_some();
                if !self.pressed_pointer_buttons.begin(button, admitted) {
                    return;
                }
                self.send_pointer_input(PointerKind::Down, Some(button));
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
        self.send_input(if focused {
            InputEvent::FocusGained
        } else {
            InputEvent::FocusLost
        });
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
        if let Some(host) = &mut self.host {
            host.handle_input(InputEvent::Composition(CompositionEvent::Cancel));
        }
    }

    fn service_scheduled_work(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.window.is_none() {
            return false;
        }
        let now = self.visual_now();
        if now >= self.next_heartbeat {
            let scene_changed = self.host_mut().heartbeat();
            self.next_heartbeat = now + HEARTBEAT;
            if scene_changed {
                self.request_redraw();
            }
        }
        if self
            .gpu
            .as_ref()
            .and_then(GpuText::next_animation_deadline)
            .is_some_and(|deadline| now >= deadline)
        {
            self.request_redraw();
        }
        if self.fatal_error.is_some() {
            self.cancel_and_disable_ime();
            self.shutdown_host();
            event_loop.exit();
            return true;
        }
        false
    }

    fn schedule_next_wake(&self, event_loop: &ActiveEventLoop) {
        let animation = self.gpu.as_ref().and_then(GpuText::next_animation_deadline);
        event_loop.set_control_flow(ControlFlow::WaitUntil(next_scheduled_deadline(
            self.next_heartbeat,
            animation,
        )));
    }

    fn cancel_pointer_gesture(&mut self) {
        self.pressed_pointer_buttons.clear();
        if let Some(host) = &mut self.host {
            host.cancel_pointer_gesture();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, _cause: StartCause) {
        self.service_scheduled_work(event_loop);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let font_profile = self.font_profile.as_ref().clone();
        let app_config = self
            .app_config
            .take()
            .expect("native startup configuration is consumed exactly once");
        let wake_proxy = self.wake_proxy.clone();
        let startup = start_after_preflight(
            || {
                let geometry = native_window_geometry();
                let window = event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title("Mandatum")
                            .with_inner_size(geometry.initial)
                            .with_min_inner_size(geometry.minimum),
                    )
                    .map_err(|error| {
                        GpuStartupError::no_display(format!("no window (headless?): {error}"))
                    })?;
                #[cfg(target_os = "macos")]
                window.set_option_as_alt(OptionAsAlt::OnlyRight);
                Ok(std::sync::Arc::new(window))
            },
            |window| pollster::block_on(GpuText::new_with_profile(window.clone(), font_profile)),
            || {
                FrontendHost::new_with_wake_callback(app_config, move || {
                    let _ = wake_proxy.send_event(UserEvent::Wake);
                })
            },
        );
        let (window, gpu, mut host) = match startup {
            Ok(started) => started,
            Err(error) => {
                self.fail(format!("{}: {error}", startup_error_kind(error.kind())));
                event_loop.exit();
                return;
            }
        };
        self.clipboard = match arboard::Clipboard::new() {
            Ok(clipboard) => Some(clipboard),
            Err(error) => {
                host.report_platform_error(format!("clipboard unavailable: {error}"));
                None
            }
        };
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.host = Some(host);
        self.declare_font_facts();
        self.next_heartbeat = Instant::now() + HEARTBEAT;
        self.resize_host();
        self.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                if self.service_scheduled_work(event_loop) {
                    return;
                }
                let scene_changed = self.drain_runtime();
                if self.exit_if_requested(event_loop) {
                    return;
                }
                if scene_changed {
                    self.request_redraw();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.service_scheduled_work(event_loop) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.cancel_and_disable_ime();
                self.shutdown_host();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                // The frame below repaints regardless, so the drain's
                // scene-change verdict is not needed here.
                self.drain_runtime();
                if self.exit_if_requested(event_loop) {
                    return;
                }
                if let Err(error) = self.render_frame() {
                    self.fail(error.to_string());
                }
                if self.fatal_error.is_some() {
                    self.cancel_and_disable_ime();
                    self.shutdown_host();
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                self.scene_presentable = false;
                self.cancel_pointer_gesture();
                self.host_mut().suspend_scene_interaction();
                if let Some(gpu) = &mut self.gpu {
                    gpu.snap_presentation_motion();
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
                self.send_input(InputEvent::Composition(CompositionEvent::Cancel));
            }
            WindowEvent::Ime(ime)
                if ime_event_is_accepted(self.window_focused, self.ime_allowed) =>
            {
                if let Some(composition) = translate_ime(ime) {
                    self.send_input(InputEvent::Composition(composition));
                }
            }
            WindowEvent::Ime(_) => {}
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = key_for_platform_translation(
                    &event.logical_key,
                    &event.key_without_modifiers(),
                    self.modifiers,
                );
                match translate_key(&key, self.modifiers) {
                    PlatformAction::Input(input) => self.send_input(input),
                    PlatformAction::PasteShortcut(shortcut) => {
                        if self.host().handles_workspace_key(shortcut) {
                            self.send_input(InputEvent::Key(shortcut));
                        } else if let Some(clipboard) = &mut self.clipboard {
                            match clipboard.get_text() {
                                Ok(text) => self.send_input(InputEvent::Paste(text)),
                                Err(error) => self.host_mut().report_platform_error(format!(
                                    "clipboard read failed: {error}"
                                )),
                            }
                            self.request_redraw();
                        } else {
                            self.host_mut().report_platform_error(
                                "clipboard read failed: clipboard unavailable",
                            );
                            self.request_redraw();
                        }
                    }
                    PlatformAction::CopyShortcut(shortcut) => {
                        if self.host().handles_workspace_key(shortcut) {
                            self.send_input(InputEvent::Key(shortcut));
                        } else {
                            self.host_mut().copy_selection();
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
            WindowEvent::CursorLeft { .. } => {
                if self.host_mut().pointer_left() {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = neutral_button(button) {
                    self.pointer_button(state, button);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.pointer_wheel(delta),
            WindowEvent::Focused(focused) => self.focus_changed(focused),
            WindowEvent::Occluded(_) => {}
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if !self.service_scheduled_work(event_loop) {
            self.schedule_next_wake(event_loop);
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.cancel_and_disable_ime();
        self.shutdown_host();
    }
}

fn startup_error_kind(kind: GpuStartupErrorKind) -> &'static str {
    match kind {
        GpuStartupErrorKind::NoAdapter => "no_adapter",
        GpuStartupErrorKind::NoDisplay => "no_display",
        GpuStartupErrorKind::DeviceRequest => "device_request",
        GpuStartupErrorKind::InvalidConfiguration => "invalid_configuration",
    }
}

struct NativeLaunch {
    event_loop: EventLoop<UserEvent>,
    app: App,
}

fn construct_native_launch(
    mut app_config: AppConfig,
    font_profile: Box<ResolvedFontProfile>,
    font_warning: Option<String>,
) -> Result<NativeLaunch, String> {
    eprintln!(
        "mandatum-native: font {}",
        serde_json::to_string(font_profile.info()).expect("FontInfo is JSON serializable")
    );
    if let Some(warning) = font_warning {
        app_config.config_warnings.push(warning);
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|error| format!("no display: {error}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let app = App::new(proxy, app_config, font_profile);
    Ok(NativeLaunch { event_loop, app })
}

fn main() {
    // Config loads before the font preflight so `[font]` can inform the
    // request; `--font-info` therefore reports the configured font too.
    let app_config = match AppConfig::from_current_dir() {
        Ok(app_config) => app_config,
        Err(error) => {
            eprintln!("mandatum-native: host initialization failed: {error}");
            std::process::exit(2);
        }
    };
    let configured_font = ConfiguredFont {
        family: app_config.font_family.clone(),
        size: app_config.font_size,
    };
    let launch = match launch_after_font_preflight(
        std::env::args().skip(1),
        &configured_font,
        |profile, warning| construct_native_launch(app_config, profile, warning),
    ) {
        Ok(FontPreflightOutcome::Info(json)) => {
            println!("{json}");
            return;
        }
        Ok(FontPreflightOutcome::Launch(launch)) => launch,
        Err(error) => {
            eprintln!("mandatum-native: {error}");
            std::process::exit(2);
        }
    };
    let NativeLaunch {
        event_loop,
        mut app,
    } = launch;
    if let Err(error) = event_loop.run_app(&mut app) {
        app.fail(format!("event loop error: {error}"));
    }
    app.cancel_and_disable_ime();
    app.shutdown_host();
    if app.fatal_error.is_some() {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests;
