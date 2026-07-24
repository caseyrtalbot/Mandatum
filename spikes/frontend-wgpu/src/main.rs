//! Excluded native/GPU frontend over Mandatum's real workstation host.
//!
//! The spike owns winit, wgpu, clipboard integration, paint scheduling, and
//! latency instrumentation. Product state, PTYs, parsing, commands, recovery,
//! and persistence stay behind `mandatum_app::FrontendHost`.

mod stats;

use std::{
    collections::VecDeque,
    fs,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use mandatum_app::{AppConfig, FrontendEffect, FrontendHost};
use mandatum_gpu_renderer_spike::{
    GpuFaultInjection, GpuFaultInjectionResult, GpuFrameSkip, GpuRenderError, GpuRenderOutcome,
    GpuStartupError, GpuStartupErrorKind, GpuSurfaceRecovery, GpuText, NativeTextSettings,
};
use mandatum_scene::{
    SceneSize, WorkspaceScene,
    input::{
        CompositionEvent, InputEvent, Key as InputKey, KeyCode, Modifiers, PointerButton,
        PointerEvent, PointerKind, TextRange,
    },
};
use serde::Serialize;
use stats::{MemorySamples, MemorySummary, MetricSummary, Samples, StressState, StressSummary};
#[cfg(target_os = "macos")]
use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    platform::modifier_supplement::KeyEventExtModifierSupplement,
    window::{Window, WindowId},
};

const DEFAULT_INJECT_TOTAL: u32 = 300;
const DEFAULT_INJECT_INTERVAL: Duration = Duration::from_micros(33_333);
const RESIZE_EXERCISE_STEPS: u64 = 1_000;
const DEFAULT_RESIZE_INTERVAL: Duration = Duration::from_millis(16);
const DEFAULT_SOAK_DURATION: Duration = Duration::from_secs(30 * 60);
// macOS applies live window-size changes asynchronously and continuous PTY
// backpressure can delay the event loop for several seconds. A fifteen-second
// cadence still exercises 120 resize/scale/input cycles in the required
// 30-minute soak without defining a schedule the system cannot service.
const DEFAULT_SOAK_INTERVAL: Duration = Duration::from_secs(15);
const DEFAULT_MEMORY_INTERVAL: Duration = Duration::from_secs(5);
const MAX_MEASUREMENT_SAMPLES: usize = 200_000;
const HEARTBEAT: Duration = Duration::from_millis(250);
const EVENT_DRAIN_BUDGET: usize = 16;
const IDLE_FRAME_CUTOFF_MS: f64 = 250.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum StressConfig {
    ResizeExercise { steps: u64 },
    Soak { duration: Duration },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FaultConfig {
    SurfaceOutdated,
    SurfaceLost,
    DeviceLost,
    OutOfMemory,
}

impl FaultConfig {
    fn label(self) -> &'static str {
        match self {
            Self::SurfaceOutdated => "surface_outdated",
            Self::SurfaceLost => "surface_lost",
            Self::DeviceLost => "device_lost",
            Self::OutOfMemory => "out_of_memory",
        }
    }

    fn injection(self) -> GpuFaultInjection {
        match self {
            Self::SurfaceOutdated => GpuFaultInjection::SurfaceOutdated,
            Self::SurfaceLost => GpuFaultInjection::SurfaceLost,
            Self::DeviceLost => GpuFaultInjection::DeviceLost,
            Self::OutOfMemory => GpuFaultInjection::OutOfMemory,
        }
    }
}

#[derive(Clone)]
struct Config {
    exit_after: Option<f64>,
    typing_bench: bool,
    typing_samples: u32,
    typing_interval: Duration,
    flood: bool,
    scale_after: Option<f64>,
    scale_factor: f32,
    stress: Option<StressConfig>,
    stress_interval: Option<Duration>,
    fault: Option<FaultConfig>,
    fault_after: Duration,
    memory_interval: Duration,
    text_settings: NativeTextSettings,
    harness_project_path: Option<String>,
}

fn parse_config() -> Result<Config, String> {
    parse_config_from(std::env::args().skip(1))
}

