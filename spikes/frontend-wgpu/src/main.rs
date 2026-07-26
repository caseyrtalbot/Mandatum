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

use mandatum_app::{
    AppConfig, FrameSnapshot, FrontendEffect, FrontendHost, PreparedVisualScenario,
    VisualScenarioId, prepare_visual_scenario,
};
use mandatum_native_renderer::{
    ActiveTransitionWindow, GpuFaultInjection, GpuFaultInjectionResult, GpuFrameSkip,
    GpuRenderError, GpuRenderOutcome, GpuStartupError, GpuStartupErrorKind, GpuSurfaceRecovery,
    GpuText, NativeTextSettings, prepare_token_sampler,
};
use mandatum_scene::{
    BackingScale, LogicalPoint, LogicalSize, PaneContent, PhysicalSize as ScenePhysicalSize,
    SceneCell, SceneCellStyle, SceneColor, SceneSize, Theme, TransitionRole, UiColor,
    ViewportMetrics, WorkspaceScene,
    input::{
        CompositionEvent, InputEvent, Key as InputKey, KeyCode, Modifiers, PointerButton,
        PointerEvent, PointerKind, TextRange,
    },
};
use serde::Serialize;
use stats::{
    MemorySamples, MemorySummary, MetricSummary, RefreshIntervalSummary, Samples, StressState,
    StressSummary, duration_delta_ms, one_core_cpu_percent, parse_process_cpu_time,
    refresh_interval_summary, stress_checkpoint_action,
};
#[cfg(target_os = "macos")]
use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize as WindowLogicalSize, PhysicalPosition, PhysicalSize},
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
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_IDLE_WARMUP: Duration = Duration::from_secs(5);
const TRANSITION_ACTION_INTERVAL: Duration = Duration::from_millis(180);
const MAX_MEASUREMENT_SAMPLES: usize = 200_000;
const HEARTBEAT: Duration = Duration::from_millis(250);
const VISUAL_SCENARIO_SETTLE_DELAY: Duration = Duration::from_millis(300);
const VISUAL_CHECKPOINT_RETRY_DELAY: Duration = Duration::from_millis(50);
const MAX_VISUAL_CHECKPOINT_RETRIES: u8 = 40;
const EVENT_DRAIN_BUDGET: usize = 16;
const IDLE_FRAME_CUTOFF_MS: f64 = 250.0;

fn animation_redraw_is_due(now: Instant, deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| now >= deadline)
}

fn contiguous_animation_interval(
    previous: Option<Instant>,
    present: Instant,
    was_active: bool,
    is_active: bool,
) -> (Option<Duration>, Option<Instant>) {
    if !was_active && !is_active {
        return (None, None);
    }
    (
        previous.map(|previous| present.saturating_duration_since(previous)),
        is_active.then_some(present),
    )
}

fn checkpoint_instant(
    checkpoint: VisualCheckpoint,
    origin: Instant,
    window: Option<ActiveTransitionWindow>,
) -> Result<Instant, GpuRenderError> {
    match checkpoint {
        VisualCheckpoint::Reduced => Ok(origin),
        VisualCheckpoint::Start | VisualCheckpoint::Midpoint | VisualCheckpoint::End => {
            let window = window.ok_or_else(|| GpuRenderError::Internal {
                message: format!(
                    "{} checkpoint did not start an ApprovalArrival transition",
                    checkpoint.as_str()
                ),
            })?;
            if window.started_at != origin {
                return Err(GpuRenderError::Internal {
                    message: format!(
                        "{} checkpoint ApprovalArrival started at an unexpected instant",
                        checkpoint.as_str()
                    ),
                });
            }
            Ok(match checkpoint {
                VisualCheckpoint::Start => window.started_at,
                VisualCheckpoint::Midpoint => window.midpoint(),
                VisualCheckpoint::End => window.finishes_at,
                VisualCheckpoint::Reduced => unreachable!(),
            })
        }
    }
}

fn retained_visual_checkpoint_snapshot<T: Clone>(
    origin: Option<Instant>,
    snapshot: Option<&T>,
) -> Option<T> {
    origin.and_then(|_| snapshot.cloned())
}

fn checkpoint_freeze_after_outcome(
    current: Option<Instant>,
    candidate: Option<Instant>,
    presented: bool,
) -> Option<Instant> {
    if presented {
        candidate.or(current)
    } else {
        current
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VisualCheckpoint {
    Start,
    Midpoint,
    End,
    Reduced,
}

impl VisualCheckpoint {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Midpoint => "midpoint",
            Self::End => "end",
            Self::Reduced => "reduced",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "start" => Some(Self::Start),
            "midpoint" => Some(Self::Midpoint),
            "end" => Some(Self::End),
            "reduced" => Some(Self::Reduced),
            _ => None,
        }
    }

    const fn reference_id(self) -> &'static str {
        match self {
            Self::Start => "attention-motion-start",
            Self::Midpoint => "attention-motion-midpoint",
            Self::End => "attention-motion-end",
            Self::Reduced => "attention-reduced",
        }
    }
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
    shaping_cache_enabled: bool,
    text_settings: NativeTextSettings,
    harness_project_path: Option<String>,
    visual_scenario: Option<VisualScenarioId>,
    visual_theme: Option<Theme>,
    token_sampler: bool,
    display_name: Option<String>,
    visual_transition_exercise: Option<Duration>,
    idle_warmup: Option<Duration>,
    idle_measure: Option<Duration>,
    visual_checkpoint: Option<VisualCheckpoint>,
}

fn parse_config() -> Result<Config, String> {
    parse_config_from(std::env::args().skip(1))
}

