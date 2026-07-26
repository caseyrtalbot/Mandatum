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
// Matches the app layer's DRAIN_EVENT_BUDGET; the 3ms wall-clock deadline
// inside the app drain remains the responsiveness bound.
const EVENT_DRAIN_BUDGET: usize = 256;
// Retry cadence after a skipped render (transient surface timeout): soon
// enough that the missed frame presents within a few refresh intervals,
// long enough that a persistently timing-out surface cannot spin the loop.
const SKIPPED_RENDER_RETRY: Duration = Duration::from_millis(50);

trait VisualClock {
    fn now(&self) -> Instant;
}

struct MonotonicVisualClock;

impl VisualClock for MonotonicVisualClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Deadlines that exist only to drive a repaint (animation stepping, the
/// skipped-render retry) never arm a wake while the window is occluded:
/// repaints are suppressed while occluded, so arming an already-elapsed
/// deadline would wake the loop hot until de-occlusion instead of letting
/// it idle at the heartbeat. De-occlusion itself forces a render, which
/// re-derives or clears both deadlines.
fn next_scheduled_deadline(
    heartbeat: Instant,
    animation: Option<Instant>,
    render_retry: Option<Instant>,
    occluded: bool,
) -> Instant {
    if occluded {
        return heartbeat;
    }
    [animation, render_retry]
        .into_iter()
        .flatten()
        .fold(heartbeat, Instant::min)
}

/// A skipped render latched `force_render` but presented nothing, and no
/// other path re-requests the frame; retry through the scheduled-work
/// mechanism rather than an immediate re-request, which could tight-loop
/// against a persistently timing-out surface.
fn skipped_render_retry_deadline(now: Instant) -> Instant {
    now + SKIPPED_RENDER_RETRY
}

/// A repaint is skippable only when nothing could have changed the frame:
/// no forced repaint pending (surface recovery, resize, scale/font
/// transitions, de-occlusion), no active presentation motion, and the scene
/// generation matches the last presented frame.
fn render_can_be_skipped(
    force_render: bool,
    animation_active: bool,
    last_rendered_generation: Option<u64>,
    scene_generation: u64,
) -> bool {
    !force_render
        && !animation_active
        && last_rendered_generation.is_some_and(|generation| generation == scene_generation)
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
    before: (bool, Option<usize>, Option<usize>),
    after: (bool, Option<usize>, Option<usize>),
) -> bool {
    kind != PointerKind::Move || before.0 || after.0 || before.1 != after.1 || before.2 != after.2
}

/// Fold a precise (trackpad pixel) scroll delta, already converted to cell
/// units, into whole-cell steps, carrying the sub-cell remainder between
/// events. A direction reversal on an axis discards that axis's remainder:
/// leftover travel from the old direction must not pay into the new one.
fn accumulate_precise_wheel(remainder: &mut (f64, f64), delta_cells: (f64, f64)) -> (i16, i16) {
    fn axis(remainder: &mut f64, delta: f64) -> i16 {
        if *remainder * delta < 0.0 {
            *remainder = 0.0;
        }
        *remainder += delta;
        let whole = remainder.trunc();
        *remainder -= whole;
        whole as i16
    }
    (
        axis(&mut remainder.0, delta_cells.0),
        axis(&mut remainder.1, delta_cells.1),
    )
}

/// Sub-cell precise-wheel travel banked between events, keyed on the mouse
/// cell that accumulated it. The cell is only a proxy for the scroll
/// target, so the bank must also be invalidated by any transition that can
/// change which pane a cell resolves to (keyboard-driven layout change,
/// resize, display-scale or font-metric change); otherwise travel banked
/// over one pane pays into a whole-row scroll against another.
#[derive(Debug, Default, PartialEq)]
struct WheelRemainderBank {
    remainder: (f64, f64),
    cell: Option<(u16, u16)>,
}

impl WheelRemainderBank {
    /// Drop all banked travel; the next precise event starts from zero.
    fn invalidate(&mut self) {
        self.remainder = (0.0, 0.0);
        self.cell = None;
    }

    /// Fold a precise delta banked against `mouse_cell`. Travel banked
    /// over a different cell may have targeted another pane, so it is
    /// dropped rather than paid into the new target.
    fn accumulate(&mut self, mouse_cell: (u16, u16), delta_cells: (f64, f64)) -> (i16, i16) {
        if self.cell != Some(mouse_cell) {
            self.remainder = (0.0, 0.0);
            self.cell = Some(mouse_cell);
        }
        accumulate_precise_wheel(&mut self.remainder, delta_cells)
    }
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
    wheel_remainder: WheelRemainderBank,
    scene_presentable: bool,
    window_focused: bool,
    window_occluded: bool,
    ime_allowed: bool,
    last_rendered_scene_generation: Option<u64>,
    force_render: bool,
    render_retry_deadline: Option<Instant>,
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
            wheel_remainder: WheelRemainderBank::default(),
            scene_presentable: false,
            window_focused: false,
            window_occluded: false,
            ime_allowed: false,
            last_rendered_scene_generation: None,
            force_render: true,
            render_retry_deadline: None,
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Wake/heartbeat/animation repaints are pointless while the window is
    /// occluded; the runtime keeps draining regardless (PTY flow-control
    /// credits ride on drained events), only the repaint is suppressed.
    fn request_redraw_if_visible(&self) {
        if !self.window_occluded {
            self.request_redraw();
        }
    }

    fn window_occluded_changed(&mut self, occluded: bool) {
        if occluded == self.window_occluded {
            return;
        }
        self.window_occluded = occluded;
        if occluded {
            // Snap motion so no mid-flight frame is left frozen on screen
            // and the animation deadline stops arming wakes.
            if let Some(gpu) = &mut self.gpu {
                gpu.snap_presentation_motion();
            }
        } else {
            self.force_render = true;
            self.request_redraw();
        }
    }

    /// Belt-and-braces de-occlusion: focus and cursor events prove the
    /// window is visible even if `Occluded(false)` was never delivered.
    fn note_window_visible(&mut self) {
        if self.window_occluded {
            self.window_occluded_changed(false);
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
        self.force_render = true;
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
            // The scene changed (possibly the pane layout): sub-row wheel
            // travel banked under the old layout must not pay into a
            // scroll against whatever pane now occupies that cell.
            self.wheel_remainder.invalidate();
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
            self.force_render = true;
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
        if render_can_be_skipped(
            self.force_render,
            self.gpu.as_ref().is_some_and(GpuText::animation_is_active),
            self.last_rendered_scene_generation,
            self.host().scene_generation(),
        ) {
            return Ok(());
        }
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
        let snapshot_generation = snapshot.scene_generation;
        let visual_now = self.visual_now();
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        // Threading the snapshot's scene generation lets the renderer reuse
        // its compiled scene on animation-only frames; the generation is the
        // same dirtiness counter the render-skip guard above keys on.
        let outcome = match gpu.render_generation_at(
            &snapshot.scene,
            &snapshot.theme,
            Some(snapshot_generation),
            visual_now,
        ) {
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
                        self.force_render = true;
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
                // Pane-geometry motion no longer suspends pointer admission:
                // hit targets stay at stable settled geometry, so input
                // admitted mid-flight resolves against the final layout.
                self.scene_presentable = true;
                self.last_rendered_scene_generation = Some(snapshot_generation);
                self.force_render = false;
                self.render_retry_deadline = None;
                self.consecutive_surface_recoveries = 0;
                self.consecutive_device_recoveries = 0;
                // Present-paced animation stepping: chaining a redraw off
                // each presented frame locks motion to the display refresh
                // (FIFO present). The WaitUntil animation deadline stays as
                // fallback for outcomes where no present occurs.
                if self.gpu.as_ref().is_some_and(GpuText::animation_is_active) {
                    self.request_redraw_if_visible();
                }
            }
            GpuRenderOutcome::Skipped { .. } => {
                self.force_render = true;
                self.host_mut().suspend_scene_interaction();
                // Nothing was presented and `scene_presentable` stays
                // false, so without a retry the app would stop presenting
                // and drop pointer input until an unrelated scene change.
                self.render_retry_deadline = Some(skipped_render_retry_deadline(self.visual_now()));
            }
            GpuRenderOutcome::SurfaceReconfigured { .. } => {
                self.force_render = true;
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
        // A render performed while occluded (an already-queued redraw, a
        // synchronous resize render) can create fresh motion whose deadline
        // occlusion suppresses from ever being serviced; snapping mirrors
        // the Occluded(true) transition and keeps "no motion state while
        // occluded" true for every render path.
        if self.window_occluded
            && let Some(gpu) = self.gpu.as_mut()
        {
            gpu.snap_presentation_motion();
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
        let (dx, dy, precise) = match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                self.wheel_remainder.invalidate();
                ((-x).round() as i16, (-y).round() as i16, false)
            }
            MouseScrollDelta::PixelDelta(position) => {
                // Precise deltas accumulate in cell units (rows vertically)
                // and dispatch every whole row, so a slow trackpad scroll
                // moves one row at a time with no amplification downstream.
                let (dx, dy) = self.wheel_remainder.accumulate(
                    self.mouse_cell,
                    (
                        -position.x / f64::from(gpu.cell_w()),
                        -position.y / f64::from(gpu.cell_h()),
                    ),
                );
                (dx, dy, true)
            }
        };
        // Wheel bypasses the scene_presentable gate: it re-resolves its pane
        // target in the app layer and needs no fresh hit test, so it is
        // admitted whenever a scene exists, even while geometry settles.
        if self.scene_size().is_none() {
            return;
        }
        if dx != 0 {
            self.send_pointer_input(PointerKind::Wheel { dx, dy: 0, precise }, None);
        }
        if dy != 0 {
            self.send_pointer_input(PointerKind::Wheel { dx: 0, dy, precise }, None);
        }
    }

    fn focus_changed(&mut self, focused: bool) {
        self.window_focused = focused;
        if focused {
            self.note_window_visible();
        }
        if !focused {
            self.cancel_and_disable_ime();
            self.pressed_pointer_buttons.clear();
            self.modifiers = ModifiersState::empty();
            self.wheel_remainder.invalidate();
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
                self.request_redraw_if_visible();
            }
        }
        if self
            .gpu
            .as_ref()
            .and_then(GpuText::next_animation_deadline)
            .is_some_and(|deadline| now >= deadline)
        {
            self.request_redraw_if_visible();
        }
        if self
            .render_retry_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            // One-shot: the deadline is consumed when serviced, so a
            // persistently skipping surface re-arms per attempt instead of
            // pinning an expired WaitUntil target.
            self.render_retry_deadline = None;
            self.request_redraw_if_visible();
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
            self.render_retry_deadline,
            self.window_occluded,
        )));
    }

    fn cancel_pointer_gesture(&mut self) {
        self.pressed_pointer_buttons.clear();
        // Every caller is a geometry transition (resize, display-scale or
        // font-metric change): banked sub-row wheel travel is stale against
        // the new cell-to-pane mapping.
        self.wheel_remainder.invalidate();
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
                    self.request_redraw_if_visible();
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
                // render_frame's skip guard reads the post-drain scene
                // generation, so the drain's scene-change verdict is not
                // needed here.
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
                self.force_render = true;
                self.cancel_pointer_gesture();
                self.host_mut().suspend_scene_interaction();
                if let Some(gpu) = &mut self.gpu {
                    gpu.snap_presentation_motion();
                    gpu.resize_surface(size.width, size.height);
                }
                self.refresh_mouse_cell();
                self.resize_host();
                // Render inside the resize step so a frame matching the new
                // size exists before the window edge moves again; a deferred
                // redraw would rubber-band content behind the edge.
                if let Err(error) = self.render_frame() {
                    self.fail(error.to_string());
                }
                if self.fatal_error.is_some() {
                    self.cancel_and_disable_ime();
                    self.shutdown_host();
                    event_loop.exit();
                }
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
                self.note_window_visible();
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
            WindowEvent::Occluded(occluded) => self.window_occluded_changed(occluded),
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
