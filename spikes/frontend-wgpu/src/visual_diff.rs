use std::{
    collections::BTreeSet,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use png::{BitDepth, ColorType, Decoder, Limits};
use serde::{Deserialize, Serialize};

pub const PROFILE_ID: &str = "macbook-pro-metal-scale2";
pub const PHYSICAL_WIDTH: u32 = 1_600;
pub const PHYSICAL_HEIGHT: u32 = 1_200;
pub const LOGICAL_WIDTH: u32 = 800;
pub const LOGICAL_HEIGHT: u32 = 600;
pub const BACKING_SCALE: f64 = 2.0;
pub const SCENE_COLUMNS: u16 = 102;
pub const SCENE_ROWS: u16 = 35;
pub const SSIM_THRESHOLD: f64 = 0.995;
pub const CHANGED_PIXEL_THRESHOLD: f64 = 0.01;
const MAX_MASK_COVERAGE: f64 = 0.05;
const MIN_VALID_SSIM_CENTERS: f64 = 0.90;
const CHANNEL_DELTA_THRESHOLD: u8 = 2;
const SSIM_RADIUS: usize = 5;
const SSIM_SIGMA: f64 = 1.5;
const SSIM_K1: f64 = 0.01;
const SSIM_K2: f64 = 0.03;
const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenarioId {
    Typography,
    CalmTerminal,
    DenseWorkspace,
    Attention,
    Palette,
    FullModal,
    Welcome,
    ContextMenu,
    Artifacts,
    Narrow,
    Restored,
    AttentionMotionStart,
    AttentionMotionMidpoint,
    AttentionMotionEnd,
    AttentionReduced,
}

impl ScenarioId {
    pub const ALL: [Self; 15] = [
        Self::Typography,
        Self::CalmTerminal,
        Self::DenseWorkspace,
        Self::Attention,
        Self::Palette,
        Self::FullModal,
        Self::Welcome,
        Self::ContextMenu,
        Self::Artifacts,
        Self::Narrow,
        Self::Restored,
        Self::AttentionMotionStart,
        Self::AttentionMotionMidpoint,
        Self::AttentionMotionEnd,
        Self::AttentionReduced,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Typography => "typography",
            Self::CalmTerminal => "calm-terminal",
            Self::DenseWorkspace => "dense-workspace",
            Self::Attention => "attention",
            Self::Palette => "palette",
            Self::FullModal => "full-modal",
            Self::Welcome => "welcome",
            Self::ContextMenu => "context-menu",
            Self::Artifacts => "artifacts",
            Self::Narrow => "narrow",
            Self::Restored => "restored",
            Self::AttentionMotionStart => "attention-motion-start",
            Self::AttentionMotionMidpoint => "attention-motion-midpoint",
            Self::AttentionMotionEnd => "attention-motion-end",
            Self::AttentionReduced => "attention-reduced",
        }
    }

    pub const fn base_scenario(self) -> &'static str {
        match self {
            Self::AttentionMotionStart
            | Self::AttentionMotionMidpoint
            | Self::AttentionMotionEnd
            | Self::AttentionReduced => "attention",
            _ => self.as_str(),
        }
    }

    pub const fn checkpoint(self) -> Option<&'static str> {
        match self {
            Self::AttentionMotionStart => Some("start"),
            Self::AttentionMotionMidpoint => Some("midpoint"),
            Self::AttentionMotionEnd => Some("end"),
            Self::AttentionReduced => Some("reduced"),
            _ => None,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.as_str() == value)
            .ok_or_else(|| format!("unknown scenario {value:?}"))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureMetadata {
    pub schema_version: u32,
    pub profile: String,
    pub scenario: String,
    pub theme: String,
    pub captured_at: String,
    pub surface: SurfaceMetadata,
    pub scene: SceneMetadata,
    pub font: FontMetadata,
    pub display: DisplayMetadata,
    pub gpu: GpuMetadata,
    pub source: SourceMetadata,
    pub build: BuildMetadata,
    pub capture: CaptureMethodMetadata,
    #[serde(default)]
    pub fallback_regions: Vec<String>,
    pub acceptance: Option<AcceptanceMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceMetadata {
    pub logical_width: u32,
    pub logical_height: u32,
    pub physical_width: u32,
    pub physical_height: u32,
    pub backing_scale: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SceneMetadata {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FontMetadata {
    pub source: String,
    pub family: String,
    pub size: f64,
    pub faces: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayMetadata {
    pub id: String,
    pub name: String,
    pub refresh_hz: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GpuMetadata {
    pub name: String,
    pub backend: String,
    pub device_type: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadata {
    pub commit: String,
    pub dirty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildMetadata {
    pub profile: String,
    pub executable: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureMethodMetadata {
    pub api: String,
    pub color_space: String,
    pub pixel_format: String,
    pub client_surface: bool,
    pub shows_cursor: bool,
    pub includes_shadow: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceMetadata {
    pub reason: String,
    pub accepted_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaskFile {
    schema_version: u32,
    coordinate_space: String,
    rectangles: Vec<MaskRectangle>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaskRectangle {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    kind: String,
    fallback_id: String,
    reason: String,
}

#[derive(Clone, Debug)]
struct DecodedImage {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComparisonReport {
    pub schema_version: u32,
    pub profile: String,
    pub scenario: String,
    pub ssim: f64,
    pub changed_pixel_fraction: f64,
    pub changed_pixels: usize,
    pub compared_pixels: usize,
    pub masked_pixels: usize,
    pub valid_ssim_centers: usize,
    pub total_ssim_centers: usize,
    pub passed: bool,
}

pub fn run_cli(args: impl IntoIterator<Item = String>) -> Result<bool, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or_else(usage)?;
    let mut profile = None;
    let mut scenario = None;
    let mut reason = None;
    while let Some(argument) = args.next() {
        let target = match argument.as_str() {
            "--profile" => &mut profile,
            "--scenario" => &mut scenario,
            "--reason" => &mut reason,
            "--help" | "-h" => return Err(usage()),
            _ => return Err(format!("unknown argument {argument:?}\n{}", usage())),
        };
        let value = args
            .next()
            .ok_or_else(|| format!("{argument} requires a value"))?;
        if value.starts_with("--") {
            return Err(format!("{argument} requires a value"));
        }
        *target = Some(value);
    }
    let profile = profile.ok_or_else(|| "--profile is required".to_owned())?;
    validate_profile(&profile)?;
    let scenario = ScenarioId::parse(
        scenario
            .as_deref()
            .ok_or_else(|| "--scenario is required".to_owned())?,
    )?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    match command.as_str() {
        "compare" => {
            if reason.is_some() {
                return Err("--reason is valid only for accept".to_owned());
            }
            let report = compare_at(root, &profile, scenario)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .map_err(|error| format!("could not serialize comparison: {error}"))?
            );
            Ok(report.passed)
        }
        "accept" => {
            let reason = reason.ok_or_else(|| "accept requires --reason".to_owned())?;
            accept_at(root, &profile, scenario, &reason)?;
            Ok(true)
        }
        _ => Err(format!("unknown command {command:?}\n{}", usage())),
    }
}

fn usage() -> String {
    "usage: visual-diff <compare|accept> --profile <id> --scenario <id> [--reason <text>]"
        .to_owned()
}

pub fn compare_at(
    root: &Path,
    profile: &str,
    scenario: ScenarioId,
) -> Result<ComparisonReport, String> {
    validate_profile(profile)?;
    let baseline_dir = baseline_dir(root, profile, scenario);
    let candidate_dir = candidate_dir(root, profile, scenario);
    let baseline_metadata =
        read_metadata(&baseline_dir.join("metadata.json"), profile, scenario, true)?;
    let candidate_metadata = read_metadata(
        &candidate_dir.join("metadata.json"),
        profile,
        scenario,
        false,
    )?;
    validate_compatible_metadata(&baseline_metadata, &candidate_metadata)?;

    let baseline = decode_png(&baseline_dir.join("baseline.png"))?;
    let candidate = decode_png(&candidate_dir.join("candidate.png"))?;
    validate_image_dimensions(&baseline)?;
    validate_image_dimensions(&candidate)?;
    if baseline.width != candidate.width || baseline.height != candidate.height {
        return Err("baseline and candidate image dimensions differ".to_owned());
    }

    let mask = read_mask(
        &baseline_dir.join("mask.json"),
        baseline.width,
        baseline.height,
        &baseline_metadata,
        &candidate_metadata,
    )?;
    compare_images(profile, scenario, &baseline, &candidate, &mask)
}

pub fn accept_at(
    root: &Path,
    profile: &str,
    scenario: ScenarioId,
    reason: &str,
) -> Result<(), String> {
    validate_profile(profile)?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("accept requires a nonblank --reason".to_owned());
    }
    let candidate_dir = candidate_dir(root, profile, scenario);
    let baseline_dir = baseline_dir(root, profile, scenario);
    let mut metadata = read_metadata(
        &candidate_dir.join("metadata.json"),
        profile,
        scenario,
        false,
    )?;
    if metadata.source.dirty {
        return Err("refusing to accept a candidate captured from a dirty source tree".to_owned());
    }
    let image = decode_png(&candidate_dir.join("candidate.png"))?;
    validate_image_dimensions(&image)?;
    read_mask(
        &baseline_dir.join("mask.json"),
        image.width,
        image.height,
        &metadata,
        &metadata,
    )?;

    metadata.acceptance = Some(AcceptanceMetadata {
        reason: reason.to_owned(),
        accepted_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "system clock predates the Unix epoch".to_owned())?
            .as_secs(),
    });
    let encoded_metadata = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| format!("could not serialize accepted metadata: {error}"))?;
    let encoded_image = fs::read(candidate_dir.join("candidate.png"))
        .map_err(|error| format!("could not read candidate PNG: {error}"))?;

    fs::create_dir_all(&baseline_dir)
        .map_err(|error| format!("could not create baseline directory: {error}"))?;
    atomic_replace(&baseline_dir.join("baseline.png"), &encoded_image)?;
    atomic_replace(&baseline_dir.join("metadata.json"), &encoded_metadata)?;
    Ok(())
}

fn validate_profile(profile: &str) -> Result<(), String> {
    if profile != PROFILE_ID {
        return Err(format!(
            "unknown profile {profile:?}; expected {PROFILE_ID:?}"
        ));
    }
    Ok(())
}

fn read_metadata(
    path: &Path,
    profile: &str,
    scenario: ScenarioId,
    baseline: bool,
) -> Result<CaptureMetadata, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let metadata: CaptureMetadata = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    validate_metadata(&metadata, profile, scenario, baseline)?;
    Ok(metadata)
}

fn validate_metadata(
    metadata: &CaptureMetadata,
    profile: &str,
    scenario: ScenarioId,
    baseline: bool,
) -> Result<(), String> {
    if metadata.schema_version != 1 {
        return Err("metadata schema_version must be 1".to_owned());
    }
    if metadata.profile != profile || metadata.scenario != scenario.as_str() {
        return Err("metadata profile/scenario does not match the requested artifact".to_owned());
    }
    if baseline && metadata.acceptance.is_none() {
        return Err("baseline metadata lacks an explicit acceptance record".to_owned());
    }
    if !baseline && metadata.acceptance.is_some() {
        return Err("candidate metadata must not contain an acceptance record".to_owned());
    }
    if baseline && metadata.source.dirty {
        return Err("accepted baseline metadata records a dirty source tree".to_owned());
    }
    if metadata.theme != "mandatum-dark" {
        return Err("fixed profile requires the mandatum-dark theme".to_owned());
    }
    if metadata.captured_at.trim().is_empty() {
        return Err("metadata captured_at must be nonblank".to_owned());
    }
    let surface = &metadata.surface;
    if (
        surface.logical_width,
        surface.logical_height,
        surface.physical_width,
        surface.physical_height,
    ) != (
        LOGICAL_WIDTH,
        LOGICAL_HEIGHT,
        PHYSICAL_WIDTH,
        PHYSICAL_HEIGHT,
    ) || (surface.backing_scale - BACKING_SCALE).abs() > f64::EPSILON
    {
        return Err("metadata does not match the fixed reference surface".to_owned());
    }
    if (metadata.scene.columns, metadata.scene.rows) != (SCENE_COLUMNS, SCENE_ROWS) {
        return Err("metadata does not match the fixed 102x35 scene".to_owned());
    }
    if metadata.font.source != "bundled"
        || metadata.font.family != "JetBrains Mono"
        || (metadata.font.size - 13.0).abs() > f64::EPSILON
        || metadata.font.faces.is_empty()
        || metadata
            .font
            .faces
            .iter()
            .any(|face| face.trim().is_empty())
    {
        return Err("metadata does not match bundled JetBrains Mono 13".to_owned());
    }
    if metadata.display.id.trim().is_empty()
        || metadata.display.name.trim().is_empty()
        || !metadata.display.refresh_hz.is_finite()
        || metadata.display.refresh_hz <= 0.0
    {
        return Err("metadata display identity/refresh is incomplete".to_owned());
    }
    if metadata.gpu.name.trim().is_empty()
        || metadata.gpu.backend.trim().is_empty()
        || metadata.gpu.device_type.trim().is_empty()
    {
        return Err("metadata GPU identity is incomplete".to_owned());
    }
    if metadata.source.commit.len() != 40
        || !metadata
            .source
            .commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("metadata source commit must be a full lowercase Git object ID".to_owned());
    }
    if !matches!(metadata.build.profile.as_str(), "debug" | "release")
        || metadata.build.executable.trim().is_empty()
    {
        return Err("metadata build identity is incomplete".to_owned());
    }
    let capture = &metadata.capture;
    if capture.api != "ScreenCaptureKit"
        || capture.color_space != "srgb"
        || capture.pixel_format != "rgba8-unpremultiplied"
        || !capture.client_surface
        || capture.shows_cursor
        || capture.includes_shadow
    {
        return Err("metadata does not describe the fixed client-surface capture".to_owned());
    }
    let fallback_regions = metadata
        .fallback_regions
        .iter()
        .map(|region| region.trim())
        .collect::<BTreeSet<_>>();
    if fallback_regions.len() != metadata.fallback_regions.len() || fallback_regions.contains("") {
        return Err("metadata fallback region identifiers must be unique and nonblank".to_owned());
    }
    if let Some(acceptance) = &metadata.acceptance
        && acceptance.reason.trim().is_empty()
    {
        return Err("metadata acceptance reason must be nonblank".to_owned());
    }
    Ok(())
}

fn validate_compatible_metadata(
    baseline: &CaptureMetadata,
    candidate: &CaptureMetadata,
) -> Result<(), String> {
    if baseline.surface.logical_width != candidate.surface.logical_width
        || baseline.surface.logical_height != candidate.surface.logical_height
        || baseline.surface.physical_width != candidate.surface.physical_width
        || baseline.surface.physical_height != candidate.surface.physical_height
        || (baseline.surface.backing_scale - candidate.surface.backing_scale).abs() > f64::EPSILON
        || baseline.scene.columns != candidate.scene.columns
        || baseline.scene.rows != candidate.scene.rows
        || baseline.font.source != candidate.font.source
        || baseline.font.family != candidate.font.family
        || (baseline.font.size - candidate.font.size).abs() > f64::EPSILON
        || baseline.font.faces != candidate.font.faces
        || baseline.theme != candidate.theme
        || baseline.display.id != candidate.display.id
        || baseline.display.name != candidate.display.name
        || (baseline.display.refresh_hz - candidate.display.refresh_hz).abs() > 0.1
        || baseline.gpu.name != candidate.gpu.name
        || baseline.gpu.backend != candidate.gpu.backend
        || baseline.gpu.device_type != candidate.gpu.device_type
    {
        return Err("baseline and candidate fixed-reference metadata are incompatible".to_owned());
    }
    Ok(())
}

fn decode_png(path: &Path) -> Result<DecodedImage, String> {
    let encoded =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if encoded.len() > MAX_PNG_BYTES {
        return Err(format!("{} exceeds the 32 MiB PNG limit", path.display()));
    }
    let mut decoder = Decoder::new(Cursor::new(encoded));
    decoder.set_limits(Limits {
        bytes: PHYSICAL_WIDTH as usize * PHYSICAL_HEIGHT as usize * 4,
    });
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    let info = reader.info();
    if info.animation_control.is_some() || info.interlaced {
        return Err(format!(
            "{} must be a non-interlaced still PNG",
            path.display()
        ));
    }
    if info.bit_depth != BitDepth::Eight
        || !matches!(info.color_type, ColorType::Rgb | ColorType::Rgba)
    {
        return Err(format!(
            "{} must use 8-bit RGB or RGBA pixels",
            path.display()
        ));
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{} decoded size overflows", path.display()))?;
    let mut decoded = vec![0; output_size];
    let output = reader
        .next_frame(&mut decoded)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    decoded.truncate(output.buffer_size());
    let rgba = match output.color_type {
        ColorType::Rgb => decoded
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        ColorType::Rgba => {
            unpremultiply_rgba8(&mut decoded);
            decoded
        }
        _ => {
            return Err(format!(
                "{} decoded to an unsupported format",
                path.display()
            ));
        }
    };
    Ok(DecodedImage {
        width: output.width as usize,
        height: output.height as usize,
        rgba,
    })
}

fn unpremultiply_rgba8(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel[..3].fill(0);
        } else if alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

fn validate_image_dimensions(image: &DecodedImage) -> Result<(), String> {
    if (image.width, image.height) != (PHYSICAL_WIDTH as usize, PHYSICAL_HEIGHT as usize) {
        return Err(format!(
            "image is {}x{}, expected {}x{}",
            image.width, image.height, PHYSICAL_WIDTH, PHYSICAL_HEIGHT
        ));
    }
    let expected = image
        .width
        .checked_mul(image.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "image dimensions overflow".to_owned())?;
    if image.rgba.len() != expected {
        return Err("image RGBA buffer has the wrong length".to_owned());
    }
    Ok(())
}

fn read_mask(
    path: &Path,
    width: usize,
    height: usize,
    baseline: &CaptureMetadata,
    candidate: &CaptureMetadata,
) -> Result<Vec<bool>, String> {
    let mut mask = vec![false; width * height];
    if !path.exists() {
        return Ok(mask);
    }
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let file: MaskFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    if file.schema_version != 1 || file.coordinate_space != "physical-client-surface-pixels" {
        return Err("mask schema/coordinate space is invalid".to_owned());
    }
    let baseline_regions = baseline
        .fallback_regions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let candidate_regions = candidate
        .fallback_regions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for rectangle in file.rectangles {
        if rectangle.width == 0 || rectangle.height == 0 {
            return Err("mask rectangles must have positive dimensions".to_owned());
        }
        if rectangle.kind != "os-fallback-glyph"
            || rectangle.reason.trim().is_empty()
            || !baseline_regions.contains(rectangle.fallback_id.as_str())
            || !candidate_regions.contains(rectangle.fallback_id.as_str())
        {
            return Err(
                "mask rectangles may cover only recorded OS fallback glyph regions".to_owned(),
            );
        }
        let right = rectangle
            .x
            .checked_add(rectangle.width)
            .ok_or_else(|| "mask rectangle overflows".to_owned())?;
        let bottom = rectangle
            .y
            .checked_add(rectangle.height)
            .ok_or_else(|| "mask rectangle overflows".to_owned())?;
        if right > width as u32 || bottom > height as u32 {
            return Err("mask rectangle lies outside the client surface".to_owned());
        }
        for y in rectangle.y as usize..bottom as usize {
            let row = y * width;
            for x in rectangle.x as usize..right as usize {
                mask[row + x] = true;
            }
        }
    }
    let masked = mask.iter().filter(|value| **value).count();
    if masked as f64 / mask.len() as f64 > MAX_MASK_COVERAGE {
        return Err("mask covers more than 5% of client-surface pixels".to_owned());
    }
    Ok(mask)
}

fn compare_images(
    profile: &str,
    scenario: ScenarioId,
    baseline: &DecodedImage,
    candidate: &DecodedImage,
    mask: &[bool],
) -> Result<ComparisonReport, String> {
    if baseline.width != candidate.width
        || baseline.height != candidate.height
        || baseline.rgba.len() != candidate.rgba.len()
        || mask.len() != baseline.width * baseline.height
    {
        return Err("comparison inputs have incompatible dimensions".to_owned());
    }
    let masked_pixels = mask.iter().filter(|masked| **masked).count();
    let compared_pixels = mask.len() - masked_pixels;
    if compared_pixels == 0 {
        return Err("mask leaves no pixels to compare".to_owned());
    }
    let changed_pixels = baseline
        .rgba
        .chunks_exact(4)
        .zip(candidate.rgba.chunks_exact(4))
        .zip(mask)
        .filter(|((baseline, candidate), masked)| {
            !**masked
                && (0..3).any(|channel| {
                    baseline[channel].abs_diff(candidate[channel]) > CHANNEL_DELTA_THRESHOLD
                })
        })
        .count();
    let changed_pixel_fraction = changed_pixels as f64 / compared_pixels as f64;
    let baseline_luma = luminance(&baseline.rgba);
    let candidate_luma = luminance(&candidate.rgba);
    let (ssim, valid_ssim_centers) = ssim(
        &baseline_luma,
        &candidate_luma,
        baseline.width,
        baseline.height,
        mask,
    )?;
    let total_ssim_centers = baseline.width * baseline.height;
    if valid_ssim_centers as f64 / (total_ssim_centers as f64) < MIN_VALID_SSIM_CENTERS {
        return Err("mask leaves fewer than 90% of SSIM window centers valid".to_owned());
    }
    let passed = ssim >= SSIM_THRESHOLD && changed_pixel_fraction <= CHANGED_PIXEL_THRESHOLD;
    Ok(ComparisonReport {
        schema_version: 1,
        profile: profile.to_owned(),
        scenario: scenario.as_str().to_owned(),
        ssim,
        changed_pixel_fraction,
        changed_pixels,
        compared_pixels,
        masked_pixels,
        valid_ssim_centers,
        total_ssim_centers,
        passed,
    })
}

fn luminance(rgba: &[u8]) -> Vec<f64> {
    rgba.chunks_exact(4)
        .map(|pixel| {
            let red = srgb_to_linear(pixel[0]);
            let green = srgb_to_linear(pixel[1]);
            let blue = srgb_to_linear(pixel[2]);
            0.2126 * red + 0.7152 * green + 0.0722 * blue
        })
        .collect()
}

fn srgb_to_linear(value: u8) -> f64 {
    let value = f64::from(value) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn ssim(
    left: &[f64],
    right: &[f64],
    width: usize,
    height: usize,
    mask: &[bool],
) -> Result<(f64, usize), String> {
    if left.len() != width * height || right.len() != left.len() || mask.len() != left.len() {
        return Err("SSIM inputs have incompatible dimensions".to_owned());
    }
    let kernel = gaussian_kernel();
    let left_sq = left.iter().map(|value| value * value).collect::<Vec<_>>();
    let right_sq = right.iter().map(|value| value * value).collect::<Vec<_>>();
    let cross = left
        .iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .collect::<Vec<_>>();
    let horizontal = [
        horizontal_blur(left, width, height, &kernel),
        horizontal_blur(right, width, height, &kernel),
        horizontal_blur(&left_sq, width, height, &kernel),
        horizontal_blur(&right_sq, width, height, &kernel),
        horizontal_blur(&cross, width, height, &kernel),
    ];
    let mask_prefix = mask_prefix(mask, width, height);
    let c1 = (SSIM_K1 * 1.0).powi(2);
    let c2 = (SSIM_K2 * 1.0).powi(2);
    let mut sum = 0.0;
    let mut valid = 0;
    for y in 0..height {
        for x in 0..width {
            if window_is_masked(&mask_prefix, width, height, x, y) {
                continue;
            }
            let index = y * width + x;
            let mut moments = [0.0; 5];
            for (kernel_index, weight) in kernel.iter().enumerate() {
                let sample_y = clamp_offset(y, kernel_index, height);
                let sample_index = sample_y * width + x;
                for moment in 0..5 {
                    moments[moment] += horizontal[moment][sample_index] * weight;
                }
            }
            let left_mean = moments[0];
            let right_mean = moments[1];
            let left_variance = (moments[2] - left_mean * left_mean).max(0.0);
            let right_variance = (moments[3] - right_mean * right_mean).max(0.0);
            let covariance = moments[4] - left_mean * right_mean;
            let numerator = (2.0 * left_mean * right_mean + c1) * (2.0 * covariance + c2);
            let denominator = (left_mean * left_mean + right_mean * right_mean + c1)
                * (left_variance + right_variance + c2);
            if !denominator.is_finite() || denominator <= 0.0 {
                return Err(format!("SSIM denominator is invalid at pixel {index}"));
            }
            sum += numerator / denominator;
            valid += 1;
        }
    }
    if valid == 0 {
        return Err("mask leaves no valid SSIM window centers".to_owned());
    }
    Ok((sum / valid as f64, valid))
}

fn gaussian_kernel() -> [f64; 11] {
    let mut kernel = [0.0; 11];
    let mut sum = 0.0;
    for (index, value) in kernel.iter_mut().enumerate() {
        let distance = index as f64 - SSIM_RADIUS as f64;
        *value = (-distance * distance / (2.0 * SSIM_SIGMA * SSIM_SIGMA)).exp();
        sum += *value;
    }
    for value in &mut kernel {
        *value /= sum;
    }
    kernel
}

fn horizontal_blur(values: &[f64], width: usize, height: usize, kernel: &[f64; 11]) -> Vec<f64> {
    let mut output = vec![0.0; values.len()];
    for y in 0..height {
        for x in 0..width {
            let mut value = 0.0;
            for (kernel_index, weight) in kernel.iter().enumerate() {
                let sample_x = clamp_offset(x, kernel_index, width);
                value += values[y * width + sample_x] * weight;
            }
            output[y * width + x] = value;
        }
    }
    output
}

fn clamp_offset(center: usize, kernel_index: usize, limit: usize) -> usize {
    let offset = kernel_index as isize - SSIM_RADIUS as isize;
    center.saturating_add_signed(offset).min(limit - 1)
}

fn mask_prefix(mask: &[bool], width: usize, height: usize) -> Vec<u32> {
    let stride = width + 1;
    let mut prefix = vec![0_u32; stride * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0_u32;
        for x in 0..width {
            row_sum += u32::from(mask[y * width + x]);
            prefix[(y + 1) * stride + x + 1] = prefix[y * stride + x + 1] + row_sum;
        }
    }
    prefix
}

fn window_is_masked(prefix: &[u32], width: usize, height: usize, x: usize, y: usize) -> bool {
    let left = x.saturating_sub(SSIM_RADIUS);
    let top = y.saturating_sub(SSIM_RADIUS);
    let right = x.saturating_add(SSIM_RADIUS).min(width - 1) + 1;
    let bottom = y.saturating_add(SSIM_RADIUS).min(height - 1) + 1;
    let stride = width + 1;
    let count = prefix[bottom * stride + right] + prefix[top * stride + left]
        - prefix[top * stride + right]
        - prefix[bottom * stride + left];
    count > 0
}

fn baseline_dir(root: &Path, profile: &str, scenario: ScenarioId) -> PathBuf {
    root.join("visual-baselines")
        .join(profile)
        .join(scenario.as_str())
}

fn candidate_dir(root: &Path, profile: &str, scenario: ScenarioId) -> PathBuf {
    root.join("visual-candidates")
        .join(profile)
        .join(scenario.as_str())
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no usable file name", path.display()))?;
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("could not replace {}: {error}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs::{self, File},
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use png::{BitDepth, ColorType, Encoder, SrgbRenderingIntent};

    use super::*;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(label: &str) -> Self {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mandatum-visual-diff-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("test root");
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn metadata(scenario: ScenarioId, dirty: bool, accepted: bool) -> CaptureMetadata {
        CaptureMetadata {
            schema_version: 1,
            profile: PROFILE_ID.to_owned(),
            scenario: scenario.as_str().to_owned(),
            theme: "mandatum-dark".to_owned(),
            captured_at: "2026-07-24T12:00:00Z".to_owned(),
            surface: SurfaceMetadata {
                logical_width: LOGICAL_WIDTH,
                logical_height: LOGICAL_HEIGHT,
                physical_width: PHYSICAL_WIDTH,
                physical_height: PHYSICAL_HEIGHT,
                backing_scale: BACKING_SCALE,
            },
            scene: SceneMetadata {
                columns: SCENE_COLUMNS,
                rows: SCENE_ROWS,
            },
            font: FontMetadata {
                source: "bundled".to_owned(),
                family: "JetBrains Mono".to_owned(),
                size: 13.0,
                faces: vec![
                    "JetBrains Mono Regular".to_owned(),
                    "JetBrains Mono Bold".to_owned(),
                ],
            },
            display: DisplayMetadata {
                id: "display-1".to_owned(),
                name: "Reference".to_owned(),
                refresh_hz: 60.0,
            },
            gpu: GpuMetadata {
                name: "Apple M4 Pro".to_owned(),
                backend: "Metal".to_owned(),
                device_type: "IntegratedGpu".to_owned(),
            },
            source: SourceMetadata {
                commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
                dirty,
            },
            build: BuildMetadata {
                profile: "debug".to_owned(),
                executable: "mandatum-native-lab".to_owned(),
            },
            capture: CaptureMethodMetadata {
                api: "ScreenCaptureKit".to_owned(),
                color_space: "srgb".to_owned(),
                pixel_format: "rgba8-unpremultiplied".to_owned(),
                client_surface: true,
                shows_cursor: false,
                includes_shadow: false,
            },
            fallback_regions: Vec::new(),
            acceptance: accepted.then(|| AcceptanceMetadata {
                reason: "initial reference".to_owned(),
                accepted_at_unix_seconds: 1,
            }),
        }
    }

    fn write_metadata(path: &Path, metadata: &CaptureMetadata) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(metadata).unwrap()).unwrap();
    }

    fn write_png(path: &Path, rgba: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = File::create(path).unwrap();
        let mut encoder = Encoder::new(file, PHYSICAL_WIDTH, PHYSICAL_HEIGHT);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_source_srgb(SrgbRenderingIntent::Perceptual);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(rgba)
            .unwrap();
    }

    fn uniform_rgba(value: u8) -> Vec<u8> {
        let mut pixels = vec![value; PHYSICAL_WIDTH as usize * PHYSICAL_HEIGHT as usize * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        pixels
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, output);
                } else {
                    output.insert(path, fs::read(entry.path()).unwrap());
                }
            }
        }
        let mut output = BTreeMap::new();
        visit(root, &mut output);
        output
    }

    fn prepare_pair(root: &Path, scenario: ScenarioId, candidate_dirty: bool) {
        let baseline = baseline_dir(root, PROFILE_ID, scenario);
        let candidate = candidate_dir(root, PROFILE_ID, scenario);
        write_metadata(
            &baseline.join("metadata.json"),
            &metadata(scenario, false, true),
        );
        write_metadata(
            &candidate.join("metadata.json"),
            &metadata(scenario, candidate_dirty, false),
        );
        let pixels = uniform_rgba(24);
        write_png(&baseline.join("baseline.png"), &pixels);
        write_png(&candidate.join("candidate.png"), &pixels);
    }

    #[test]
    fn canonical_scenario_ids_match_the_plan() {
        assert_eq!(
            ScenarioId::ALL.map(ScenarioId::as_str),
            [
                "typography",
                "calm-terminal",
                "dense-workspace",
                "attention",
                "palette",
                "full-modal",
                "welcome",
                "context-menu",
                "artifacts",
                "narrow",
                "restored",
                "attention-motion-start",
                "attention-motion-midpoint",
                "attention-motion-end",
                "attention-reduced",
            ]
        );
    }

    #[test]
    fn attention_checkpoint_ids_map_to_the_base_fixture_and_capture_instant() {
        assert_eq!(
            ScenarioId::AttentionMotionStart.base_scenario(),
            "attention"
        );
        assert_eq!(ScenarioId::AttentionMotionStart.checkpoint(), Some("start"));
        assert_eq!(
            ScenarioId::AttentionMotionMidpoint.checkpoint(),
            Some("midpoint")
        );
        assert_eq!(ScenarioId::AttentionMotionEnd.checkpoint(), Some("end"));
        assert_eq!(ScenarioId::AttentionReduced.checkpoint(), Some("reduced"));
        assert_eq!(ScenarioId::Attention.base_scenario(), "attention");
        assert_eq!(ScenarioId::Attention.checkpoint(), None);
    }

    #[test]
    fn metadata_validation_is_strict_about_fixed_profile() {
        let mut value = serde_json::to_value(metadata(ScenarioId::Palette, false, false)).unwrap();
        value["surface"]["backing_scale"] = serde_json::json!(1.0);
        let decoded_metadata: CaptureMetadata = serde_json::from_value(value).unwrap();
        assert!(
            validate_metadata(&decoded_metadata, PROFILE_ID, ScenarioId::Palette, false,)
                .unwrap_err()
                .contains("fixed reference surface")
        );

        let mut value = serde_json::to_value(metadata(ScenarioId::Palette, false, false)).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<CaptureMetadata>(value).is_err());
    }

    #[test]
    fn unpremultiply_handles_translucent_and_zero_alpha_pixels() {
        let mut pixels = [64, 32, 16, 128, 255, 1, 2, 0];
        unpremultiply_rgba8(&mut pixels);
        assert_eq!(pixels, [128, 64, 32, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn changed_pixel_threshold_ignores_delta_two_but_counts_delta_three() {
        let baseline = DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![10, 10, 10, 255],
        };
        let mut candidate = baseline.clone();
        candidate.rgba[0] = 12;
        let report = compare_images(
            PROFILE_ID,
            ScenarioId::Palette,
            &baseline,
            &candidate,
            &[false],
        )
        .unwrap();
        assert_eq!(report.changed_pixels, 0);

        candidate.rgba[0] = 13;
        let report = compare_images(
            PROFILE_ID,
            ScenarioId::Palette,
            &baseline,
            &candidate,
            &[false],
        )
        .unwrap();
        assert_eq!(report.changed_pixels, 1);
    }

    #[test]
    fn identical_images_have_perfect_ssim() {
        let image = DecodedImage {
            width: 3,
            height: 2,
            rgba: [30, 40, 50, 255].repeat(6),
        };
        let report = compare_images(
            PROFILE_ID,
            ScenarioId::Typography,
            &image,
            &image,
            &[false; 6],
        )
        .unwrap();
        assert!((report.ssim - 1.0).abs() < 1e-12);
        assert_eq!(report.changed_pixel_fraction, 0.0);
        assert!(report.passed);
    }

    #[test]
    fn gaussian_window_is_normalized_symmetric_and_sigma_1_5() {
        let kernel = gaussian_kernel();
        assert!((kernel.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((kernel[5] - 0.266_011_724_861_794_36).abs() < 1e-12);
        for index in 0..kernel.len() {
            assert!((kernel[index] - kernel[kernel.len() - 1 - index]).abs() < 1e-15);
        }
    }

    #[test]
    fn scattered_mask_refuses_fewer_than_ninety_percent_ssim_centers() {
        let image = DecodedImage {
            width: 20,
            height: 20,
            rgba: [30, 40, 50, 255].repeat(400),
        };
        let mut mask = vec![false; 400];
        mask[10 * 20 + 10] = true;
        assert!(
            compare_images(PROFILE_ID, ScenarioId::Typography, &image, &image, &mask,)
                .unwrap_err()
                .contains("fewer than 90%")
        );
    }

    #[test]
    fn mask_rejects_disallowed_or_excessive_regions() {
        let root = TestRoot::new("masks");
        let path = root.0.join("mask.json");
        fs::write(
            &path,
            r#"{"schema_version":1,"coordinate_space":"physical-client-surface-pixels","rectangles":[{"x":0,"y":0,"width":1,"height":1,"kind":"arbitrary","fallback_id":"emoji","reason":"hide"}]}"#,
        )
        .unwrap();
        let mut baseline = metadata(ScenarioId::Typography, false, true);
        baseline.fallback_regions = vec!["emoji".to_owned()];
        let mut candidate = metadata(ScenarioId::Typography, false, false);
        candidate.fallback_regions = vec!["emoji".to_owned()];
        assert!(
            read_mask(&path, 10, 10, &baseline, &candidate)
                .unwrap_err()
                .contains("OS fallback")
        );

        fs::write(
            &path,
            r#"{"schema_version":1,"coordinate_space":"physical-client-surface-pixels","rectangles":[{"x":0,"y":0,"width":6,"height":1,"kind":"os-fallback-glyph","fallback_id":"emoji","reason":"system emoji"}]}"#,
        )
        .unwrap();
        assert!(
            read_mask(&path, 10, 10, &baseline, &candidate)
                .unwrap_err()
                .contains("more than 5%")
        );
    }

    #[test]
    fn compare_is_successful_and_writes_no_files() {
        let root = TestRoot::new("compare-no-write");
        prepare_pair(&root.0, ScenarioId::CalmTerminal, false);
        let before = snapshot_tree(&root.0);
        let report = compare_at(&root.0, PROFILE_ID, ScenarioId::CalmTerminal).expect("comparison");
        let after = snapshot_tree(&root.0);
        assert!(report.passed);
        assert_eq!(before, after);
    }

    #[test]
    fn accept_requires_reason_and_refuses_dirty_candidate() {
        let root = TestRoot::new("accept-refusal");
        prepare_pair(&root.0, ScenarioId::Palette, true);
        assert!(
            accept_at(&root.0, PROFILE_ID, ScenarioId::Palette, " ")
                .unwrap_err()
                .contains("nonblank")
        );
        assert!(
            accept_at(
                &root.0,
                PROFILE_ID,
                ScenarioId::Palette,
                "intentional update"
            )
            .unwrap_err()
            .contains("dirty")
        );
    }

    #[test]
    fn accept_atomically_replaces_baseline_and_preserves_mask() {
        let root = TestRoot::new("accept");
        prepare_pair(&root.0, ScenarioId::Welcome, false);
        let baseline = baseline_dir(&root.0, PROFILE_ID, ScenarioId::Welcome);
        let mask = baseline.join("mask.json");
        fs::write(
            &mask,
            r#"{"schema_version":1,"coordinate_space":"physical-client-surface-pixels","rectangles":[]}"#,
        )
        .unwrap();
        let mask_before = fs::read(&mask).unwrap();
        accept_at(
            &root.0,
            PROFILE_ID,
            ScenarioId::Welcome,
            "record current native surface",
        )
        .unwrap();
        let accepted: CaptureMetadata =
            serde_json::from_slice(&fs::read(baseline.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(
            accepted.acceptance.unwrap().reason,
            "record current native surface"
        );
        assert_eq!(fs::read(mask).unwrap(), mask_before);
        assert!(fs::read_dir(baseline).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }
}