fn display_names_match(requested: &str, candidate: &str) -> bool {
    requested == candidate || (requested == "Built-in Retina Display" && candidate == "Color LCD")
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
        shaping_cache_enabled: true,
        text_settings: defaults,
        harness_project_path: None,
        visual_scenario: None,
        visual_theme: None,
        token_sampler: false,
        display_name: None,
        visual_transition_exercise: None,
        idle_warmup: None,
        idle_measure: None,
        visual_checkpoint: None,
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
            "--disable-shaping-cache" => config.shaping_cache_enabled = false,
            "--visual-scenario" => {
                let value = required_value(&mut args, &arg)?;
                config.visual_scenario = Some(
                    value
                        .parse::<VisualScenarioId>()
                        .map_err(|error| error.to_string())?,
                );
            }
            "--visual-theme" => {
                let value = required_value(&mut args, &arg)?;
                config.visual_theme = Some(
                    Theme::BUILTIN_NAMES
                        .contains(&value.as_str())
                        .then(|| Theme::builtin(&value))
                        .flatten()
                        .ok_or_else(|| {
                            invalid_value(&arg, &value, &Theme::BUILTIN_NAMES.join("|"))
                        })?,
                );
            }
            "--token-sampler" => config.token_sampler = true,
            "--display" => {
                let value = required_value(&mut args, &arg)?;
                config.display_name = Some(parse_font_family(&value).ok_or_else(|| {
                    invalid_value(&arg, &value, "1..=128 non-control characters")
                })?);
            }
            "--visual-transition-exercise-seconds" => {
                let value = required_value(&mut args, &arg)?;
                let seconds = parse_bounded_u64(&value, 1, 3_600)
                    .ok_or_else(|| invalid_value(&arg, &value, "1..=3600 seconds"))?;
                config.visual_transition_exercise = Some(Duration::from_secs(seconds));
            }
            "--warmup-seconds" => {
                let value = required_value(&mut args, &arg)?;
                let seconds = parse_bounded_u64(&value, 0, 3_600)
                    .ok_or_else(|| invalid_value(&arg, &value, "0..=3600 seconds"))?;
                config.idle_warmup = Some(Duration::from_secs(seconds));
            }
            "--idle-measure-seconds" => {
                let value = required_value(&mut args, &arg)?;
                let seconds = parse_bounded_u64(&value, 1, 3_600)
                    .ok_or_else(|| invalid_value(&arg, &value, "1..=3600 seconds"))?;
                config.idle_measure = Some(Duration::from_secs(seconds));
            }
            "--visual-checkpoint" => {
                let value = required_value(&mut args, &arg)?;
                config.visual_checkpoint =
                    Some(VisualCheckpoint::parse(&value).ok_or_else(|| {
                        invalid_value(&arg, &value, "start|midpoint|end|reduced")
                    })?);
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
    if config.visual_scenario.is_some()
        && (config.typing_bench
            || config.flood
            || config.stress.is_some()
            || config.fault.is_some())
    {
        return Err(
            "visual scenarios must run in isolation from measurement, stress, and fault workloads"
                .to_owned(),
        );
    }
    if config.idle_warmup.is_some() && config.idle_measure.is_none() {
        return Err("--warmup-seconds requires --idle-measure-seconds".to_owned());
    }
    if config.idle_measure.is_some() && config.visual_transition_exercise.is_some() {
        return Err(
            "idle measurement and visual transition exercise must run in isolation".to_owned(),
        );
    }
    if (config.idle_measure.is_some() || config.visual_transition_exercise.is_some())
        && (config.typing_bench
            || config.flood
            || config.stress.is_some()
            || config.fault.is_some()
            || config.token_sampler
            || config.scale_after.is_some()
            || config.exit_after.is_some())
    {
        return Err(
            "visual transition and idle measurements must run in isolation from other workloads and --exit-after"
                .to_owned(),
        );
    }
    if config.idle_measure.is_some() {
        if config
            .visual_scenario
            .is_some_and(|scenario| scenario != VisualScenarioId::CalmTerminal)
        {
            return Err(
                "--idle-measure-seconds requires the calm-terminal visual scenario".to_owned(),
            );
        }
        config.visual_scenario = Some(VisualScenarioId::CalmTerminal);
        config.idle_warmup.get_or_insert(DEFAULT_IDLE_WARMUP);
    }
    if config.visual_transition_exercise.is_some() && config.visual_scenario.is_none() {
        config.visual_scenario = Some(VisualScenarioId::DenseWorkspace);
    }
    if config.visual_checkpoint.is_some()
        && config.visual_scenario != Some(VisualScenarioId::Attention)
    {
        return Err(
            "--visual-checkpoint is valid only with --visual-scenario attention".to_owned(),
        );
    }
    if config.visual_checkpoint.is_some()
        && (config.visual_transition_exercise.is_some() || config.idle_measure.is_some())
    {
        return Err(
            "visual checkpoints must run in isolation from timed visual measurements".to_owned(),
        );
    }
    if config.visual_theme.is_some() && config.visual_scenario.is_none() && !config.token_sampler {
        return Err(
            "--visual-theme requires --visual-scenario, --token-sampler, or a visual measurement"
                .to_owned(),
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
    if let Some(duration) = config.visual_transition_exercise {
        include(duration + Duration::from_secs(5));
    }
    if let Some(duration) = config.idle_measure {
        include(
            config.idle_warmup.unwrap_or(DEFAULT_IDLE_WARMUP) + duration + Duration::from_secs(5),
        );
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
    shaping_cache_entries: usize,
    shaping_cache_entries_high_water: usize,
    shaping_cache_accounted_bytes: usize,
    shaping_cache_accounted_bytes_high_water: usize,
    shaping_cache_hits: u64,
    shaping_cache_misses: u64,
    shaping_cache_evictions: u64,
    shaping_cache_rejections: u64,
    shaping_cache_invalidations: u64,
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
    shaping_cache_enabled: bool,
    injected_fault: Option<&'static str>,
    fault_after_ms: Option<u64>,
    scale_after_ms: Option<u64>,
    scale_factor: f32,
    font_family: String,
    font_size: f32,
    harness_project_path: Option<String>,
    window_visibility_policy: &'static str,
    visual_transition_exercise_ms: Option<u64>,
    idle_warmup_ms: Option<u64>,
    idle_measure_ms: Option<u64>,
    visual_checkpoint: Option<&'static str>,
    elapsed_ms: u64,
}

#[derive(Serialize)]
struct RunEvidence {
    schema_version: u8,
    outcome: OutcomeEvidence,
    platform: PlatformEvidence,
    gpu: Option<GpuEvidence>,
    display_refresh_hz: Option<f64>,
    render_geometry: Option<RenderGeometryEvidence>,
    first_usable_frame_ms: Option<f64>,
    first_usable_frame_within_1s: Option<bool>,
    workload: WorkloadEvidence,
    input_to_present_ms: MetricSummary,
    frame_ms: MetricSummary,
    render_stages: RenderStageEvidence,
    redraw_count: u64,
    present_count: u64,
    visual_transition: Option<VisualTransitionEvidence>,
    idle_window: Option<IdleWindowEvidence>,
    resource_samples: Vec<ResourceSample>,
    stress: Option<StressSummary>,
    fault_injection: Option<FaultEvidence>,
    memory: MemorySummary,
    lifecycle: LifecycleEvidence,
    notes: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct VisualTransitionEvidence {
    duration_ms: u64,
    redraw_count: u64,
    present_count: u64,
    present_interval_ms: MetricSummary,
    refresh_relative: RefreshIntervalSummary,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct IdleWindowEvidence {
    duration_ms: u64,
    process_cpu_ms: Option<u64>,
    one_core_cpu_percent: Option<f64>,
    redraw_count: u64,
    present_count: u64,
}

#[derive(Clone, Debug, Serialize)]
struct ResourceSample {
    elapsed_ms: u64,
    checkpoint: &'static str,
    stress_progress_percent: Option<u8>,
    quad_capacity_floats: usize,
    raster_capacity_floats: usize,
    text_row_capacity: usize,
    raster_cache_entries: usize,
    raster_cache_bytes: usize,
    shaping_cache_entries: usize,
    shaping_cache_accounted_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct RenderStageEvidence {
    shaping_ms: MetricSummary,
    frame_prepare_ms: MetricSummary,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct RenderGeometryEvidence {
    backing_scale: f32,
    surface_width_px: u32,
    surface_height_px: u32,
    scene_columns: u16,
    scene_rows: u16,
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
        shaping_cache_enabled: config.shaping_cache_enabled,
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
        visual_transition_exercise_ms: config
            .visual_transition_exercise
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64),
        idle_warmup_ms: config
            .idle_warmup
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64),
        idle_measure_ms: config
            .idle_measure
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64),
        visual_checkpoint: config.visual_checkpoint.map(VisualCheckpoint::as_str),
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
        shaping_cache_enabled: true,
        text_settings: fallback,
        harness_project_path: None,
        visual_scenario: None,
        visual_theme: None,
        token_sampler: false,
        display_name: None,
        visual_transition_exercise: None,
        idle_warmup: None,
        idle_measure: None,
        visual_checkpoint: None,
    };
    let config = config.unwrap_or(&fallback_config);
    let evidence = RunEvidence {
        schema_version: 3,
        outcome: OutcomeEvidence::failure(phase, kind, message),
        platform: PlatformEvidence {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        },
        gpu: None,
        display_refresh_hz: None,
        render_geometry: None,
        first_usable_frame_ms: None,
        first_usable_frame_within_1s: None,
        workload: workload_evidence(config, Duration::ZERO),
        input_to_present_ms: MetricSummary::default(),
        frame_ms: MetricSummary::default(),
        render_stages: RenderStageEvidence::default(),
        redraw_count: 0,
        present_count: 0,
        visual_transition: None,
        idle_window: None,
        resource_samples: Vec::new(),
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

fn process_cpu_time() -> Option<Duration> {
    let output = Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then_some(())
        .and_then(|()| std::str::from_utf8(&output.stdout).ok())
        .and_then(parse_process_cpu_time)
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
    shaping_ms: Samples,
    frame_prepare_ms: Samples,
    memory: MemorySamples,
    next_memory_sample: Instant,
    memory_trend_at: Option<Instant>,
    resource_samples: Vec<ResourceSample>,
    next_resource_sample: Instant,
    stress_resource_checkpoints: [bool; 3],
    redraw_count: u64,
    present_count: u64,
    transition_started_at: Option<Instant>,
    transition_ends_at: Option<Instant>,
    next_transition_action: Option<Instant>,
    transition_action_index: u64,
    transition_counts_start: (u64, u64),
    transition_intervals: Samples,
    transition_last_present: Option<Instant>,
    transition_frames_over_two_periods: u64,
    visual_transition_evidence: Option<VisualTransitionEvidence>,
    idle_measure_at: Option<Instant>,
    idle_started_at: Option<Instant>,
    idle_ends_at: Option<Instant>,
    idle_cpu_start: Option<Duration>,
    idle_counts_start: (u64, u64),
    idle_window_evidence: Option<IdleWindowEvidence>,
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
    mouse_logical: Option<LogicalPoint>,
    mouse_cell: (u16, u16),
    pressed_pointer_buttons: PressedPointerButtons,
    wheel_cell_remainder: (f64, f64),
    scene_presentable: bool,
    scale_probe_at: Option<Instant>,
    scale_probe_applied: bool,
    window_focused: bool,
    ime_allowed: bool,
    prepared_visual_scenario: Option<PreparedVisualScenario>,
    visual_scenario_at: Option<Instant>,
    visual_checkpoint_snapshot: Option<FrameSnapshot>,
    visual_checkpoint_origin: Option<Instant>,
    visual_checkpoint_pending_at: Option<Instant>,
    visual_checkpoint_frozen_at: Option<Instant>,
    visual_checkpoint_retry_at: Option<Instant>,
    visual_checkpoint_retries: u8,
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
            shaping_ms: Samples::with_limit(MAX_MEASUREMENT_SAMPLES),
            frame_prepare_ms: Samples::with_limit(MAX_MEASUREMENT_SAMPLES),
            memory: MemorySamples::default(),
            next_memory_sample: now,
            memory_trend_at: None,
            resource_samples: Vec::new(),
            next_resource_sample: now,
            stress_resource_checkpoints: [false; 3],
            redraw_count: 0,
            present_count: 0,
            transition_started_at: None,
            transition_ends_at: None,
            next_transition_action: None,
            transition_action_index: 0,
            transition_counts_start: (0, 0),
            transition_intervals: Samples::with_limit(MAX_MEASUREMENT_SAMPLES),
            transition_last_present: None,
            transition_frames_over_two_periods: 0,
            visual_transition_evidence: None,
            idle_measure_at: None,
            idle_started_at: None,
            idle_ends_at: None,
            idle_cpu_start: None,
            idle_counts_start: (0, 0),
            idle_window_evidence: None,
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
            mouse_logical: None,
            mouse_cell: (0, 0),
            pressed_pointer_buttons: PressedPointerButtons::default(),
            wheel_cell_remainder: (0.0, 0.0),
            scene_presentable: false,
            scale_probe_at,
            scale_probe_applied: false,
            window_focused: false,
            ime_allowed: false,
            prepared_visual_scenario: None,
            visual_scenario_at: None,
            visual_checkpoint_snapshot: None,
            visual_checkpoint_origin: None,
            visual_checkpoint_pending_at: None,
            visual_checkpoint_frozen_at: None,
            visual_checkpoint_retry_at: None,
            visual_checkpoint_retries: 0,
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn renderer_animation_deadline(&self) -> Option<Instant> {
        if self.config.visual_checkpoint.is_some() {
            return None;
        }
        self.gpu.as_ref().and_then(GpuText::next_animation_deadline)
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
        self.lifecycle.shaping_cache_entries = snapshot.shaping_cache_entries;
        self.lifecycle.shaping_cache_entries_high_water = snapshot.shaping_cache_entries_high_water;
        self.lifecycle.shaping_cache_accounted_bytes = snapshot.shaping_cache_accounted_bytes;
        self.lifecycle.shaping_cache_accounted_bytes_high_water =
            snapshot.shaping_cache_accounted_bytes_high_water;
        self.lifecycle.shaping_cache_hits = snapshot.shaping_cache_hits;
        self.lifecycle.shaping_cache_misses = snapshot.shaping_cache_misses;
        self.lifecycle.shaping_cache_evictions = snapshot.shaping_cache_evictions;
        self.lifecycle.shaping_cache_rejections = snapshot.shaping_cache_rejections;
        self.lifecycle.shaping_cache_invalidations = snapshot.shaping_cache_invalidations;
    }

    fn capture_resource_sample(
        &mut self,
        at: Instant,
        checkpoint: &'static str,
        stress_progress_percent: Option<u8>,
    ) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        let snapshot = gpu.lifecycle_snapshot();
        self.resource_samples.push(ResourceSample {
            elapsed_ms: at
                .saturating_duration_since(self.start)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            checkpoint,
            stress_progress_percent,
            quad_capacity_floats: snapshot.quad_capacity_floats,
            raster_capacity_floats: snapshot.raster_capacity_floats,
            text_row_capacity: snapshot.text_row_capacity,
            raster_cache_entries: snapshot.raster_cache_entries,
            raster_cache_bytes: snapshot.raster_cache_bytes,
            shaping_cache_entries: snapshot.shaping_cache_entries,
            shaping_cache_accounted_bytes: snapshot.shaping_cache_accounted_bytes,
        });
    }

    fn capture_due_stress_resource_checkpoints(&mut self, at: Instant) {
        let Some(stress) = &self.stress else {
            return;
        };
        let summary = stress.summary(at);
        let checkpoints = [
            (80_u8, "stress_80_percent"),
            (90, "stress_90_percent"),
            (100, "stress_100_percent"),
        ];
        let mut due = Vec::new();
        for (index, (percent, label)) in checkpoints.into_iter().enumerate() {
            let target = stress_checkpoint_action(summary.expected_actions, percent);
            if !self.stress_resource_checkpoints[index] && target > 0 && summary.presented >= target
            {
                self.stress_resource_checkpoints[index] = true;
                due.push((label, percent));
            }
        }
        for (label, percent) in due {
            self.capture_resource_sample(at, label, Some(percent));
        }
    }

    fn begin_visual_measurement(&mut self, now: Instant) {
        if self.config.visual_checkpoint.is_some() {
            self.visual_checkpoint_origin = Some(now);
            self.visual_checkpoint_pending_at = None;
            self.visual_checkpoint_frozen_at = None;
            self.visual_checkpoint_retry_at = None;
            self.visual_checkpoint_retries = 0;
            return;
        }
        if let Some(duration) = self.config.visual_transition_exercise {
            self.transition_started_at = Some(now);
            self.transition_ends_at = Some(now + duration);
            self.next_transition_action = Some(now);
            self.transition_counts_start = (self.redraw_count, self.present_count);
            self.transition_last_present = None;
            return;
        }
        if self.config.idle_measure.is_some() {
            self.idle_measure_at =
                Some(now + self.config.idle_warmup.unwrap_or(DEFAULT_IDLE_WARMUP));
        }
    }

    fn service_visual_measurements(&mut self, now: Instant) {
        if self
            .idle_measure_at
            .is_some_and(|measure_at| now >= measure_at)
        {
            self.idle_measure_at = None;
            self.idle_started_at = Some(now);
            self.idle_ends_at = self.config.idle_measure.map(|duration| now + duration);
            self.idle_cpu_start = process_cpu_time();
            self.idle_counts_start = (self.redraw_count, self.present_count);
        }
        if self
            .next_transition_action
            .is_some_and(|next_action| now >= next_action)
            && self.transition_ends_at.is_some_and(|end| now < end)
        {
            let key = match self.transition_action_index % 4 {
                0 => InputKey::ctrl('p'),
                1 => InputKey::plain(KeyCode::Down),
                2 => InputKey::plain(KeyCode::Up),
                _ => InputKey::plain(KeyCode::Escape),
            };
            self.transition_action_index = self.transition_action_index.saturating_add(1);
            self.next_transition_action = Some(now + TRANSITION_ACTION_INTERVAL);
            self.transition_last_present = None;
            self.send_input(InputEvent::Key(key), false, now);
        }
        if self
            .transition_ends_at
            .is_some_and(|transition_ends_at| now >= transition_ends_at)
        {
            self.finish_visual_transition(now);
            self.deadline = Some(now);
        }
        if self
            .idle_ends_at
            .is_some_and(|idle_ends_at| now >= idle_ends_at)
        {
            self.finish_idle_window(now);
            self.deadline = Some(now);
        }
    }

    fn finish_visual_transition(&mut self, now: Instant) {
        let Some(started_at) = self.transition_started_at.take() else {
            return;
        };
        self.transition_ends_at = None;
        self.next_transition_action = None;
        let intervals = self.transition_intervals.summary();
        self.visual_transition_evidence = Some(VisualTransitionEvidence {
            duration_ms: now
                .saturating_duration_since(started_at)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            redraw_count: self
                .redraw_count
                .saturating_sub(self.transition_counts_start.0),
            present_count: self
                .present_count
                .saturating_sub(self.transition_counts_start.1),
            present_interval_ms: intervals,
            refresh_relative: refresh_interval_summary(
                &intervals,
                self.display_refresh_hz,
                self.transition_frames_over_two_periods,
            ),
        });
    }

    fn finish_idle_window(&mut self, now: Instant) {
        let Some(started_at) = self.idle_started_at.take() else {
            return;
        };
        self.idle_ends_at = None;
        let duration_ms = now
            .saturating_duration_since(started_at)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let process_cpu_ms = duration_delta_ms(self.idle_cpu_start.take(), process_cpu_time());
        self.idle_window_evidence = Some(IdleWindowEvidence {
            duration_ms,
            process_cpu_ms,
            one_core_cpu_percent: one_core_cpu_percent(process_cpu_ms, duration_ms),
            redraw_count: self.redraw_count.saturating_sub(self.idle_counts_start.0),
            present_count: self.present_count.saturating_sub(self.idle_counts_start.1),
        });
    }

    fn render_geometry_evidence(&self) -> Option<RenderGeometryEvidence> {
        let gpu = self.gpu.as_ref()?;
        let (surface_width_px, surface_height_px) = gpu.surface_size();
        let scene = self.scene_size()?;
        Some(RenderGeometryEvidence {
            backing_scale: gpu.scale(),
            surface_width_px,
            surface_height_px,
            scene_columns: scene.width,
            scene_rows: scene.height,
        })
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
                // The lab never declares font facts, so the appearance
                // overlay shows no font rows and this effect cannot fire.
                FrontendEffect::ApplyFont { .. } => {}
            }
        }
    }

    fn drain_runtime(&mut self) -> (bool, bool) {
        let generation_before = self.host().scene_generation();
        let drained = self.host_mut().drain_runtime_bounded(EVENT_DRAIN_BUDGET);
        let scene_changed = self.host().scene_generation() != generation_before;
        if scene_changed {
            self.dirty_from_runtime = true;
        }
        self.apply_effects();
        (drained == EVENT_DRAIN_BUDGET, scene_changed)
    }

    fn render_frame(&mut self) -> Result<(), GpuRenderError> {
        self.scene_presentable = false;
        let Some(viewport) = self.viewport_metrics() else {
            self.cancel_and_disable_ime();
            self.host_mut().suspend_scene_interaction();
            return Ok(());
        };
        let mut snapshot = retained_visual_checkpoint_snapshot(
            self.visual_checkpoint_origin,
            self.visual_checkpoint_snapshot.as_ref(),
        )
        .unwrap_or_else(|| self.host_mut().frame_with_viewport(viewport));
        if let Some(prepared) = &self.prepared_visual_scenario {
            prepared
                .stabilize_snapshot(&mut snapshot)
                .map_err(|error| GpuRenderError::Internal {
                    message: format!("visual scenario stabilization failed: {error}"),
                })?;
        }
        if self.config.token_sampler {
            apply_token_sampler_scene(&mut snapshot.scene, &snapshot.theme);
        }
        if scene_is_suspended_by_tiled_minimum(&snapshot.scene) {
            self.cancel_and_disable_ime();
            self.host_mut().suspend_scene_interaction();
            return Ok(());
        }
        self.sync_ime(&snapshot.scene);
        let checkpoint = self.config.visual_checkpoint;
        let checkpoint_origin = self.visual_checkpoint_origin;
        let pending_at = self.visual_checkpoint_pending_at;
        let frozen_at = self.visual_checkpoint_frozen_at;
        let first_checkpoint_render = checkpoint.is_some()
            && checkpoint_origin.is_some()
            && pending_at.is_none()
            && frozen_at.is_none();
        let visual_now = frozen_at
            .or(pending_at)
            .or(checkpoint_origin)
            .unwrap_or_else(Instant::now);
        let (render_result, freeze_at, animation_was_active) = {
            let Some(gpu) = self.gpu.as_mut() else {
                return Ok(());
            };
            let animation_was_active = gpu.animation_is_active();
            let first = gpu.render_at(&snapshot.scene, &snapshot.theme, visual_now);
            let result = match (
                first,
                checkpoint,
                checkpoint_origin,
                first_checkpoint_render,
            ) {
                (Ok(outcome), Some(checkpoint), Some(origin), true) => {
                    if checkpoint == VisualCheckpoint::Reduced
                        && (gpu.animation_is_active()
                            || gpu.next_animation_deadline().is_some()
                            || gpu.pointer_geometry_is_moving())
                    {
                        (
                            Err(GpuRenderError::Internal {
                                message:
                                    "reduced-motion checkpoint produced active presentation motion"
                                        .to_owned(),
                            }),
                            None,
                        )
                    } else {
                        let approval_window =
                            gpu.active_transition_window(TransitionRole::ApprovalArrival);
                        if checkpoint != VisualCheckpoint::Reduced && approval_window.is_none() {
                            return Err(GpuRenderError::Internal {
                                message: format!(
                                    "{} checkpoint did not start an ApprovalArrival transition",
                                    checkpoint.as_str()
                                ),
                            });
                        }
                        let target = checkpoint_instant(checkpoint, origin, approval_window);
                        match target {
                            Ok(target)
                                if matches!(
                                    checkpoint,
                                    VisualCheckpoint::Midpoint | VisualCheckpoint::End
                                ) =>
                            {
                                (
                                    gpu.render_at(&snapshot.scene, &snapshot.theme, target),
                                    Some(target),
                                )
                            }
                            Ok(target) => (Ok(outcome), Some(target)),
                            Err(error) => (Err(error), None),
                        }
                    }
                }
                (result, _, _, _) => (result, None),
            };
            (result.0, result.1, animation_was_active)
        };
        if let Some(freeze_at) = freeze_at {
            self.visual_checkpoint_pending_at = Some(freeze_at);
        }
        let outcome = match render_result {
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
                let recreate_result = {
                    let Some(gpu) = self.gpu.as_mut() else {
                        return Ok(());
                    };
                    pollster::block_on(gpu.recreate_device())
                };
                match recreate_result {
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
        let freeze_candidate = self.visual_checkpoint_pending_at;
        let was_checkpoint_frozen = self.visual_checkpoint_frozen_at.is_some();
        self.visual_checkpoint_frozen_at = checkpoint_freeze_after_outcome(
            self.visual_checkpoint_frozen_at,
            freeze_candidate,
            matches!(&outcome, GpuRenderOutcome::Presented { .. }),
        );
        let checkpoint_became_ready =
            !was_checkpoint_frozen && self.visual_checkpoint_frozen_at.is_some();
        match outcome {
            GpuRenderOutcome::Presented {
                at: present,
                timings,
            } => {
                if checkpoint.is_some() {
                    self.visual_checkpoint_pending_at = None;
                    self.visual_checkpoint_retry_at = None;
                    self.visual_checkpoint_retries = 0;
                }
                if checkpoint_became_ready
                    && let (Some(window), Some(checkpoint)) = (&self.window, checkpoint)
                {
                    window.set_title(&format!("Mandatum Visual {}", checkpoint.reference_id()));
                }
                self.present_count = self.present_count.saturating_add(1);
                if self
                    .transition_started_at
                    .is_some_and(|start| present >= start)
                    && self.transition_ends_at.is_some_and(|end| present <= end)
                {
                    let animation_is_active =
                        self.gpu.as_ref().is_some_and(GpuText::animation_is_active);
                    let (interval, next_present) = contiguous_animation_interval(
                        self.transition_last_present,
                        present,
                        animation_was_active,
                        animation_is_active,
                    );
                    if let Some(interval) = interval {
                        let interval_ms = interval.as_secs_f64() * 1_000.0;
                        self.transition_intervals.push(interval_ms);
                        if self
                            .display_refresh_hz
                            .filter(|refresh_hz| refresh_hz.is_finite() && *refresh_hz > 0.0)
                            .is_some_and(|refresh_hz| interval_ms > 2_000.0 / refresh_hz)
                        {
                            self.transition_frames_over_two_periods =
                                self.transition_frames_over_two_periods.saturating_add(1);
                        }
                    }
                    self.transition_last_present = next_present;
                }
                self.shaping_ms
                    .push(timings.shaping.as_secs_f64() * 1_000.0);
                self.frame_prepare_ms
                    .push(timings.frame_prepare.as_secs_f64() * 1_000.0);
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
                self.capture_due_stress_resource_checkpoints(present);
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
            GpuRenderOutcome::Skipped { reason, timings } => {
                self.shaping_ms
                    .push(timings.shaping.as_secs_f64() * 1_000.0);
                self.frame_prepare_ms
                    .push(timings.frame_prepare.as_secs_f64() * 1_000.0);
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
                if checkpoint.is_some() {
                    self.schedule_visual_checkpoint_retry()?;
                }
            }
            GpuRenderOutcome::SurfaceReconfigured { recovery, timings } => {
                self.shaping_ms
                    .push(timings.shaping.as_secs_f64() * 1_000.0);
                self.frame_prepare_ms
                    .push(timings.frame_prepare.as_secs_f64() * 1_000.0);
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
                if checkpoint.is_some() {
                    self.schedule_visual_checkpoint_retry()?;
                }
            }
        }
        Ok(())
    }

    fn schedule_visual_checkpoint_retry(&mut self) -> Result<(), GpuRenderError> {
        self.visual_checkpoint_retries = self.visual_checkpoint_retries.saturating_add(1);
        if self.visual_checkpoint_retries > MAX_VISUAL_CHECKPOINT_RETRIES {
            return Err(GpuRenderError::Internal {
                message: "visual checkpoint surface remained unavailable after bounded retries"
                    .to_owned(),
            });
        }
        self.visual_checkpoint_retry_at = Some(Instant::now() + VISUAL_CHECKPOINT_RETRY_DELAY);
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
        let now = Instant::now();
        self.memory.push(process_rss_bytes());
        self.capture_resource_sample(now, "final", None);
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
        if self.fatal_error.is_none()
            && self.config.visual_transition_exercise.is_some()
            && self.visual_transition_evidence.is_none()
        {
            self.fail(
                "runtime",
                "visual_transition_incomplete",
                "visual transition exercise ended before its delimited evidence window completed",
            );
        }
        if self.fatal_error.is_none()
            && self.config.idle_measure.is_some()
            && self.idle_window_evidence.is_none()
        {
            self.fail(
                "runtime",
                "idle_window_incomplete",
                "idle measurement ended before its delimited evidence window completed",
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
            schema_version: 3,
            outcome,
            platform: PlatformEvidence {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            },
            gpu: self.gpu_evidence.clone(),
            display_refresh_hz: self.display_refresh_hz,
            render_geometry: self.render_geometry_evidence(),
            first_usable_frame_ms: self.first_usable_frame_ms,
            first_usable_frame_within_1s: self.first_usable_frame_ms.map(|ms| ms <= 1_000.0),
            workload: workload_evidence(&self.config, now.saturating_duration_since(self.start)),
            input_to_present_ms: input_to_present,
            frame_ms: self.frame_ms.summary(),
            render_stages: RenderStageEvidence {
                shaping_ms: self.shaping_ms.summary(),
                frame_prepare_ms: self.frame_prepare_ms.summary(),
            },
            redraw_count: self.redraw_count,
            present_count: self.present_count,
            visual_transition: self.visual_transition_evidence,
            idle_window: self.idle_window_evidence,
            resource_samples: std::mem::take(&mut self.resource_samples),
            stress,
            fault_injection: self.config.fault.map(|fault| FaultEvidence {
                requested: fault.label(),
                injected: self.fault_injected,
                post_recovery_present: self.post_recovery_present,
            }),
            memory,
            lifecycle: self.lifecycle,
            notes: "FrontendHost is preserved across renderer recovery; input-to-present is emitted only by the isolated typing benchmark; soak input is action-counted without claiming causal latency; frame timing excludes idle gaps >=250ms; shaping_ms covers cache lookup plus miss shaping and admission; frame_prepare_ms starts before scene preparation and ends after glyphon prepare, before surface acquisition, submit, and present",
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
            self.mouse_logical = None;
            return;
        };
        let scale = f64::from(gpu.scale());
        self.mouse_logical = (scale.is_finite() && scale > 0.0)
            .then(|| LogicalPoint::from_pixels(x / scale, y / scale).ok())
            .flatten();
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
        let redraw_before = self.host().pointer_move_redraw_state();
        let (column, row) = self.mouse_cell;
        let pointer = PointerEvent {
            kind,
            button,
            column,
            row,
            mods: neutral_modifiers(self.modifiers),
        };
        if let Some(logical_position) = self.mouse_logical {
            self.host_mut()
                .handle_pointer_at_logical(pointer, logical_position);
        } else {
            self.host_mut().handle_input(InputEvent::Pointer(pointer));
        }
        let redraw_after = self.host().pointer_move_redraw_state();
        self.apply_effects();
        if kind != PointerKind::Move
            || redraw_before.0
            || redraw_after.0
            || redraw_before.1 != redraw_after.1
            || redraw_before.2 != redraw_after.2
        {
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
                self.wheel_cell_remainder = (0.0, 0.0);
                ((-x).round() as i16, (-y).round() as i16, false)
            }
            MouseScrollDelta::PixelDelta(position) => {
                self.wheel_cell_remainder.0 += -position.x / f64::from(gpu.cell_w());
                self.wheel_cell_remainder.1 += -position.y / f64::from(gpu.cell_h());
                let dx = self.wheel_cell_remainder.0.trunc();
                let dy = self.wheel_cell_remainder.1.trunc();
                self.wheel_cell_remainder.0 -= dx;
                self.wheel_cell_remainder.1 -= dy;
                (dx as i16, dy as i16, true)
            }
        };
        if dx != 0 {
            self.pointer_input(PointerKind::Wheel { dx, dy: 0, precise }, None);
        }
        if dy != 0 {
            self.pointer_input(PointerKind::Wheel { dx: 0, dy, precise }, None);
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
            .visual_checkpoint_retry_at
            .is_some_and(|retry_at| now >= retry_at)
        {
            self.visual_checkpoint_retry_at = None;
            self.request_redraw();
        }
        if self
            .visual_scenario_at
            .is_some_and(|visual_scenario_at| now >= visual_scenario_at)
        {
            self.visual_scenario_at = None;
            let Some(prepared) = self.prepared_visual_scenario.take() else {
                self.fail(
                    "runtime",
                    "visual_scenario",
                    "scheduled visual scenario fixture was unavailable",
                );
                return false;
            };
            let Some(viewport) = self.viewport_metrics() else {
                self.fail(
                    "runtime",
                    "visual_scenario",
                    "fixed visual surface did not produce usable scene geometry",
                );
                return false;
            };
            let scene_size = viewport.scene_size();
            let drive_result = if self.config.visual_checkpoint.is_some() {
                prepared.drive_attention_arrival(self.host_mut(), viewport, Duration::from_secs(5))
            } else {
                prepared.drive(self.host_mut(), scene_size, Duration::from_secs(5))
            };
            self.prepared_visual_scenario = Some(prepared);
            let driven_snapshot = match drive_result {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.fail("runtime", "visual_scenario", error.to_string());
                    return false;
                }
            };
            if self.config.visual_checkpoint.is_some() {
                self.visual_checkpoint_snapshot = Some(driven_snapshot);
            }
            // Scenario driving is synchronous and may consume most of its
            // timeout. Measurement windows begin only after the stable fixture
            // has actually been produced.
            self.begin_visual_measurement(Instant::now());
            self.request_redraw();
        }
        if self
            .scale_probe_at
            .is_some_and(|scale_probe_at| now >= scale_probe_at)
        {
            self.scale_probe_at = None;
            self.scale_probe_applied = true;
            self.apply_scale_factor(self.config.scale_factor);
        }
        self.service_visual_measurements(now);
        // Exercise actions may establish renderer motion, but the lab follows
        // the production scheduler: no renderer deadline means no extra frame.
        if animation_redraw_is_due(now, self.renderer_animation_deadline()) {
            self.request_redraw();
        }
        if now >= self.next_resource_sample {
            self.capture_resource_sample(now, "interval", None);
            self.next_resource_sample = now + RESOURCE_SAMPLE_INTERVAL;
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.cancel_and_disable_ime();
            self.shutdown_host();
            self.print_summary();
            event_loop.exit();
            return true;
        }
        if now >= self.next_heartbeat {
            let changed = self.host_mut().heartbeat();
            self.next_heartbeat = now + HEARTBEAT;
            if changed {
                self.request_redraw();
            }
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
        next = next.min(self.next_resource_sample);
        if let Some(scale_probe_at) = self.scale_probe_at {
            next = next.min(scale_probe_at);
        }
        if let Some(fault_at) = self.fault_at {
            next = next.min(fault_at);
        }
        if let Some(visual_scenario_at) = self.visual_scenario_at {
            next = next.min(visual_scenario_at);
        }
        if let Some(visual_checkpoint_retry_at) = self.visual_checkpoint_retry_at {
            next = next.min(visual_checkpoint_retry_at);
        }
        if let Some(idle_measure_at) = self.idle_measure_at {
            next = next.min(idle_measure_at);
        }
        if let Some(idle_ends_at) = self.idle_ends_at {
            next = next.min(idle_ends_at);
        }
        if let Some(next_transition_action) = self.next_transition_action {
            next = next.min(next_transition_action);
        }
        if let Some(animation_deadline) = self.renderer_animation_deadline() {
            next = next.min(animation_deadline);
        }
        if let Some(transition_ends_at) = self.transition_ends_at {
            next = next.min(transition_ends_at);
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
                let title = if self.config.token_sampler {
                    "Mandatum Phase 2 Token Sampler".to_owned()
                } else if let Some(checkpoint) = self.config.visual_checkpoint {
                    format!("Mandatum Visual {} loading", checkpoint.reference_id())
                } else {
                    self.config.visual_scenario.map_or_else(
                        || "Mandatum GPU Host Spike".to_owned(),
                        |scenario| format!("Mandatum Visual {}", scenario.as_str()),
                    )
                };
                let mut attributes = Window::default_attributes().with_title(title);
                if self.config.visual_scenario.is_some() || self.config.token_sampler {
                    attributes = attributes
                        // The fixed catalog contract is 800x600 logical. Use a
                        // logical request so creating the window while a 1x
                        // external display is primary cannot double the client
                        // surface after placement on the 2x reference panel.
                        .with_inner_size(WindowLogicalSize::new(800_u32, 600_u32))
                        .with_decorations(false);
                }
                if let Some(display_name) = self.config.display_name.as_deref() {
                    let monitors = event_loop.available_monitors().collect::<Vec<_>>();
                    let monitor = monitors
                        .iter()
                        .find(|monitor| {
                            monitor
                                .name()
                                .as_deref()
                                .is_some_and(|candidate| {
                                    display_names_match(display_name, candidate)
                                })
                                || (display_name == "Built-in Retina Display"
                                    && (monitor.scale_factor() - 2.0).abs() < f64::EPSILON)
                        })
                        .ok_or_else(|| {
                            let active = monitors
                                .iter()
                                .map(|monitor| {
                                    format!(
                                        "{} scale={} position={:?} size={:?}",
                                        monitor.name().unwrap_or_else(|| "<unnamed>".to_owned()),
                                        monitor.scale_factor(),
                                        monitor.position(),
                                        monitor.size()
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            GpuStartupError::no_display(format!(
                                "requested display {display_name:?} is not active; active displays: {active}"
                            ))
                        })?;
                    let position = monitor.position();
                    let scale = monitor.scale_factor();
                    // macOS reports a Retina monitor's global origin in the
                    // main display's physical coordinate space. Window
                    // placement expects the corresponding AppKit points.
                    #[cfg(target_os = "macos")]
                    let position = PhysicalPosition::new(
                        (f64::from(position.x) / scale).round() as i32,
                        (f64::from(position.y) / scale).round() as i32,
                    );
                    attributes = attributes.with_position(position);
                }
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
        let (window, mut gpu, mut host) = match startup {
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
        gpu.set_shaping_cache_enabled(self.config.shaping_cache_enabled);
        let prepared_visual_scenario = if let Some(scenario) = self.config.visual_scenario {
            let fixture_root = self
                .config
                .harness_project_path
                .as_deref()
                .map(std::path::Path::new)
                .expect("visual scenarios always use an isolated harness path");
            match prepare_visual_scenario(scenario, fixture_root) {
                Ok(prepared) => Some(prepared),
                Err(error) => {
                    self.fail("startup", "visual_scenario", error.to_string());
                    host.shutdown();
                    self.print_summary();
                    event_loop.exit();
                    return;
                }
            }
        } else {
            None
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
        self.prepared_visual_scenario = prepared_visual_scenario;
        self.visual_scenario_at = self
            .prepared_visual_scenario
            .as_ref()
            .map(|_| ready + VISUAL_SCENARIO_SETTLE_DELAY);
        self.next_heartbeat = ready + HEARTBEAT;
        self.next_memory_sample = ready;
        self.next_resource_sample = ready;
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
        self.capture_resource_sample(ready, "startup", None);
        self.next_resource_sample = ready + RESOURCE_SAMPLE_INTERVAL;
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
                if self.service_scheduled_work(event_loop) {
                    return;
                }
                let (more_pending, scene_changed) = self.drain_runtime();
                if self.exit_if_requested(event_loop) {
                    return;
                }
                if more_pending {
                    let _ = self.wake_proxy.send_event(UserEvent::Wake);
                }
                if scene_changed && self.scene_presentable {
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
                self.redraw_count = self.redraw_count.saturating_add(1);
                let (more_pending, _) = self.drain_runtime();
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
                if more_pending {
                    let _ = self.wake_proxy.send_event(UserEvent::Wake);
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
                if self.prepared_visual_scenario.is_some() {
                    self.visual_scenario_at = Some(Instant::now() + VISUAL_SCENARIO_SETTLE_DELAY);
                }
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

#[cfg(test)]
fn scene_size_from_metrics(
    width: u32,
    height: u32,
    cell_width: f32,
    cell_height: f32,
) -> Option<SceneSize> {
    viewport_metrics_from_renderer(width, height, 1.0, cell_width, cell_height)
        .map(ViewportMetrics::scene_size)
}

fn viewport_metrics_from_renderer(
    width: u32,
    height: u32,
    scale: f32,
    physical_cell_width: f32,
    physical_cell_height: f32,
) -> Option<ViewportMetrics> {
    if width == 0
        || height == 0
        || !scale.is_finite()
        || scale <= 0.0
        || !physical_cell_width.is_finite()
        || !physical_cell_height.is_finite()
        || physical_cell_width <= 0.0
        || physical_cell_height <= 0.0
    {
        return None;
    }
    let scale = f64::from(scale);
    let viewport = ViewportMetrics::new(
        LogicalSize::from_pixels(f64::from(width) / scale, f64::from(height) / scale).ok()?,
        ScenePhysicalSize::new(width, height),
        BackingScale::new(scale).ok()?,
        LogicalSize::from_pixels(
            f64::from(physical_cell_width) / scale,
            f64::from(physical_cell_height) / scale,
        )
        .ok()?,
    )
    .ok()?;
    let size = viewport.scene_size();
    // One pane needs a 3x3 bordered interior between the one-row header and
    // status strips. Suspend scene production while a minimized/tiny window
    // cannot satisfy that structural contract.
    (size.width >= 3 && size.height >= 5).then_some(viewport)
}

fn apply_token_sampler_scene(scene: &mut WorkspaceScene, theme: &Theme) {
    let Some(viewport) = scene.presentation.viewport else {
        return;
    };
    let bounds = mandatum_scene::LogicalRect::from_units(
        0,
        0,
        viewport.logical_size.width_units(),
        viewport.logical_size.height_units(),
    );
    let Ok(sampler) = prepare_token_sampler(theme, bounds) else {
        return;
    };
    let Some(pane) = scene.panes.first_mut() else {
        return;
    };
    let inner = mandatum_scene::layout::pane_inner_rect(pane.area);
    let canvas = theme.ui.palette.canvas;
    let canvas_style = SceneCellStyle {
        background: scene_color(canvas),
        ..SceneCellStyle::default()
    };
    let mut rows = vec![
        vec![SceneCell::grapheme(" ", canvas_style); usize::from(inner.width)];
        usize::from(inner.height)
    ];

    for (row, swatch) in sampler
        .swatches()
        .iter()
        .take(usize::from(inner.height))
        .enumerate()
    {
        let display = composite_over(swatch.color, canvas);
        let foreground = if relative_luminance(display) > 0.42 {
            UiColor::rgb(0x0b, 0x0d, 0x10)
        } else {
            UiColor::rgb(0xe7, 0xea, 0xf0)
        };
        let style = SceneCellStyle {
            foreground: scene_color(foreground),
            background: scene_color(display),
            bold: true,
            ..SceneCellStyle::default()
        };
        let label = format!(
            "{:?}  #{:02X}{:02X}{:02X}{:02X}",
            swatch.role,
            swatch.color.red,
            swatch.color.green,
            swatch.color.blue,
            swatch.color.alpha
        );
        for (column, character) in label.chars().enumerate() {
            let Some(cell) = rows[row].get_mut(column) else {
                break;
            };
            *cell = SceneCell::grapheme(character.to_string(), style);
        }
        for cell in rows[row].iter_mut().skip(label.chars().count()) {
            *cell = SceneCell::grapheme(" ", style);
        }
    }

    pane.title = "Phase 2 semantic UI token sampler".to_owned();
    pane.content = PaneContent::Terminal(mandatum_scene::TerminalSurface {
        rows,
        ..mandatum_scene::TerminalSurface::default()
    });
    scene.header.text = "Mandatum · Phase 2 token and native presentation foundation".to_owned();
    scene.status.text = "Direct UI RGBA roles · terminal palette remains independent".to_owned();
}

fn scene_color(color: UiColor) -> SceneColor {
    SceneColor::Rgb(color.red, color.green, color.blue)
}

fn composite_over(foreground: UiColor, background: UiColor) -> UiColor {
    let alpha = u16::from(foreground.alpha);
    let blend = |front: u8, back: u8| {
        ((u16::from(front) * alpha + u16::from(back) * (255 - alpha) + 127) / 255) as u8
    };
    UiColor::rgb(
        blend(foreground.red, background.red),
        blend(foreground.green, background.green),
        blend(foreground.blue, background.blue),
    )
}

fn relative_luminance(color: UiColor) -> f64 {
    let linear = |component: u8| {
        let value = f64::from(component) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.red) + 0.7152 * linear(color.green) + 0.0722 * linear(color.blue)
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
    config.typing_bench
        || config.flood
        || config.stress.is_some()
        || config.fault.is_some()
        || config.visual_scenario.is_some()
        || config.token_sampler
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
        let mut app_config = if let Some(scenario) = config.visual_scenario {
            prepare_visual_scenario(scenario, &project_path)
                .map_err(std::io::Error::other)?
                .app_config()
        } else {
            AppConfig {
                workspace_name: "Mandatum GPU Harness".to_owned(),
                workspace_file: project_path.join(".mandatum").join("workspace.json"),
                project_path: project_path.clone(),
                shell_program: "/bin/sh".to_owned(),
                spawn_pty: true,
                restore_on_startup: false,
                ..AppConfig::default()
            }
        };
        if config.idle_measure.is_some() {
            // Idle evidence excludes restore work by contract. The fixed
            // calm-terminal shell/output recipe still comes from the catalog,
            // but durable workspace restoration is disabled for this process.
            app_config.restore_on_startup = false;
        }
        if config.visual_checkpoint == Some(VisualCheckpoint::Reduced) {
            app_config.reduced_motion = true;
        }
        if let Some(theme) = config.visual_theme.clone() {
            app_config.theme = theme;
        }
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
        DEFAULT_MEMORY_INTERVAL, DEFAULT_SOAK_DURATION, FaultConfig, IdleWindowEvidence,
        LifecycleEvidence, MemorySummary, MetricSummary, OutcomeEvidence, PlatformAction,
        PlatformEvidence, PressedPointerButtons, RefreshIntervalSummary, RenderStageEvidence,
        ResourceSample, RunEvidence, StressConfig, VisualCheckpoint, VisualScenarioId,
        VisualTransitionEvidence, WorkloadEvidence, animation_redraw_is_due, app_config_for_run,
        checkpoint_freeze_after_outcome, configured_run_timeout, contiguous_animation_interval,
        ime_event_is_accepted, key_for_platform_translation, pane_geometry_is_suspended,
        parse_config_from, parse_font_family, parse_font_size, parse_ps_rss_kib, parse_scale_delay,
        parse_scale_factor, retained_visual_checkpoint_snapshot, run_exit_code,
        scene_size_from_metrics, start_after_preflight, translate_ime, translate_key,
        uses_isolated_harness,
    };
    use mandatum_scene::Theme;
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
    fn visual_checkpoint_retains_snapshot_and_freezes_only_after_presented() {
        let origin = std::time::Instant::now();
        let target = origin + std::time::Duration::from_millis(120);

        assert_eq!(
            retained_visual_checkpoint_snapshot(Some(origin), Some(&41_u64)),
            Some(41),
            "checkpoint retries keep the exact driven snapshot"
        );
        assert_eq!(
            retained_visual_checkpoint_snapshot::<u64>(Some(origin), None),
            None,
            "a missing capture still falls back to one live frame"
        );
        assert_eq!(
            retained_visual_checkpoint_snapshot(None, Some(&41_u64)),
            None,
            "ordinary rendering never reuses checkpoint-only state"
        );

        let after_skip = checkpoint_freeze_after_outcome(None, Some(target), false);
        assert_eq!(
            after_skip, None,
            "timeout, occlusion, or surface recovery must not report a frozen checkpoint"
        );
        assert_eq!(
            retained_visual_checkpoint_snapshot(Some(origin), Some(&41_u64)),
            Some(41),
            "checkpoint outcome classification does not consume the retained snapshot"
        );
        let after_present = checkpoint_freeze_after_outcome(after_skip, Some(target), true);
        assert_eq!(after_present, Some(target));
        assert_eq!(
            checkpoint_freeze_after_outcome(after_present, None, false),
            Some(target),
            "later skipped redraws cannot unfreeze a presented checkpoint"
        );
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
    fn visual_theme_cli_accepts_only_canonical_themes_on_visual_routes() {
        for name in Theme::BUILTIN_NAMES.iter().copied() {
            let config = parse_config_from(
                ["--visual-scenario", "palette", "--visual-theme", name]
                    .into_iter()
                    .map(str::to_owned),
            )
            .expect("canonical visual theme");
            assert_eq!(
                config
                    .visual_theme
                    .as_ref()
                    .map(|theme| theme.name.as_str()),
                Some(name)
            );
        }

        for invalid in [
            vec!["--visual-theme", "mandatum-light"],
            vec!["--visual-theme", "light", "--visual-scenario", "palette"],
            vec!["--visual-theme", "solarized", "--token-sampler"],
        ] {
            assert!(
                parse_config_from(invalid.into_iter().map(str::to_owned)).is_err(),
                "invalid visual theme route was accepted"
            );
        }
    }

    #[test]
    fn selected_visual_theme_reaches_the_isolated_host_config() {
        let mut config = parse_config_from(
            [
                "--token-sampler",
                "--visual-theme",
                "mandatum-high-contrast",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("high-contrast token sampler");
        let app_config = app_config_for_run(&mut config).expect("isolated app config");
        let project_path = app_config.project_path.clone();

        assert_eq!(app_config.theme.name, "mandatum-high-contrast");

        std::fs::remove_dir_all(project_path).expect("remove isolated test harness");
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

        let uncached = parse_config_from(
            ["--typing-samples", "10", "--disable-shaping-cache"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("paired uncached measurement config");
        assert!(!uncached.shaping_cache_enabled);

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

        let visual = parse_config_from(
            [
                "--visual-scenario",
                "dense-workspace",
                "--display",
                "Built-in Retina Display",
                "--exit-after",
                "30",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("canonical visual scenario");
        assert_eq!(
            visual.visual_scenario,
            Some(VisualScenarioId::DenseWorkspace)
        );
        assert_eq!(
            visual.display_name.as_deref(),
            Some("Built-in Retina Display")
        );
        let transition = parse_config_from(
            ["--visual-transition-exercise-seconds", "5"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("isolated transition exercise");
        assert_eq!(
            transition.visual_transition_exercise,
            Some(std::time::Duration::from_secs(5))
        );
        assert_eq!(
            transition.visual_scenario,
            Some(VisualScenarioId::DenseWorkspace)
        );
        assert_eq!(
            configured_run_timeout(&transition),
            Some(std::time::Duration::from_secs(10))
        );
        let idle = parse_config_from(
            ["--idle-measure-seconds", "30"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("isolated calm-terminal idle window");
        assert_eq!(idle.idle_warmup, Some(std::time::Duration::from_secs(5)));
        assert_eq!(idle.idle_measure, Some(std::time::Duration::from_secs(30)));
        assert_eq!(idle.visual_scenario, Some(VisualScenarioId::CalmTerminal));
        assert!(uses_isolated_harness(&idle));
        let sampler = parse_config_from(
            ["--token-sampler", "--exit-after", "5"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("native token sampler route");
        assert!(sampler.token_sampler);
        assert!(uses_isolated_harness(&sampler));

        for (name, checkpoint, reference_id) in [
            ("start", VisualCheckpoint::Start, "attention-motion-start"),
            (
                "midpoint",
                VisualCheckpoint::Midpoint,
                "attention-motion-midpoint",
            ),
            ("end", VisualCheckpoint::End, "attention-motion-end"),
            ("reduced", VisualCheckpoint::Reduced, "attention-reduced"),
        ] {
            let config = parse_config_from(
                [
                    "--visual-scenario",
                    "attention",
                    "--visual-checkpoint",
                    name,
                ]
                .into_iter()
                .map(str::to_owned),
            )
            .expect("attention checkpoint config");
            assert_eq!(config.visual_checkpoint, Some(checkpoint));
            assert_eq!(checkpoint.reference_id(), reference_id);
        }

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
            vec!["--visual-scenario", "dashboard"],
            vec!["--visual-scenario", "palette", "--resize-exercise"],
            vec!["--warmup-seconds", "5"],
            vec![
                "--visual-transition-exercise-seconds",
                "5",
                "--idle-measure-seconds",
                "30",
            ],
            vec![
                "--visual-transition-exercise-seconds",
                "5",
                "--typing-bench",
            ],
            vec!["--idle-measure-seconds", "30", "--exit-after", "10"],
            vec![
                "--idle-measure-seconds",
                "30",
                "--visual-scenario",
                "attention",
            ],
            vec!["--visual-checkpoint", "start"],
            vec![
                "--visual-scenario",
                "calm-terminal",
                "--visual-checkpoint",
                "start",
            ],
            vec![
                "--visual-scenario",
                "attention",
                "--visual-checkpoint",
                "eventually",
            ],
            vec![
                "--visual-scenario",
                "attention",
                "--visual-checkpoint",
                "start",
                "--visual-transition-exercise-seconds",
                "5",
            ],
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
            schema_version: 3,
            outcome: OutcomeEvidence::failure("startup", "no_display", "headless"),
            platform: PlatformEvidence {
                os: "test-os",
                arch: "test-arch",
            },
            gpu: None,
            display_refresh_hz: None,
            render_geometry: None,
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
                shaping_cache_enabled: true,
                injected_fault: None,
                fault_after_ms: None,
                scale_after_ms: None,
                scale_factor: 1.5,
                font_family: "monospace".to_owned(),
                font_size: 15.0,
                harness_project_path: None,
                window_visibility_policy: "normal",
                visual_transition_exercise_ms: None,
                idle_warmup_ms: None,
                idle_measure_ms: None,
                visual_checkpoint: None,
                elapsed_ms: 0,
            },
            input_to_present_ms: MetricSummary::default(),
            frame_ms: MetricSummary::default(),
            render_stages: RenderStageEvidence::default(),
            redraw_count: 0,
            present_count: 0,
            visual_transition: None,
            idle_window: None,
            resource_samples: Vec::new(),
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
        assert!(json["render_stages"]["shaping_ms"]["p50"].is_null());
        assert_eq!(json["schema_version"], 3);
        assert_eq!(json["outcome"]["kind"], "no_display");
        assert_eq!(json["redraw_count"], 0);
        assert_eq!(json["present_count"], 0);
        assert!(json["visual_transition"].is_null());
        assert!(json["idle_window"].is_null());
        assert!(json["workload"]["visual_checkpoint"].is_null());
        assert_eq!(json["resource_samples"], serde_json::json!([]));
    }

    #[test]
    fn transition_exercise_only_redraws_for_a_due_renderer_deadline() {
        let now = std::time::Instant::now();
        assert!(!animation_redraw_is_due(now, None));
        assert!(!animation_redraw_is_due(
            now,
            Some(now + std::time::Duration::from_millis(1))
        ));
        assert!(animation_redraw_is_due(now, Some(now)));
    }

    #[test]
    fn transition_intervals_exclude_inactive_gaps_between_actions() {
        let first = std::time::Instant::now();
        let active = first + std::time::Duration::from_millis(8);
        let (interval, previous) = contiguous_animation_interval(Some(first), active, true, true);
        assert_eq!(interval, Some(std::time::Duration::from_millis(8)));
        assert_eq!(previous, Some(active));

        let finish = active + std::time::Duration::from_millis(8);
        let (interval, previous) = contiguous_animation_interval(previous, finish, true, false);
        assert_eq!(interval, Some(std::time::Duration::from_millis(8)));
        assert_eq!(previous, None);

        let next_action = finish + std::time::Duration::from_millis(60);
        let (interval, previous) =
            contiguous_animation_interval(previous, next_action, false, false);
        assert_eq!(interval, None);
        assert_eq!(previous, None);
    }

    #[test]
    fn phase_six_measurement_schema_names_every_delimited_field() {
        let intervals = MetricSummary {
            sample_count: 10,
            misses: 0,
            p50: Some(8.0),
            p95: Some(9.5),
            max: Some(12.0),
        };
        let transition = serde_json::to_value(VisualTransitionEvidence {
            duration_ms: 5_000,
            redraw_count: 600,
            present_count: 598,
            present_interval_ms: intervals,
            refresh_relative: RefreshIntervalSummary {
                display_period_ms: Some(1_000.0 / 120.0),
                p95_display_periods: Some(1.14),
                frames_over_two_periods: 2,
                fraction_over_two_periods: Some(0.00334),
            },
        })
        .expect("transition evidence serializes");
        assert_eq!(transition["duration_ms"], 5_000);
        assert_eq!(transition["redraw_count"], 600);
        assert_eq!(transition["present_count"], 598);
        assert_eq!(transition["present_interval_ms"]["sample_count"], 10);
        assert_eq!(transition["refresh_relative"]["frames_over_two_periods"], 2);

        let idle = serde_json::to_value(IdleWindowEvidence {
            duration_ms: 30_000,
            process_cpu_ms: Some(120),
            one_core_cpu_percent: Some(0.4),
            redraw_count: 0,
            present_count: 0,
        })
        .expect("idle evidence serializes");
        assert_eq!(
            idle,
            serde_json::json!({
                "duration_ms": 30_000,
                "process_cpu_ms": 120,
                "one_core_cpu_percent": 0.4,
                "redraw_count": 0,
                "present_count": 0
            })
        );

        let resource = serde_json::to_value(ResourceSample {
            elapsed_ms: 4_000,
            checkpoint: "stress_80_percent",
            stress_progress_percent: Some(80),
            quad_capacity_floats: 1,
            raster_capacity_floats: 2,
            text_row_capacity: 3,
            raster_cache_entries: 4,
            raster_cache_bytes: 5,
            shaping_cache_entries: 6,
            shaping_cache_accounted_bytes: 7,
        })
        .expect("resource sample serializes");
        assert_eq!(resource["checkpoint"], "stress_80_percent");
        assert_eq!(resource["stress_progress_percent"], 80);
        assert_eq!(resource["quad_capacity_floats"], 1);
        assert_eq!(resource["shaping_cache_accounted_bytes"], 7);
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