fn parse_config_from(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let defaults = NativeTextSettings::default();
    let mut font_family = defaults.family().to_owned();
    let mut font_size = defaults.font_size();
    let mut typing_interval_set = false;
    let mut stress_interval_set = false;
    let mut fault_after_set = false;
    let mut scale_factor_set = false;
    let mut config = Config {
        exit_after: None,
        typing_bench: false,
        typing_samples: DEFAULT_INJECT_TOTAL,
        typing_interval: DEFAULT_INJECT_INTERVAL,
        flood: false,
        scale_after: None,
        scale_factor: 1.5,
        stress: None,
        stress_interval: None,
        fault: None,
        fault_after: Duration::from_secs(1),
        memory_interval: DEFAULT_MEMORY_INTERVAL,
        text_settings: defaults,
        harness_project_path: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-after" => {
                let value = required_value(&mut args, &arg)?;
                config.exit_after = Some(
                    parse_bounded_f64(&value, 0.01, 21_600.0)
                        .ok_or_else(|| invalid_value(&arg, &value, "0.01..=21600 seconds"))?,
                );
            }
            "--typing-bench" => config.typing_bench = true,
            "--typing-samples" => {
                let value = required_value(&mut args, &arg)?;
                config.typing_samples = parse_bounded_u64(&value, 1, 200_000)
                    .ok_or_else(|| invalid_value(&arg, &value, "1..=200000"))?
                    as u32;
                config.typing_bench = true;
            }
            "--typing-interval-ms" => {
                let value = required_value(&mut args, &arg)?;
                config.typing_interval = parse_duration_ms(&value, 1, 60_000)
                    .ok_or_else(|| invalid_value(&arg, &value, "1..=60000 ms"))?;
                typing_interval_set = true;
            }
            "--flood" => config.flood = true,
            "--resize-exercise" => {
                set_stress(
                    &mut config,
                    StressConfig::ResizeExercise {
                        steps: RESIZE_EXERCISE_STEPS,
                    },
                )?;
            }
            "--resize-count" => {
                let value = required_value(&mut args, &arg)?;
                let steps = parse_bounded_u64(&value, 1, 100_000)
                    .ok_or_else(|| invalid_value(&arg, &value, "1..=100000"))?;
                set_stress(&mut config, StressConfig::ResizeExercise { steps })?;
            }
            "--soak" => {
                set_stress(
                    &mut config,
                    StressConfig::Soak {
                        duration: DEFAULT_SOAK_DURATION,
                    },
                )?;
                config.flood = true;
            }
            "--soak-seconds" => {
                let value = required_value(&mut args, &arg)?;
                let seconds = parse_bounded_u64(&value, 1, 21_600)
                    .ok_or_else(|| invalid_value(&arg, &value, "1..=21600 seconds"))?;
                set_stress(
                    &mut config,
                    StressConfig::Soak {
                        duration: Duration::from_secs(seconds),
                    },
                )?;
                config.flood = true;
            }
            "--stress-interval-ms" => {
                let value = required_value(&mut args, &arg)?;
                config.stress_interval = Some(
                    parse_duration_ms(&value, 5, 60_000)
                        .ok_or_else(|| invalid_value(&arg, &value, "5..=60000 ms"))?,
                );
                stress_interval_set = true;
            }
            "--memory-interval-ms" => {
                let value = required_value(&mut args, &arg)?;
                config.memory_interval = parse_duration_ms(&value, 250, 60_000)
                    .ok_or_else(|| invalid_value(&arg, &value, "250..=60000 ms"))?;
            }
            "--inject-fault" => {
                let value = required_value(&mut args, &arg)?;
                config.fault = Some(match value.as_str() {
                    "surface-outdated" => FaultConfig::SurfaceOutdated,
                    "surface-lost" => FaultConfig::SurfaceLost,
                    "device-lost" => FaultConfig::DeviceLost,
                    "out-of-memory" => FaultConfig::OutOfMemory,
                    _ => {
                        return Err(invalid_value(
                            &arg,
                            &value,
                            "surface-outdated|surface-lost|device-lost|out-of-memory",
                        ));
                    }
                });
            }
            "--fault-after" => {
                let value = required_value(&mut args, &arg)?;
                let seconds = parse_bounded_f64(&value, 0.0, 3_600.0)
                    .ok_or_else(|| invalid_value(&arg, &value, "0..=3600 seconds"))?;
                config.fault_after = Duration::from_secs_f64(seconds);
                fault_after_set = true;
            }
            "--scale-after" => {
                let value = required_value(&mut args, &arg)?;
                config.scale_after = Some(
                    parse_scale_delay(&value)
                        .ok_or_else(|| invalid_value(&arg, &value, "0..=3600 seconds"))?,
                );
            }
            "--scale-factor" => {
                let value = required_value(&mut args, &arg)?;
                config.scale_factor = parse_scale_factor(&value)
                    .ok_or_else(|| invalid_value(&arg, &value, "0.25..=8"))?;
                scale_factor_set = true;
            }
            "--font-family" => {
                let value = required_value(&mut args, &arg)?;
                font_family = parse_font_family(&value)
                    .ok_or_else(|| invalid_value(&arg, &value, "1..=128 non-control characters"))?;
            }
            "--font-size" => {
                let value = required_value(&mut args, &arg)?;
                font_size =
                    parse_font_size(&value).ok_or_else(|| invalid_value(&arg, &value, "6..=72"))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    config.text_settings =
        NativeTextSettings::new(font_family, font_size).map_err(|error| error.to_string())?;
    if typing_interval_set && !config.typing_bench {
        return Err("--typing-interval-ms requires --typing-bench or --typing-samples".to_owned());
    }
    if stress_interval_set && config.stress.is_none() {
        return Err("--stress-interval-ms requires a resize exercise or soak".to_owned());
    }
    if fault_after_set && config.fault.is_none() {
        return Err("--fault-after requires --inject-fault".to_owned());
    }
    if scale_factor_set && config.scale_after.is_none() && config.stress.is_none() {
        return Err("--scale-factor requires --scale-after or a stress workload".to_owned());
    }
    if config.typing_bench && (config.flood || config.stress.is_some()) {
        return Err(
            "typing latency must run in isolation from flood and stress workloads".to_owned(),
        );
    }
    Ok(config)
}

fn required_value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {option}"))
}

fn set_stress(config: &mut Config, stress: StressConfig) -> Result<(), String> {
    if config.stress.is_some() {
        return Err(
            "choose only one of --resize-exercise/--resize-count/--soak/--soak-seconds".to_owned(),
        );
    }
    config.stress = Some(stress);
    Ok(())
}

fn parse_duration_ms(value: &str, min: u64, max: u64) -> Option<Duration> {
    parse_bounded_u64(value, min, max).map(Duration::from_millis)
}

fn parse_bounded_u64(value: &str, min: u64, max: u64) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| (min..=max).contains(parsed))
}

fn parse_bounded_f64(value: &str, min: f64, max: f64) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|parsed| parsed.is_finite() && (min..=max).contains(parsed))
}

fn invalid_value(option: &str, value: &str, expected: &str) -> String {
    format!("invalid value for {option}: {value:?}; expected {expected}")
}

fn configured_run_timeout(config: &Config) -> Option<Duration> {
    let explicit = config.exit_after.map(Duration::from_secs_f64);
    let mut automatic = None;
    let mut include = |candidate: Duration| {
        automatic = Some(automatic.map_or(candidate, |current: Duration| current.max(candidate)));
    };
    match config.stress {
        Some(StressConfig::ResizeExercise { steps }) => {
            let interval = config
                .stress_interval
                .unwrap_or(DEFAULT_RESIZE_INTERVAL)
                .as_secs_f64();
            include(Duration::from_secs_f64(
                (interval * steps as f64 * 20.0).clamp(60.0, 21_600.0),
            ));
        }
        Some(StressConfig::Soak { duration }) => {
            include(Duration::from_millis(400) + duration);
        }
        None => {}
    }
    if config.typing_bench {
        include(
            Duration::from_millis(400)
                + config.typing_interval * config.typing_samples
                + Duration::from_secs(2),
        );
    }
    if config.fault.is_some() {
        include(config.fault_after + Duration::from_secs(5));
    }
    match (explicit, automatic) {
        (Some(explicit), Some(automatic)) => Some(explicit.min(automatic)),
        (Some(explicit), None) => Some(explicit),
        (None, automatic) => automatic,
    }
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

#[derive(Clone, Debug, Serialize)]
struct OutcomeEvidence {
    status: &'static str,
    phase: &'static str,
    kind: &'static str,
    message: String,
}

impl OutcomeEvidence {
    fn success(message: impl Into<String>) -> Self {
        Self {
            status: "ok",
            phase: "complete",
            kind: "clean_exit",
            message: message.into(),
        }
    }

    fn failure(phase: &'static str, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: "error",
            phase,
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct GpuEvidence {
    name: String,
    backend: String,
    device_type: String,
    driver: String,
    driver_info: String,
    vendor: u32,
    device: u32,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct LifecycleEvidence {
    input_correlation_drops: u64,
    timeout_skips: u64,
    occluded_skips: u64,
    window_occlusion_events: u64,
    outdated_reconfigurations: u64,
    lost_reconfigurations: u64,
    device_recreations: u64,
    device_recreation_failures: u64,
    device_generation: u64,
    surface_generation: u64,
    renderer_surface_reconfigurations: u64,
    injected_faults: u64,
    quad_capacity_floats: usize,
    raster_capacity_floats: usize,
    text_row_capacity: usize,
    raster_cache_entries: usize,
    raster_cache_entries_high_water: usize,
    raster_cache_bytes: usize,
    raster_cache_bytes_high_water: usize,
}

#[derive(Clone, Debug, Serialize)]
struct WorkloadEvidence {
    typing_bench: bool,
    typing_target: u32,
    typing_interval_ms: Option<u64>,
    flood: bool,
    stress: &'static str,
    stress_target: Option<u64>,
    soak_seconds: Option<u64>,
    stress_interval_ms: Option<u64>,
    memory_interval_ms: u64,
    injected_fault: Option<&'static str>,
    fault_after_ms: Option<u64>,
    scale_after_ms: Option<u64>,
    scale_factor: f32,
    font_family: String,
    font_size: f32,
    harness_project_path: Option<String>,
    window_visibility_policy: &'static str,
    elapsed_ms: u64,
}

#[derive(Serialize)]
struct RunEvidence {
    schema_version: u8,
    outcome: OutcomeEvidence,
    platform: PlatformEvidence,
    gpu: Option<GpuEvidence>,
    display_refresh_hz: Option<f64>,
    first_usable_frame_ms: Option<f64>,
    first_usable_frame_within_1s: Option<bool>,
    workload: WorkloadEvidence,
    input_to_present_ms: MetricSummary,
    frame_ms: MetricSummary,
    stress: Option<StressSummary>,
    fault_injection: Option<FaultEvidence>,
    memory: MemorySummary,
    lifecycle: LifecycleEvidence,
    notes: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct FaultEvidence {
    requested: &'static str,
    injected: bool,
    post_recovery_present: bool,
}

#[derive(Serialize)]
struct PlatformEvidence {
    os: &'static str,
    arch: &'static str,
}

fn workload_evidence(config: &Config, elapsed: Duration) -> WorkloadEvidence {
    let (stress, stress_target, soak_seconds, default_interval) = match config.stress {
        None => ("none", None, None, None),
        Some(StressConfig::ResizeExercise { steps }) => (
            "resize_scale_exercise",
            Some(steps),
            None,
            Some(DEFAULT_RESIZE_INTERVAL),
        ),
        Some(StressConfig::Soak { duration }) => (
            "flood_resize_input_soak",
            None,
            Some(duration.as_secs()),
            Some(DEFAULT_SOAK_INTERVAL),
        ),
    };
    WorkloadEvidence {
        typing_bench: config.typing_bench,
        typing_target: if config.typing_bench {
            config.typing_samples
        } else {
            0
        },
        typing_interval_ms: config
            .typing_bench
            .then_some(config.typing_interval.as_millis() as u64),
        flood: config.flood,
        stress,
        stress_target,
        soak_seconds,
        stress_interval_ms: config
            .stress_interval
            .or(default_interval)
            .map(|interval| interval.as_millis() as u64),
        memory_interval_ms: config.memory_interval.as_millis() as u64,
        injected_fault: config.fault.map(FaultConfig::label),
        fault_after_ms: config.fault.map(|_| config.fault_after.as_millis() as u64),
        scale_after_ms: config.scale_after.map(|seconds| (seconds * 1_000.0) as u64),
        scale_factor: config.scale_factor,
        font_family: config.text_settings.family().to_owned(),
        font_size: config.text_settings.font_size(),
        harness_project_path: config.harness_project_path.clone(),
        window_visibility_policy: if matches!(config.stress, Some(StressConfig::Soak { .. })) {
            "focus_each_action_reference"
        } else {
            "normal"
        },
        elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
    }
}

fn startup_error_kind(kind: GpuStartupErrorKind) -> &'static str {
    match kind {
        GpuStartupErrorKind::NoDisplay => "no_display",
        GpuStartupErrorKind::NoAdapter => "no_adapter",
        GpuStartupErrorKind::DeviceRequest => "device_request",
        GpuStartupErrorKind::InvalidConfiguration => "invalid_configuration",
    }
}

fn render_error_kind(error: &GpuRenderError) -> &'static str {
    match error {
        GpuRenderError::Scene(_) => "scene_compile",
        GpuRenderError::OutOfMemory { .. } => "out_of_memory",
        GpuRenderError::DeviceLost { .. } => "device_lost",
        GpuRenderError::Validation { .. } => "gpu_validation",
        GpuRenderError::Internal { .. } => "gpu_internal",
        GpuRenderError::SurfaceValidation => "surface_validation",
        GpuRenderError::SurfaceRecreation { .. } => "surface_recreation",
        GpuRenderError::TextAtlasFull => "text_atlas_full",
        GpuRenderError::TextRender { .. } => "text_render",
        GpuRenderError::FaultInjection { .. } => "fault_injection",
    }
}

fn print_failure(
    config: Option<&Config>,
    phase: &'static str,
    kind: &'static str,
    message: impl Into<String>,
) {
    let fallback = NativeTextSettings::default();
    let fallback_config = Config {
        exit_after: None,
        typing_bench: false,
        typing_samples: DEFAULT_INJECT_TOTAL,
        typing_interval: DEFAULT_INJECT_INTERVAL,
        flood: false,
        scale_after: None,
        scale_factor: 1.5,
        stress: None,
        stress_interval: None,
        fault: None,
        fault_after: Duration::from_secs(1),
        memory_interval: DEFAULT_MEMORY_INTERVAL,
        text_settings: fallback,
        harness_project_path: None,
    };
    let config = config.unwrap_or(&fallback_config);
    let evidence = RunEvidence {
        schema_version: 1,
        outcome: OutcomeEvidence::failure(phase, kind, message),
        platform: PlatformEvidence {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        },
        gpu: None,
        display_refresh_hz: None,
        first_usable_frame_ms: None,
        first_usable_frame_within_1s: None,
        workload: workload_evidence(config, Duration::ZERO),
        input_to_present_ms: MetricSummary::default(),
        frame_ms: MetricSummary::default(),
        stress: None,
        fault_injection: config.fault.map(|fault| FaultEvidence {
            requested: fault.label(),
            injected: false,
            post_recovery_present: false,
        }),
        memory: MemorySummary::default(),
        lifecycle: LifecycleEvidence::default(),
        notes: "run failed before native measurement evidence was available",
    };
    println!(
        "{}",
        serde_json::to_string(&evidence).expect("evidence is serializable")
    );
}

fn process_rss_bytes() -> Option<u64> {
    #[cfg(unix)]
    {
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then_some(())
            .and_then(|()| parse_ps_rss_kib(&output.stdout))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn parse_ps_rss_kib(stdout: &[u8]) -> Option<u64> {
    std::str::from_utf8(stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

#[derive(Debug)]
enum UserEvent {
    Wake,
    WatchdogExpired(Arc<AtomicBool>),
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

struct App {
    config: Config,
    app_config: Option<AppConfig>,
    wake_proxy: EventLoopProxy<UserEvent>,
    host: Option<FrontendHost>,
    window: Option<std::sync::Arc<Window>>,
    gpu: Option<GpuText>,
    gpu_evidence: Option<GpuEvidence>,
    display_refresh_hz: Option<f64>,
    clipboard: Option<arboard::Clipboard>,

    input_to_present: Samples,
    frame_ms: Samples,
    memory: MemorySamples,
    next_memory_sample: Instant,
    memory_trend_at: Option<Instant>,
    pending_inputs: VecDeque<Instant>,
    dirty_from_runtime: bool,
    last_present: Option<Instant>,
    stress: Option<StressState>,
    lifecycle: LifecycleEvidence,
    fault_at: Option<Instant>,
    fault_injected: bool,
    awaiting_recovery_present: bool,
    post_recovery_present: bool,
    consecutive_surface_recoveries: u8,
    consecutive_device_recoveries: u8,
    first_usable_frame_ms: Option<f64>,

    start: Instant,
    deadline: Option<Instant>,
    next_heartbeat: Instant,
    summary_printed: bool,
    fatal_error: Option<String>,
    failure_phase: &'static str,
    failure_kind: &'static str,

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
    fn new(
        config: Config,
        proxy: EventLoopProxy<UserEvent>,
        app_config: AppConfig,
        process_start: Instant,
    ) -> Self {
        let now = Instant::now();
        let scale_probe_at = config
            .scale_after
            .map(|seconds| now + Duration::from_secs_f64(seconds));
        Self {
            config,
            app_config: Some(app_config),
            wake_proxy: proxy,
            host: None,
            window: None,
            gpu: None,
            gpu_evidence: None,
            display_refresh_hz: None,
            clipboard: None,
            input_to_present: Samples::with_limit(MAX_MEASUREMENT_SAMPLES),
            frame_ms: Samples::with_limit(MAX_MEASUREMENT_SAMPLES),
            memory: MemorySamples::default(),
            next_memory_sample: now,
            memory_trend_at: None,
            pending_inputs: VecDeque::new(),
            dirty_from_runtime: false,
            last_present: None,
            stress: None,
            lifecycle: LifecycleEvidence::default(),
            fault_at: None,
            fault_injected: false,
            awaiting_recovery_present: false,
            post_recovery_present: false,
            consecutive_surface_recoveries: 0,
            consecutive_device_recoveries: 0,
            first_usable_frame_ms: None,
            start: process_start,
            deadline: None,
            next_heartbeat: now + HEARTBEAT,
            summary_printed: false,
            fatal_error: None,
            failure_phase: "runtime",
            failure_kind: "runtime_error",
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

    fn fail(&mut self, phase: &'static str, kind: &'static str, message: impl Into<String>) {
        self.failure_phase = phase;
        self.failure_kind = kind;
        self.fatal_error = Some(message.into());
    }

    fn capture_gpu_evidence(&mut self) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        let metadata = gpu.adapter_metadata();
        self.gpu_evidence = Some(GpuEvidence {
            name: metadata.name.clone(),
            backend: metadata.backend.to_owned(),
            device_type: metadata.device_type.to_owned(),
            driver: metadata.driver.clone(),
            driver_info: metadata.driver_info.clone(),
            vendor: metadata.vendor,
            device: metadata.device,
        });
    }

    fn capture_lifecycle_evidence(&mut self) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        let snapshot = gpu.lifecycle_snapshot();
        self.lifecycle.device_generation = snapshot.device_generation;
        self.lifecycle.surface_generation = snapshot.surface_generation;
        self.lifecycle.renderer_surface_reconfigurations = snapshot.surface_reconfigurations;
        self.lifecycle.device_recreations = self
            .lifecycle
            .device_recreations
            .max(snapshot.device_recreations);
        self.lifecycle.injected_faults = snapshot.injected_faults;
        self.lifecycle.quad_capacity_floats = snapshot.quad_capacity_floats;
        self.lifecycle.raster_capacity_floats = snapshot.raster_capacity_floats;
        self.lifecycle.text_row_capacity = snapshot.text_row_capacity;
        self.lifecycle.raster_cache_entries = snapshot.raster_cache_entries;
        self.lifecycle.raster_cache_entries_high_water = snapshot.raster_cache_entries_high_water;
        self.lifecycle.raster_cache_bytes = snapshot.raster_cache_bytes;
        self.lifecycle.raster_cache_bytes_high_water = snapshot.raster_cache_bytes_high_water;
    }

    fn scene_size(&self) -> Option<SceneSize> {
        let gpu = self.gpu.as_ref()?;
        let (width, height) = gpu.surface_size();
        scene_size_from_metrics(width, height, gpu.cell_w(), gpu.cell_h())
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
        if let Some(gpu) = &mut self.gpu
            && let Err(error) = gpu.set_scale(scale_factor)
        {
            self.fail("runtime", "invalid_scale", error);
            return;
        }
        self.refresh_mouse_cell();
        self.resize_host();
        self.request_redraw();
    }

    fn send_input(&mut self, input: InputEvent, measured: bool, at: Instant) {
        // Only the isolated typing benchmark has a causal contract: it waits
        // for terminal runtime output produced by the injected character.
        // Flood/soak and interactive input use separate responsiveness
        // evidence and must not be correlated with unrelated runtime drains.
        if measured && self.config.typing_bench {
            if self.pending_inputs.len() < 64 {
                self.pending_inputs.push_back(at);
            } else {
                self.lifecycle.input_correlation_drops =
                    self.lifecycle.input_correlation_drops.saturating_add(1);
                self.input_to_present.miss();
            }
        }
        self.host_mut().handle_input(input);
        self.apply_effects();
        self.request_redraw();
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
            }
        }
    }

    fn drain_runtime(&mut self) -> bool {
        let drained = self.host_mut().drain_runtime_bounded(EVENT_DRAIN_BUDGET);
        if drained > 0 {
            self.dirty_from_runtime = true;
        }
        self.apply_effects();
        drained == EVENT_DRAIN_BUDGET
    }

    fn render_frame(&mut self) -> Result<(), GpuRenderError> {
        self.scene_presentable = false;
        let Some(size) = self.scene_size() else {
            self.cancel_and_disable_ime();
            self.host_mut().suspend_scene_interaction();
            return Ok(());
        };
        let snapshot = self.host_mut().frame(size);
        if scene_is_suspended_by_tiled_minimum(&snapshot.scene) {
            self.cancel_and_disable_ime();
            self.host_mut().suspend_scene_interaction();
            return Ok(());
        }
        self.sync_ime(&snapshot.scene);
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let outcome = match gpu.render(&snapshot.scene, &snapshot.theme) {
            Ok(outcome) => outcome,
            Err(GpuRenderError::DeviceLost { .. }) => {
                self.consecutive_device_recoveries =
                    self.consecutive_device_recoveries.saturating_add(1);
                if self.consecutive_device_recoveries > 3 {
                    self.fail(
                        "runtime",
                        "device_recovery_exhausted",
                        "GPU device recovery exceeded three consecutive attempts",
                    );
                    return Ok(());
                }
                match pollster::block_on(gpu.recreate_device()) {
                    Ok(()) => {
                        self.lifecycle.device_recreations =
                            self.lifecycle.device_recreations.saturating_add(1);
                        self.scene_presentable = false;
                        self.host_mut().suspend_scene_interaction();
                        self.capture_gpu_evidence();
                        self.resize_host();
                        self.awaiting_recovery_present = true;
                        self.request_redraw();
                    }
                    Err(error) => {
                        self.lifecycle.device_recreation_failures =
                            self.lifecycle.device_recreation_failures.saturating_add(1);
                        self.fail(
                            "runtime",
                            startup_error_kind(error.kind()),
                            format!("GPU device recreation failed: {error}"),
                        );
                    }
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        match outcome {
            GpuRenderOutcome::Presented(present) => {
                self.scene_presentable = true;
                self.consecutive_surface_recoveries = 0;
                self.consecutive_device_recoveries = 0;
                self.first_usable_frame_ms.get_or_insert_with(|| {
                    present.duration_since(self.start).as_secs_f64() * 1_000.0
                });
                if self.awaiting_recovery_present {
                    self.awaiting_recovery_present = false;
                    self.post_recovery_present = true;
                }
                if let Some(stress) = &mut self.stress {
                    stress.presented();
                }
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
            GpuRenderOutcome::Skipped(reason) => {
                self.scene_presentable = false;
                self.host_mut().suspend_scene_interaction();
                self.frame_ms.miss();
                match reason {
                    GpuFrameSkip::Timeout => {
                        self.lifecycle.timeout_skips =
                            self.lifecycle.timeout_skips.saturating_add(1);
                    }
                    GpuFrameSkip::Occluded => {
                        self.lifecycle.occluded_skips =
                            self.lifecycle.occluded_skips.saturating_add(1);
                        if matches!(self.config.stress, Some(StressConfig::Soak { .. })) {
                            self.fail(
                                "runtime",
                                "measurement_occluded",
                                "GPU surface became occluded during the active soak",
                            );
                        }
                    }
                }
            }
            GpuRenderOutcome::SurfaceReconfigured(recovery) => {
                self.scene_presentable = false;
                self.host_mut().suspend_scene_interaction();
                match recovery {
                    GpuSurfaceRecovery::Outdated => {
                        self.lifecycle.outdated_reconfigurations =
                            self.lifecycle.outdated_reconfigurations.saturating_add(1);
                    }
                    GpuSurfaceRecovery::Lost => {
                        self.lifecycle.lost_reconfigurations =
                            self.lifecycle.lost_reconfigurations.saturating_add(1);
                    }
                }
                self.consecutive_surface_recoveries =
                    self.consecutive_surface_recoveries.saturating_add(1);
                if self.consecutive_surface_recoveries > 8 {
                    self.fail(
                        "runtime",
                        "surface_recovery_exhausted",
                        "GPU surface recovery exceeded eight consecutive attempts",
                    );
                }
            }
        }
        Ok(())
    }

    fn maybe_inject(&mut self, now: Instant) {
        if !self.config.typing_bench {
            return;
        }
        while self.injected < self.config.typing_samples && now >= self.next_inject {
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
            self.next_inject += self.config.typing_interval;
        }
    }

    fn maybe_stress(&mut self, now: Instant) {
        let action = self.stress.as_mut().and_then(|stress| stress.issue(now));
        let Some(action) = action else {
            if self
                .stress
                .as_ref()
                .is_some_and(|stress| stress.is_finished(now))
                && matches!(
                    self.config.stress,
                    Some(StressConfig::ResizeExercise { .. })
                )
            {
                let completion_deadline = now + Duration::from_secs(1);
                self.deadline = Some(self.deadline.map_or(completion_deadline, |deadline| {
                    deadline.min(completion_deadline)
                }));
            }
            return;
        };
        if let Some(window) = &self.window {
            if matches!(self.config.stress, Some(StressConfig::Soak { .. })) {
                window.focus_window();
            }
            let _ = window.request_inner_size(PhysicalSize::new(action.width, action.height));
        }
        self.apply_scale_factor(action.scale);
        if self.fatal_error.is_none()
            && let Some(stress) = &mut self.stress
        {
            stress.mark_scale_applied(action.sequence);
        }
        if action.restart_flood {
            self.send_input(InputEvent::Paste("seq 1 200000\n".to_owned()), false, now);
        }
        if action.inject_input {
            self.send_input(InputEvent::Key(InputKey::ctrl('l')), false, now);
            if let Some(stress) = &mut self.stress {
                stress.mark_input_issued(action.sequence);
            }
        }
    }

    fn maybe_inject_fault(&mut self, now: Instant) {
        let Some(fault) = self.config.fault else {
            return;
        };
        if self.fault_injected || self.fault_at.is_none_or(|fault_at| now < fault_at) {
            return;
        }
        let result = self
            .gpu
            .as_mut()
            .ok_or_else(|| "GPU is unavailable".to_owned())
            .and_then(|gpu| {
                gpu.inject_fault(fault.injection())
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(GpuFaultInjectionResult::SurfaceReconfigured(recovery)) => {
                self.fault_injected = true;
                match recovery {
                    GpuSurfaceRecovery::Outdated => {
                        self.lifecycle.outdated_reconfigurations =
                            self.lifecycle.outdated_reconfigurations.saturating_add(1);
                    }
                    GpuSurfaceRecovery::Lost => {
                        self.lifecycle.lost_reconfigurations =
                            self.lifecycle.lost_reconfigurations.saturating_add(1);
                    }
                }
                self.awaiting_recovery_present = true;
                self.request_redraw();
            }
            Ok(GpuFaultInjectionResult::FaultQueued) => {
                self.fault_injected = true;
                if fault != FaultConfig::OutOfMemory {
                    self.awaiting_recovery_present = true;
                }
                self.request_redraw();
            }
            Err(error) => self.fail("runtime", "fault_injection", error),
        }
        self.fault_at = None;
    }

    fn print_summary(&mut self) {
        if self.summary_printed {
            return;
        }
        self.summary_printed = true;
        self.memory.push(process_rss_bytes());
        self.capture_lifecycle_evidence();
        if let Some(fault) = self.config.fault
            && self.fatal_error.is_none()
        {
            if !self.fault_injected {
                self.fail(
                    "runtime",
                    "fault_not_injected",
                    format!("requested {} fault was not injected", fault.label()),
                );
            } else if fault != FaultConfig::OutOfMemory && !self.post_recovery_present {
                self.fail(
                    "runtime",
                    "recovery_unverified",
                    format!(
                        "injected {} fault did not produce a post-recovery present",
                        fault.label()
                    ),
                );
            }
        }
        let now = Instant::now();
        let stress = self.stress.as_mut().map(|stress| stress.finish(now));
        let memory = self.memory.summary();
        if self.fatal_error.is_none()
            && matches!(self.config.stress, Some(StressConfig::Soak { .. }))
            && (self.lifecycle.occluded_skips > 0 || self.lifecycle.window_occlusion_events > 0)
        {
            self.fail(
                "runtime",
                "measurement_occluded",
                format!(
                    "soak observed {} GPU occlusion skips and {} window occlusion events",
                    self.lifecycle.occluded_skips, self.lifecycle.window_occlusion_events
                ),
            );
        }
        if self.fatal_error.is_none()
            && let Some(stress) = stress
            && !stress.completed
        {
            self.fail(
                "runtime",
                "stress_incomplete",
                format!(
                    "stress run incomplete: issued={} applied={} presented={} misses={}",
                    stress.issued, stress.changes_applied, stress.presented, stress.misses
                ),
            );
        }
        if self.fatal_error.is_none()
            && matches!(self.config.stress, Some(StressConfig::Soak { .. }))
        {
            match memory.monotonic_growth {
                Some(false) if memory.misses == 0 => {}
                Some(true) => self.fail(
                    "runtime",
                    "monotonic_memory_growth",
                    format!(
                        "post-warmup RSS grew monotonically by {} bytes",
                        memory.trend_delta_rss_bytes
                    ),
                ),
                _ => self.fail(
                    "runtime",
                    "inconclusive_memory_evidence",
                    format!(
                        "soak requires at least three post-warmup RSS samples and zero misses; samples={} misses={}",
                        memory.trend_sample_count, memory.misses
                    ),
                ),
            }
        }
        if self.fatal_error.is_none()
            && self.gpu_evidence.is_some()
            && self.first_usable_frame_ms.is_none()
        {
            self.fail(
                "runtime",
                "no_usable_frame",
                "GPU initialized but no usable frame was presented",
            );
        }
        let outcome = self.fatal_error.as_ref().map_or_else(
            || OutcomeEvidence::success("native shell exited cleanly"),
            |error| {
                OutcomeEvidence::failure(self.failure_phase, self.failure_kind, error.to_owned())
            },
        );
        let mut input_to_present = self.input_to_present.summary();
        input_to_present.misses = input_to_present
            .misses
            .saturating_add(self.pending_inputs.len() as u64);
        let evidence = RunEvidence {
            schema_version: 1,
            outcome,
            platform: PlatformEvidence {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            },
            gpu: self.gpu_evidence.clone(),
            display_refresh_hz: self.display_refresh_hz,
            first_usable_frame_ms: self.first_usable_frame_ms,
            first_usable_frame_within_1s: self.first_usable_frame_ms.map(|ms| ms <= 1_000.0),
            workload: workload_evidence(&self.config, now.saturating_duration_since(self.start)),
            input_to_present_ms: input_to_present,
            frame_ms: self.frame_ms.summary(),
            stress,
            fault_injection: self.config.fault.map(|fault| FaultEvidence {
                requested: fault.label(),
                injected: self.fault_injected,
                post_recovery_present: self.post_recovery_present,
            }),
            memory,
            lifecycle: self.lifecycle,
            notes: "FrontendHost is preserved across renderer recovery; input-to-present is emitted only by the isolated typing benchmark; soak input is action-counted without claiming causal latency; frame timing excludes idle gaps >=250ms",
        };
        println!(
            "{}",
            serde_json::to_string(&evidence).expect("evidence is serializable")
        );
    }

    fn exit_if_requested(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.host().should_quit() {
            return false;
        }
        self.cancel_and_disable_ime();
        self.shutdown_host();
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
        let redraw = kind != PointerKind::Move || self.host().pointer_move_needs_redraw();
        let (column, row) = self.mouse_cell;
        let input = InputEvent::Pointer(PointerEvent {
            kind,
            button,
            column,
            row,
            mods: neutral_modifiers(self.modifiers),
        });
        self.host_mut().handle_input(input);
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
        if let Some(host) = &mut self.host {
            host.handle_input(InputEvent::Composition(CompositionEvent::Cancel));
        }
    }

    /// Service timers at both the start and end of each winit event batch.
    /// Continuous PTY wake/redraw traffic can prevent `about_to_wait` from
    /// running, so it cannot be the sole owner of deadlines or stress cadence.
    fn service_scheduled_work(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.window.is_none() {
            return false;
        }
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
            self.shutdown_host();
            self.print_summary();
            event_loop.exit();
            return true;
        }
        if now >= self.next_heartbeat {
            self.host_mut().heartbeat();
            self.next_heartbeat = now + HEARTBEAT;
            self.request_redraw();
        }
        if now >= self.next_memory_sample {
            if self.memory_trend_at.is_some_and(|trend_at| now >= trend_at) {
                self.memory.begin_trend();
                self.memory_trend_at = None;
            }
            self.memory.push(process_rss_bytes());
            self.next_memory_sample = now + self.config.memory_interval;
        }
        self.maybe_inject(now);
        self.maybe_stress(now);
        self.maybe_inject_fault(now);
        if self.fatal_error.is_some() {
            self.cancel_and_disable_ime();
            self.shutdown_host();
            self.print_summary();
            event_loop.exit();
            return true;
        }
        false
    }

    fn schedule_next_wake(&self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next = self.deadline.map_or(self.next_heartbeat, |deadline| {
            deadline.min(self.next_heartbeat)
        });
        next = next.min(self.next_memory_sample);
        if let Some(scale_probe_at) = self.scale_probe_at {
            next = next.min(scale_probe_at);
        }
        if let Some(fault_at) = self.fault_at {
            next = next.min(fault_at);
        }
        if self.config.typing_bench && self.injected < self.config.typing_samples {
            next = next.min(self.next_inject);
        }
        if let Some(stress) = &self.stress
            && !stress.is_finished(now)
        {
            next = next.min(stress.next_at());
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
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
        let text_settings = self.config.text_settings.clone();
        let focus_for_soak = matches!(self.config.stress, Some(StressConfig::Soak { .. }));
        let app_config = self
            .app_config
            .take()
            .expect("native startup configuration is consumed exactly once");
        let wake_proxy = self.wake_proxy.clone();
        let startup = start_after_preflight(
            || {
                let attributes = Window::default_attributes().with_title("Mandatum GPU Host Spike");
                let window = event_loop.create_window(attributes).map_err(|error| {
                    GpuStartupError::no_display(format!("no window (headless?): {error}"))
                })?;
                #[cfg(target_os = "macos")]
                window.set_option_as_alt(OptionAsAlt::OnlyRight);
                if focus_for_soak {
                    window.focus_window();
                }
                Ok(std::sync::Arc::new(window))
            },
            |window| pollster::block_on(GpuText::new(window.clone(), text_settings)),
            || {
                FrontendHost::new_with_wake_callback(app_config, move || {
                    let _ = wake_proxy.send_event(UserEvent::Wake);
                })
            },
        );
        let (window, gpu, mut host) = match startup {
            Ok(started) => started,
            Err(error) => {
                self.fail(
                    "startup",
                    startup_error_kind(error.kind()),
                    error.to_string(),
                );
                self.print_summary();
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
        self.display_refresh_hz = window
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz())
            .map(|millihertz| f64::from(millihertz) / 1_000.0);
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.host = Some(host);
        self.capture_gpu_evidence();
        self.resize_host();

        let ready = Instant::now();
        self.next_heartbeat = ready + HEARTBEAT;
        self.next_memory_sample = ready;
        self.next_inject = ready + Duration::from_millis(400);
        self.fault_at = self.config.fault.map(|_| ready + self.config.fault_after);
        self.stress = self.config.stress.map(|stress| match stress {
            StressConfig::ResizeExercise { steps } => StressState::resize_exercise(
                ready + Duration::from_millis(400),
                steps,
                self.config
                    .stress_interval
                    .unwrap_or(DEFAULT_RESIZE_INTERVAL),
            ),
            StressConfig::Soak { duration } => StressState::soak(
                ready + Duration::from_millis(400),
                duration,
                self.config.stress_interval.unwrap_or(DEFAULT_SOAK_INTERVAL),
            ),
        });
        if let Some(StressConfig::Soak { duration }) = self.config.stress {
            // The flood takes minutes to fill bounded PTY scrollback/output
            // capacity. Judge leak behavior over the steady-state second half
            // rather than misclassifying that one-time high-water ramp.
            let warmup_seconds = (duration.as_secs() / 2).clamp(1, 15 * 60);
            self.memory.pause_trend();
            self.memory_trend_at = Some(ready + Duration::from_secs(warmup_seconds));
        }
        self.deadline = configured_run_timeout(&self.config).map(|timeout| ready + timeout);
        if self.config.flood && !matches!(self.config.stress, Some(StressConfig::Soak { .. })) {
            self.send_input(
                InputEvent::Paste("seq 1 200000\n".to_owned()),
                false,
                Instant::now(),
            );
        }
        self.memory.push(process_rss_bytes());
        self.next_memory_sample = ready + self.config.memory_interval;
        self.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::WatchdogExpired(acknowledged) => {
                // Acknowledge before any orderly shutdown work. The watchdog
                // thread must never hard-exit merely because an already-due
                // normal deadline preempted this event.
                acknowledged.store(true, Ordering::Release);
                self.fail(
                    "runtime",
                    "watchdog",
                    "event loop did not exit within budget",
                );
                self.cancel_and_disable_ime();
                self.shutdown_host();
                self.print_summary();
                event_loop.exit();
            }
            UserEvent::Wake => {
                if !self.service_scheduled_work(event_loop) {
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
                self.print_summary();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                let more_pending = self.drain_runtime();
                if self.exit_if_requested(event_loop) {
                    return;
                }
                if let Err(error) = self.render_frame() {
                    let kind = render_error_kind(&error);
                    self.fail("runtime", kind, error.to_string());
                    self.cancel_and_disable_ime();
                    self.shutdown_host();
                    self.print_summary();
                    event_loop.exit();
                    return;
                }
                if self.fatal_error.is_some() {
                    self.cancel_and_disable_ime();
                    self.shutdown_host();
                    self.print_summary();
                    event_loop.exit();
                    return;
                }
                if more_pending && self.scene_presentable {
                    self.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                self.scene_presentable = false;
                self.cancel_pointer_gesture();
                self.host_mut().suspend_scene_interaction();
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize_surface(size.width, size.height);
                }
                if let Some(stress) = &mut self.stress {
                    stress.observe_resize(size.width, size.height);
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
                        if self.host().handles_workspace_key(shortcut) {
                            self.send_input(InputEvent::Key(shortcut), false, now);
                        } else if let Some(clipboard) = &mut self.clipboard {
                            match clipboard.get_text() {
                                Ok(text) => {
                                    self.send_input(InputEvent::Paste(text), false, now);
                                }
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
                            self.send_input(InputEvent::Key(shortcut), false, now);
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
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = neutral_button(button) {
                    self.pointer_button(state, button);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.pointer_wheel(delta),
            WindowEvent::Focused(focused) => self.focus_changed(focused),
            WindowEvent::Occluded(true) => {
                self.lifecycle.window_occlusion_events =
                    self.lifecycle.window_occlusion_events.saturating_add(1);
                if matches!(self.config.stress, Some(StressConfig::Soak { .. })) {
                    self.fail(
                        "runtime",
                        "measurement_occluded",
                        "window became occluded during the active soak",
                    );
                    self.cancel_and_disable_ime();
                    self.shutdown_host();
                    self.print_summary();
                    event_loop.exit();
                }
            }
            WindowEvent::Occluded(false) => {}
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

fn uses_isolated_harness(config: &Config) -> bool {
    config.typing_bench || config.flood || config.stress.is_some() || config.fault.is_some()
}

fn app_config_for_run(config: &mut Config) -> std::io::Result<AppConfig> {
    if uses_isolated_harness(config) {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let project_path = std::env::temp_dir().join(format!(
            "mandatum-native-harness-{}-{nonce}",
            std::process::id()
        ));
        // `create_dir` fails on every pre-existing file, directory, or symlink;
        // a harness never reuses stale or attacker-prepared workspace state.
        fs::create_dir(&project_path)?;
        let app_config = AppConfig {
            workspace_name: "Mandatum GPU Harness".to_owned(),
            workspace_file: project_path.join(".mandatum").join("workspace.json"),
            project_path: project_path.clone(),
            shell_program: "/bin/sh".to_owned(),
            spawn_pty: true,
            restore_on_startup: false,
            ..AppConfig::default()
        };
        config.harness_project_path = Some(project_path.display().to_string());
        Ok(app_config)
    } else {
        AppConfig::from_current_dir()
    }
}

fn main() {
    let process_start = Instant::now();
    let mut config = match parse_config() {
        Ok(config) => config,
        Err(error) => {
            print_failure(None, "startup", "invalid_arguments", error);
            std::process::exit(2);
        }
    };
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            print_failure(Some(&config), "startup", "no_display", error.to_string());
            std::process::exit(2);
        }
    };
    let app_config = match app_config_for_run(&mut config) {
        Ok(config) => config,
        Err(error) => {
            print_failure(
                Some(&config),
                "startup",
                "host_initialization",
                error.to_string(),
            );
            std::process::exit(2);
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    if let Some(timeout) = configured_run_timeout(&config) {
        let watchdog_config = config.clone();
        let watchdog_proxy = proxy.clone();
        let watchdog_acknowledged = Arc::new(AtomicBool::new(false));
        let shutdown_acknowledged = watchdog_acknowledged.clone();
        std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || {
                std::thread::sleep(timeout + Duration::from_secs(8));
                if watchdog_proxy
                    .send_event(UserEvent::WatchdogExpired(shutdown_acknowledged))
                    .is_err()
                {
                    return;
                }
                // A responsive event loop performs orderly host shutdown.
                // Hard exit is reserved for an event loop that cannot process
                // the shutdown request at all.
                std::thread::sleep(Duration::from_secs(5));
                if watchdog_acknowledged.load(Ordering::Acquire) {
                    return;
                }
                print_failure(
                    Some(&watchdog_config),
                    "runtime",
                    "watchdog_hard_exit",
                    "event loop ignored the orderly watchdog shutdown request",
                );
                std::process::exit(1);
            })
            .ok();
    }
    let mut app = App::new(config, proxy, app_config, process_start);
    if let Err(error) = event_loop.run_app(&mut app) {
        app.fail(
            "runtime",
            "event_loop",
            format!("event loop error: {error}"),
        );
    }
    // `process::exit` skips Drop, so finalize explicitly before deriving a
    // nonzero process status from any recoverable event-loop failure.
    app.cancel_and_disable_ime();
    app.shutdown_host();
    app.print_summary();
    let exit_code = run_exit_code(app.fatal_error.as_deref());
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MEMORY_INTERVAL, DEFAULT_SOAK_DURATION, FaultConfig, LifecycleEvidence,
        MemorySummary, MetricSummary, OutcomeEvidence, PlatformAction, PlatformEvidence,
        PressedPointerButtons, RunEvidence, StressConfig, WorkloadEvidence, configured_run_timeout,
        ime_event_is_accepted, key_for_platform_translation, pane_geometry_is_suspended,
        parse_config_from, parse_font_family, parse_font_size, parse_ps_rss_kib, parse_scale_delay,
        parse_scale_factor, run_exit_code, scene_size_from_metrics, start_after_preflight,
        translate_ime, translate_key,
    };
    use mandatum_scene::input::{
        CompositionEvent, InputEvent, Key as InputKey, KeyCode, Modifiers, TextRange,
    };
    use winit::{
        event::Ime,
        keyboard::{Key, ModifiersState, NamedKey},
    };

    #[test]
    fn startup_preflight_no_display_never_constructs_gpu_or_host() {
        let mut gpu_constructed = false;
        let mut host_constructed = false;

        let result = start_after_preflight(
            || Err::<(), _>("no display"),
            |_| {
                gpu_constructed = true;
                Ok(())
            },
            || {
                host_constructed = true;
            },
        );

        assert_eq!(result, Err("no display"));
        assert!(!gpu_constructed);
        assert!(!host_constructed);
    }

    #[test]
    fn startup_preflight_no_adapter_never_constructs_host() {
        let mut host_constructed = false;

        let result = start_after_preflight(
            || Ok("window"),
            |_| Err::<(), _>("no adapter"),
            || {
                host_constructed = true;
            },
        );

        assert_eq!(result, Err("no adapter"));
        assert!(!host_constructed);
    }

    #[test]
    fn startup_preflight_surface_and_device_failures_never_construct_host() {
        for failure in ["surface", "device"] {
            let mut host_constructed = false;
            let result = start_after_preflight(
                || Ok("window"),
                |_| Err::<(), _>(failure),
                || {
                    host_constructed = true;
                },
            );

            assert_eq!(result, Err(failure));
            assert!(!host_constructed);
        }
    }

    #[test]
    fn successful_startup_constructs_host_after_window_and_gpu() {
        let order = std::cell::RefCell::new(Vec::new());

        let result = start_after_preflight(
            || {
                order.borrow_mut().push("window");
                Ok::<_, &str>("window")
            },
            |_| {
                order.borrow_mut().push("gpu");
                Ok("gpu")
            },
            || {
                order.borrow_mut().push("host");
                "host"
            },
        );

        assert_eq!(result, Ok(("window", "gpu", "host")));
        assert_eq!(order.into_inner(), ["window", "gpu", "host"]);
    }

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
        assert_eq!(parse_ps_rss_kib(b" 12345\n"), Some(12_641_280));
        assert_eq!(parse_ps_rss_kib(b"not-a-number"), None);
    }

    #[test]
    fn measurement_cli_is_bounded_and_rejects_ambiguous_stress_modes() {
        let resize = parse_config_from(
            [
                "--resize-exercise",
                "--stress-interval-ms",
                "20",
                "--memory-interval-ms",
                "1000",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("bounded resize config");
        assert_eq!(
            resize.stress,
            Some(StressConfig::ResizeExercise { steps: 1_000 })
        );
        assert_eq!(resize.stress_interval.unwrap().as_millis(), 20);
        assert_eq!(resize.memory_interval.as_millis(), 1000);
        assert_eq!(configured_run_timeout(&resize).unwrap().as_secs(), 400);

        let typing = parse_config_from(
            ["--typing-samples", "1000", "--typing-interval-ms", "20"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("isolated typing config");
        assert_eq!(typing.typing_samples, 1000);
        assert_eq!(typing.typing_interval.as_millis(), 20);

        let soak = parse_config_from(["--soak"].into_iter().map(str::to_owned))
            .expect("standard soak config");
        assert_eq!(
            soak.stress,
            Some(StressConfig::Soak {
                duration: DEFAULT_SOAK_DURATION
            })
        );
        assert!(soak.flood);
        assert_eq!(
            configured_run_timeout(&soak).unwrap(),
            DEFAULT_SOAK_DURATION + std::time::Duration::from_millis(400)
        );

        let fault = parse_config_from(
            ["--inject-fault", "device-lost", "--fault-after", "0.5"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("bounded fault config");
        assert_eq!(fault.fault, Some(FaultConfig::DeviceLost));
        assert_eq!(fault.fault_after.as_millis(), 500);

        for invalid in [
            vec!["--resize-count", "0"],
            vec!["--soak-seconds", "21601"],
            vec!["--memory-interval-ms", "10"],
            vec!["--resize-exercise", "--soak"],
            vec!["--unknown"],
            vec!["--inject-fault", "spontaneous-magic"],
            vec!["--fault-after", "-1"],
            vec!["--fault-after", "1"],
            vec!["--typing-interval-ms", "10"],
            vec!["--stress-interval-ms", "100"],
            vec!["--scale-factor", "2"],
            vec!["--typing-bench", "--flood"],
            vec!["--typing-bench", "--resize-exercise"],
        ] {
            assert!(
                parse_config_from(invalid.into_iter().map(str::to_owned)).is_err(),
                "invalid config was accepted"
            );
        }
    }

    #[test]
    fn startup_evidence_schema_keeps_unavailable_first_frame_explicitly_null() {
        let evidence = RunEvidence {
            schema_version: 1,
            outcome: OutcomeEvidence::failure("startup", "no_display", "headless"),
            platform: PlatformEvidence {
                os: "test-os",
                arch: "test-arch",
            },
            gpu: None,
            display_refresh_hz: None,
            first_usable_frame_ms: None,
            first_usable_frame_within_1s: None,
            workload: WorkloadEvidence {
                typing_bench: false,
                typing_target: 0,
                typing_interval_ms: None,
                flood: false,
                stress: "none",
                stress_target: None,
                soak_seconds: None,
                stress_interval_ms: None,
                memory_interval_ms: DEFAULT_MEMORY_INTERVAL.as_millis() as u64,
                injected_fault: None,
                fault_after_ms: None,
                scale_after_ms: None,
                scale_factor: 1.5,
                font_family: "monospace".to_owned(),
                font_size: 15.0,
                harness_project_path: None,
                window_visibility_policy: "normal",
                elapsed_ms: 0,
            },
            input_to_present_ms: MetricSummary::default(),
            frame_ms: MetricSummary::default(),
            stress: None,
            fault_injection: None,
            memory: MemorySummary::default(),
            lifecycle: LifecycleEvidence::default(),
            notes: "test",
        };
        let json = serde_json::to_value(evidence).expect("schema serializes");
        assert!(json["first_usable_frame_ms"].is_null());
        assert!(json["first_usable_frame_within_1s"].is_null());
        assert!(json["input_to_present_ms"]["p50"].is_null());
        assert_eq!(json["outcome"]["kind"], "no_display");
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
