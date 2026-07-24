//! App-owned artifact loading, validation, decoding, reload detection, and cache.
//!
//! Durable state carries only [`ArtifactPaneIntent`]. Filesystem handles,
//! metadata, decoder state, and RGBA bytes stay here and are projected into the
//! renderer-neutral scene only for the current frame.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, Metadata},
    io::{BufReader, Cursor, Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::Arc,
    thread::{self, JoinHandle},
    time::SystemTime,
};

use mandatum_core::{ArtifactPaneIntent, PaneId, PaneKind, SessionId, Workspace};
use mandatum_scene::{ArtifactContent, ArtifactState, RasterSurface};
use png::{BitDepth, ColorType, Decoder, Limits, Transformations};

use crate::events::{AppEvent, AppEventSender};

pub(crate) const MAX_ARTIFACT_ENCODED_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_ARTIFACT_DIMENSION: u32 = 4_096;
pub(crate) const MAX_ARTIFACT_DECODED_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONCURRENT_ARTIFACT_LOADS: usize = 4;
const MAX_ARTIFACT_PANES: usize = 64;

#[derive(Default)]
pub(crate) struct ArtifactPreviewStore {
    entries: BTreeMap<(SessionId, PaneId), CachedArtifact>,
    pending: VecDeque<PendingLoad>,
    active_reservations: BTreeMap<u64, usize>,
    workers: Vec<JoinHandle<()>>,
    next_token: u64,
    next_revision: u64,
}

impl ArtifactPreviewStore {
    pub(crate) fn refresh_active(&mut self, workspace: &Workspace, sender: &AppEventSender) {
        self.refresh_active_inner(workspace, None, sender);
    }

    pub(crate) fn force_reload_active(
        &mut self,
        workspace: &Workspace,
        pane_id: &PaneId,
        sender: &AppEventSender,
    ) {
        self.refresh_active_inner(workspace, Some(pane_id), sender);
    }

    pub(crate) fn content(
        &self,
        workspace: &Workspace,
        pane_id: &PaneId,
        intent: &ArtifactPaneIntent,
    ) -> ArtifactContent {
        let key = (workspace.active_session().id().clone(), pane_id.clone());
        ArtifactContent {
            source_label: intent.source.display().to_string(),
            alt_text: useful_alt_text(intent),
            fit: intent.fit,
            state: self
                .entries
                .get(&key)
                .map(|entry| entry.state.clone())
                .unwrap_or(ArtifactState::Loading),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.pending.clear();
    }

    pub(crate) fn apply_load_event(&mut self, event: ArtifactLoadEvent) -> bool {
        self.reap_finished_workers();
        let reservation = self.active_reservations.remove(&event.load_token);
        let released_reservation = reservation.is_some();
        let key = (event.session_id, event.pane_id);
        let Some(entry) = self.entries.get_mut(&key) else {
            return released_reservation;
        };
        if entry.load_token != event.load_token {
            return released_reservation;
        }
        entry.retry_when_capacity = false;
        entry.state = match event.result {
            Ok(decoded) if reservation == Some(decoded.rgba8.len()) => {
                let revision = self.next_revision.max(1);
                self.next_revision = revision
                    .checked_add(1)
                    .expect("artifact revision overflowed");
                ArtifactState::Ready(RasterSurface {
                    width: decoded.width,
                    height: decoded.height,
                    revision,
                    rgba8: Arc::from(decoded.rgba8),
                })
            }
            Ok(_) => ArtifactState::Failed {
                message: "artifact decoded size changed during load".to_owned(),
            },
            Err(message) => ArtifactState::Failed { message },
        };
        true
    }

    pub(crate) fn shutdown(&mut self) {
        self.pending.clear();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        self.active_reservations.clear();
    }

    fn refresh_active_inner(
        &mut self,
        workspace: &Workspace,
        force: Option<&PaneId>,
        sender: &AppEventSender,
    ) {
        self.reap_finished_workers();
        let session = workspace.active_session();
        let session_id = session.id().clone();
        let project_root = workspace.active_project_path();
        let artifacts = session
            .panes()
            .iter()
            .filter_map(|(pane_id, pane)| match pane.kind() {
                PaneKind::Artifact { intent } => Some((pane_id.clone(), intent.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();

        self.entries.retain(|(cached_session, cached_pane), _| {
            cached_session == &session_id
                && artifacts.iter().any(|(pane_id, _)| pane_id == cached_pane)
        });
        self.pending.retain(|pending| {
            pending.session_id == session_id
                && artifacts
                    .iter()
                    .any(|(pane_id, _)| pane_id == &pending.pane_id)
        });

        for (artifact_index, (pane_id, intent)) in artifacts.iter().cloned().enumerate() {
            let key = (session_id.clone(), pane_id.clone());
            if artifact_index >= MAX_ARTIFACT_PANES {
                let message =
                    format!("artifact preview count exceeds the {MAX_ARTIFACT_PANES}-pane limit");
                let observation = Err(message.clone());
                let unchanged = force != Some(&pane_id)
                    && self.entries.get(&key).is_some_and(|entry| {
                        entry.project_root == project_root
                            && entry.source == intent.source
                            && entry.observation == observation
                    });
                if unchanged {
                    continue;
                }
                self.pending.retain(|pending| pending.key() != key);
                self.entries.remove(&key);
                let load_token = self.allocate_load_token();
                self.entries.insert(
                    key,
                    CachedArtifact {
                        project_root: project_root.to_path_buf(),
                        source: intent.source,
                        observation,
                        state: ArtifactState::Failed { message },
                        load_token,
                        retry_when_capacity: false,
                    },
                );
                continue;
            }
            let observation = observe_source(project_root, &intent.source);
            let needs_load = force == Some(&pane_id)
                || self.entries.get(&key).is_none_or(|entry| {
                    entry.retry_when_capacity
                        || entry.project_root != project_root
                        || entry.source != intent.source
                        || entry.observation != observation
                });
            if !needs_load {
                continue;
            }

            self.pending.retain(|pending| pending.key() != key);
            self.entries.remove(&key);
            let load_token = self.allocate_load_token();
            self.entries.insert(
                key.clone(),
                CachedArtifact {
                    project_root: project_root.to_path_buf(),
                    source: intent.source.clone(),
                    observation: observation.clone(),
                    state: ArtifactState::Loading,
                    load_token,
                    retry_when_capacity: false,
                },
            );
            let opened = match &observation {
                Ok(_) => open_source(project_root, &intent.source),
                Err(message) => Err(message.clone()),
            };
            match opened {
                Ok(mut opened) => {
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.observation = Ok(opened.observation.clone());
                    }
                    let decoded_bytes = inspect_png_header(&mut opened.file).and_then(|bytes| {
                        opened
                            .file
                            .seek(SeekFrom::Start(0))
                            .map(|_| bytes)
                            .map_err(|_| "artifact file could not be rewound".to_owned())
                    });
                    let Ok(decoded_bytes) = decoded_bytes else {
                        if let Some(entry) = self.entries.get_mut(&key) {
                            entry.state = ArtifactState::Failed {
                                message: decoded_bytes.unwrap_err(),
                            };
                        }
                        continue;
                    };
                    let admitted = self
                        .aggregate_decoded_bytes()
                        .checked_add(decoded_bytes)
                        .is_some_and(|total| total <= MAX_ARTIFACT_DECODED_BYTES);
                    if admitted {
                        self.pending.push_back(PendingLoad {
                            session_id: session_id.clone(),
                            pane_id: pane_id.clone(),
                            load_token,
                            decoded_bytes,
                            file: opened.file,
                        });
                    } else {
                        let retry = self.has_stale_active_reservation();
                        let Some(entry) = self.entries.get_mut(&key) else {
                            continue;
                        };
                        entry.retry_when_capacity = retry;
                        entry.state = if retry {
                            ArtifactState::Loading
                        } else {
                            ArtifactState::Failed {
                                message: "artifact previews exceed the 64 MiB aggregate limit"
                                    .to_owned(),
                            }
                        };
                    }
                }
                Err(message) => {
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.observation = Err(message.clone());
                        entry.state = ArtifactState::Failed { message };
                    }
                }
            }
        }
        self.start_pending(sender);
    }

    fn allocate_load_token(&mut self) -> u64 {
        let load_token = self.next_token.max(1);
        self.next_token = load_token
            .checked_add(1)
            .expect("artifact load token overflowed");
        load_token
    }

    fn has_stale_active_reservation(&self) -> bool {
        self.active_reservations.keys().any(|token| {
            !self
                .entries
                .values()
                .any(|entry| entry.load_token == *token)
        })
    }

    fn start_pending(&mut self, sender: &AppEventSender) {
        while self.active_reservations.len() < MAX_CONCURRENT_ARTIFACT_LOADS {
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            let sender = sender.clone();
            self.active_reservations
                .insert(pending.load_token, pending.decoded_bytes);
            self.workers.push(thread::spawn(move || {
                let result = decode_png(pending.file, pending.decoded_bytes);
                let _ = sender.send(AppEvent::Artifact(ArtifactLoadEvent {
                    session_id: pending.session_id,
                    pane_id: pending.pane_id,
                    load_token: pending.load_token,
                    result,
                }));
            }));
        }
    }

    fn aggregate_decoded_bytes(&self) -> usize {
        let ready = self
            .entries
            .values()
            .filter_map(|entry| match &entry.state {
                ArtifactState::Ready(surface) => Some(surface.rgba8.len()),
                _ => None,
            })
            .sum::<usize>();
        let pending = self
            .pending
            .iter()
            .map(|pending| pending.decoded_bytes)
            .sum::<usize>();
        let active = self.active_reservations.values().copied().sum::<usize>();
        ready.saturating_add(pending).saturating_add(active)
    }

    fn reap_finished_workers(&mut self) {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].is_finished() {
                let worker = self.workers.swap_remove(index);
                let _ = worker.join();
            } else {
                index += 1;
            }
        }
    }
}

struct CachedArtifact {
    project_root: PathBuf,
    source: PathBuf,
    observation: Result<SourceObservation, String>,
    state: ArtifactState,
    load_token: u64,
    retry_when_capacity: bool,
}

struct PendingLoad {
    session_id: SessionId,
    pane_id: PaneId,
    load_token: u64,
    decoded_bytes: usize,
    file: File,
}

impl PendingLoad {
    fn key(&self) -> (SessionId, PaneId) {
        (self.session_id.clone(), self.pane_id.clone())
    }
}

#[derive(Debug)]
pub(crate) struct ArtifactLoadEvent {
    session_id: SessionId,
    pane_id: PaneId,
    load_token: u64,
    result: Result<DecodedArtifact, String>,
}

#[cfg(test)]
impl ArtifactLoadEvent {
    pub(crate) fn failed_for_test(session_id: SessionId, pane_id: PaneId, load_token: u64) -> Self {
        Self {
            session_id,
            pane_id,
            load_token,
            result: Err("test completion".to_owned()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceObservation {
    encoded_len: u64,
    modified: Option<SystemTime>,
}

struct OpenedSource {
    file: File,
    observation: SourceObservation,
}

#[derive(Debug)]
struct DecodedArtifact {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

fn useful_alt_text(intent: &ArtifactPaneIntent) -> String {
    let supplied = intent.alt_text.trim();
    if !supplied.is_empty() {
        return supplied.to_owned();
    }
    intent
        .source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("artifact preview")
        .to_owned()
}

/// Cheap change observation for steady-state frames.
///
/// This is deliberately not the security boundary: a changed source is opened
/// again through descriptor-relative no-follow traversal before any header or
/// pixel is read. The observation only avoids reparsing unchanged PNG headers.
fn observe_source(project_root: &Path, source: &Path) -> Result<SourceObservation, String> {
    validate_relative_png_path(source)?;
    let mut candidate = project_root.to_path_buf();
    for component in source.components() {
        let Component::Normal(part) = component else {
            return Err(format!(
                "artifact path must stay project-relative: {}",
                source.display()
            ));
        };
        candidate.push(part);
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!("artifact file is missing: {}", source.display())
            } else {
                format!("artifact file cannot be inspected: {}", source.display())
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "artifact path may not contain symlinks: {}",
                source.display()
            ));
        }
    }
    let metadata = fs::metadata(&candidate)
        .map_err(|_| format!("artifact file is missing: {}", source.display()))?;
    validate_encoded_metadata(source, &metadata)?;
    Ok(SourceObservation {
        encoded_len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn open_source(project_root: &Path, source: &Path) -> Result<OpenedSource, String> {
    validate_relative_png_path(source)?;
    let canonical_root = fs::canonicalize(project_root)
        .map_err(|_| "project directory is unavailable".to_owned())?;
    let file = open_no_follow(&canonical_root, source)?;
    let metadata = file
        .metadata()
        .map_err(|_| format!("artifact file cannot be inspected: {}", source.display()))?;
    validate_encoded_metadata(source, &metadata)?;
    Ok(OpenedSource {
        file,
        observation: SourceObservation {
            encoded_len: metadata.len(),
            modified: metadata.modified().ok(),
        },
    })
}

/// Open each path component relative to an already-opened project directory.
///
/// `O_NOFOLLOW` on every hop makes the symlink policy and containment check
/// part of the open itself, so a path replacement cannot race validation and
/// redirect the decoder outside the project.
#[cfg(unix)]
fn open_no_follow(project_root: &Path, source: &Path) -> Result<File, String> {
    use rustix::fs::{Mode, OFlags, open, openat};

    let root = open(
        project_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| "project directory is unavailable".to_owned())?;
    let components = source
        .components()
        .map(|component| match component {
            Component::Normal(part) => Ok(part),
            _ => Err(format!(
                "artifact path must stay project-relative: {}",
                source.display()
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Some((file_name, directories)) = components.split_last() else {
        return Err(format!(
            "artifact path must stay project-relative: {}",
            source.display()
        ));
    };

    let mut directory = root;
    for component in directories {
        directory = openat(
            &directory,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| open_error(source, error, true))?;
    }
    let file = openat(
        &directory,
        *file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| open_error(source, error, false))?;
    Ok(File::from(file))
}

#[cfg(not(unix))]
fn open_no_follow(_project_root: &Path, _source: &Path) -> Result<File, String> {
    Err("artifact preview requires symlink-safe macOS or Linux file APIs".to_owned())
}

#[cfg(unix)]
fn open_error(source: &Path, error: rustix::io::Errno, directory: bool) -> String {
    use rustix::io::Errno;

    if error == Errno::NOENT {
        return format!("artifact file is missing: {}", source.display());
    }
    if error == Errno::LOOP || (directory && error == Errno::NOTDIR) {
        return format!(
            "artifact path may not contain symlinks: {}",
            source.display()
        );
    }
    format!(
        "artifact file cannot be opened safely: {}",
        source.display()
    )
}

fn validate_relative_png_path(source: &Path) -> Result<(), String> {
    if source.as_os_str().is_empty()
        || source.is_absolute()
        || source
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "artifact path must stay project-relative: {}",
            source.display()
        ));
    }
    let png = source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !png {
        return Err(format!(
            "artifact preview supports PNG files only: {}",
            source.display()
        ));
    }
    Ok(())
}

fn validate_encoded_metadata(source: &Path, metadata: &Metadata) -> Result<(), String> {
    if !metadata.is_file() {
        return Err(format!(
            "artifact source is not a regular file: {}",
            source.display()
        ));
    }
    if metadata.len() > MAX_ARTIFACT_ENCODED_BYTES {
        return Err(format!(
            "artifact file exceeds the 16 MiB limit: {}",
            source.display()
        ));
    }
    Ok(())
}

fn inspect_png_header(file: &mut File) -> Result<usize, String> {
    let mut decoder = Decoder::new(BufReader::new(file));
    decoder.set_transformations(Transformations::normalize_to_color8() | Transformations::ALPHA);
    decoder.set_limits(Limits {
        bytes: MAX_ARTIFACT_DECODED_BYTES,
    });
    let reader = decoder
        .read_info()
        .map_err(|error| format!("artifact PNG is malformed: {error}"))?;
    if reader.info().animation_control.is_some() {
        return Err("animated PNG previews are not supported".to_owned());
    }
    let (width, height) = reader.info().size();
    let decoded_bytes = validate_dimensions(width, height)?;
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| "artifact decoded size overflows".to_owned())?;
    if output_size > MAX_ARTIFACT_DECODED_BYTES {
        return Err("artifact decoded pixels exceed the 64 MiB limit".to_owned());
    }
    Ok(decoded_bytes)
}

fn decode_png(file: File, expected_rgba_bytes: usize) -> Result<DecodedArtifact, String> {
    let mut encoded = Vec::new();
    file.take(MAX_ARTIFACT_ENCODED_BYTES + 1)
        .read_to_end(&mut encoded)
        .map_err(|_| "artifact file could not be read".to_owned())?;
    if encoded.len() as u64 > MAX_ARTIFACT_ENCODED_BYTES {
        return Err("artifact file exceeds the 16 MiB limit".to_owned());
    }

    let mut decoder = Decoder::new(Cursor::new(encoded));
    decoder.set_transformations(Transformations::normalize_to_color8() | Transformations::ALPHA);
    decoder.set_limits(Limits {
        bytes: MAX_ARTIFACT_DECODED_BYTES,
    });
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("artifact PNG is malformed: {error}"))?;
    if reader.info().animation_control.is_some() {
        return Err("animated PNG previews are not supported".to_owned());
    }
    let (width, height) = reader.info().size();
    if validate_dimensions(width, height)? != expected_rgba_bytes {
        return Err("artifact decoded size changed during load".to_owned());
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| "artifact decoded size overflows".to_owned())?;
    if output_size > MAX_ARTIFACT_DECODED_BYTES {
        return Err("artifact decoded pixels exceed the 64 MiB limit".to_owned());
    }
    // Allocate the final RGBA admission once. Formats that decode to fewer
    // channels expand backward in this same buffer, so conversion does not
    // transiently double the admitted decoded memory.
    let mut decoded = vec![0; expected_rgba_bytes];
    let output = reader
        .next_frame(&mut decoded)
        .map_err(|error| format!("artifact PNG is malformed: {error}"))?;
    decoded.truncate(output.buffer_size());
    let rgba8 = convert_to_rgba8(output.color_type, output.bit_depth, decoded)?;
    if rgba8.len() != expected_rgba_bytes {
        return Err("artifact decoded size changed during load".to_owned());
    }
    Ok(DecodedArtifact {
        width: output.width,
        height: output.height,
        rgba8,
    })
}

fn validate_dimensions(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("artifact dimensions must be non-zero".to_owned());
    }
    if width > MAX_ARTIFACT_DIMENSION || height > MAX_ARTIFACT_DIMENSION {
        return Err(format!(
            "artifact dimensions exceed 4096x4096: {width}x{height}"
        ));
    }
    let decoded = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "artifact decoded size overflows".to_owned())?;
    if decoded > MAX_ARTIFACT_DECODED_BYTES {
        return Err("artifact decoded pixels exceed the 64 MiB limit".to_owned());
    }
    Ok(decoded)
}

fn convert_to_rgba8(
    color_type: ColorType,
    bit_depth: BitDepth,
    mut decoded: Vec<u8>,
) -> Result<Vec<u8>, String> {
    if bit_depth != BitDepth::Eight {
        return Err("artifact PNG could not be normalized to 8-bit color".to_owned());
    }
    let samples = color_type.samples();
    if !decoded.len().is_multiple_of(samples) {
        return Err("artifact PNG produced an incomplete pixel".to_owned());
    }
    let pixels = decoded.len() / samples;
    let rgba_len = pixels
        .checked_mul(4)
        .ok_or_else(|| "artifact decoded size overflows".to_owned())?;
    decoded.resize(rgba_len, 0);
    match color_type {
        ColorType::Grayscale => {
            for pixel in (0..pixels).rev() {
                let gray = decoded[pixel];
                decoded[pixel * 4..pixel * 4 + 4].copy_from_slice(&[gray, gray, gray, 255]);
            }
        }
        ColorType::Rgb => {
            for pixel in (0..pixels).rev() {
                let source = pixel * 3;
                let red = decoded[source];
                let green = decoded[source + 1];
                let blue = decoded[source + 2];
                decoded[pixel * 4..pixel * 4 + 4].copy_from_slice(&[red, green, blue, 255]);
            }
        }
        ColorType::GrayscaleAlpha => {
            for pixel in (0..pixels).rev() {
                let source = pixel * 2;
                let gray = decoded[source];
                let alpha = decoded[source + 1];
                decoded[pixel * 4..pixel * 4 + 4].copy_from_slice(&[gray, gray, gray, alpha]);
            }
        }
        ColorType::Rgba => {}
        ColorType::Indexed => {
            return Err("artifact PNG palette was not expanded".to_owned());
        }
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        sync::mpsc,
        time::Duration,
    };

    use mandatum_commands::CommandId;
    use mandatum_core::{ArtifactFit, CoreAction};
    use mandatum_scene::{PaneContent, SceneSize, input::Key};

    use super::*;
    use crate::{AppConfig, AppState};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestProject {
        path: PathBuf,
    }

    impl TestProject {
        fn new() -> Self {
            let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mandatum-artifact-preview-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn workspace(&self, source: &str, alt_text: &str) -> Workspace {
            let mut workspace = Workspace::new("test", self.path.clone());
            workspace
                .apply_action(CoreAction::CreateArtifactPane {
                    intent: ArtifactPaneIntent {
                        source: PathBuf::from(source),
                        title: "artifact".to_owned(),
                        alt_text: alt_text.to_owned(),
                        fit: ArtifactFit::Contain,
                    },
                })
                .unwrap();
            workspace
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(rgba)
            .unwrap();
    }

    fn write_apng(path: &Path) {
        let file = File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, 1, 1);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_animated(2, 0).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0, 255]).unwrap();
        writer.write_image_data(&[0, 0, 255, 255]).unwrap();
    }

    fn ready(store: &ArtifactPreviewStore, workspace: &Workspace) -> RasterSurface {
        let pane_id = workspace.active_session().focused_pane_id();
        let PaneKind::Artifact { intent } =
            workspace.active_session().pane(pane_id).unwrap().kind()
        else {
            panic!("focused pane should be artifact");
        };
        let content = store.content(workspace, pane_id, intent);
        let ArtifactState::Ready(surface) = content.state else {
            panic!("artifact should be ready: {:?}", content.state);
        };
        surface
    }

    fn sender() -> (AppEventSender, mpsc::Receiver<AppEvent>) {
        let (tx, rx) = mpsc::channel();
        (AppEventSender::new(tx), rx)
    }

    fn apply_next_load(
        store: &mut ArtifactPreviewStore,
        sender: &AppEventSender,
        rx: &mpsc::Receiver<AppEvent>,
    ) {
        let event = sender
            .recv_timeout(rx, Duration::from_secs(2))
            .expect("artifact worker should report");
        let AppEvent::Artifact(event) = event else {
            panic!("unexpected app event");
        };
        assert!(store.apply_load_event(event));
    }

    #[test]
    fn valid_png_loads_and_reload_replaces_pixels_and_advances_revision() {
        let project = TestProject::new();
        let path = project.path.join("preview.png");
        write_png(&path, 1, 1, &[1, 2, 3, 255]);
        let workspace = project.workspace("preview.png", "");
        let mut store = ArtifactPreviewStore::default();
        let (sender, rx) = sender();

        store.refresh_active(&workspace, &sender);
        apply_next_load(&mut store, &sender, &rx);
        let first = ready(&store, &workspace);
        assert_eq!((first.width, first.height), (1, 1));
        assert_eq!(first.rgba8.as_ref(), &[1, 2, 3, 255]);
        assert_eq!(first.revision, 1);

        write_png(&path, 2, 1, &[9, 8, 7, 255, 6, 5, 4, 255]);
        let pane_id = workspace.active_session().focused_pane_id().clone();
        store.force_reload_active(&workspace, &pane_id, &sender);
        apply_next_load(&mut store, &sender, &rx);
        let second = ready(&store, &workspace);
        assert_eq!((second.width, second.height), (2, 1));
        assert_eq!(second.rgba8.as_ref(), &[9, 8, 7, 255, 6, 5, 4, 255]);
        assert!(second.revision > first.revision);
    }

    #[test]
    fn traversal_absolute_missing_oversized_and_malformed_inputs_fail_visibly() {
        let project = TestProject::new();
        fs::write(project.path.join("broken.png"), b"not a png").unwrap();
        write_apng(&project.path.join("animated.png"));
        let oversized = project.path.join("large.png");
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_ARTIFACT_ENCODED_BYTES + 1)
            .unwrap();

        for source in [
            "../outside.png",
            "/tmp/outside.png",
            "missing.png",
            "large.png",
            "broken.png",
            "animated.png",
            "preview.jpg",
        ] {
            let workspace = project.workspace(source, "preview");
            let mut store = ArtifactPreviewStore::default();
            let (sender, rx) = sender();
            store.refresh_active(&workspace, &sender);
            if let Ok(AppEvent::Artifact(event)) =
                sender.recv_timeout(&rx, Duration::from_millis(250))
            {
                assert!(store.apply_load_event(event));
            }
            let pane_id = workspace.active_session().focused_pane_id();
            let PaneKind::Artifact { intent } =
                workspace.active_session().pane(pane_id).unwrap().kind()
            else {
                unreachable!();
            };
            assert!(
                matches!(
                    store.content(&workspace, pane_id, intent).state,
                    ArtifactState::Failed { .. }
                ),
                "{source} should fail visibly"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_file_and_symlinked_ancestor_are_rejected() {
        use std::os::unix::fs::symlink;

        let project = TestProject::new();
        let outside = TestProject::new();
        write_png(&outside.path.join("outside.png"), 1, 1, &[1, 2, 3, 255]);
        symlink(
            outside.path.join("outside.png"),
            project.path.join("linked.png"),
        )
        .unwrap();
        symlink(&outside.path, project.path.join("linked-dir")).unwrap();

        for source in ["linked.png", "linked-dir/outside.png"] {
            let workspace = project.workspace(source, "preview");
            let mut store = ArtifactPreviewStore::default();
            let (sender, _rx) = sender();
            store.refresh_active(&workspace, &sender);
            let pane_id = workspace.active_session().focused_pane_id();
            let PaneKind::Artifact { intent } =
                workspace.active_session().pane(pane_id).unwrap().kind()
            else {
                unreachable!();
            };
            let ArtifactState::Failed { message } =
                store.content(&workspace, pane_id, intent).state
            else {
                panic!("{source} should fail");
            };
            assert!(message.contains("symlink"), "{message}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn opened_descriptor_cannot_be_redirected_by_a_later_path_swap() {
        use std::os::unix::fs::symlink;

        let project = TestProject::new();
        let outside = TestProject::new();
        let source = project.path.join("preview.png");
        write_png(&source, 1, 1, &[1, 2, 3, 255]);
        write_png(&outside.path.join("outside.png"), 1, 1, &[9, 8, 7, 255]);

        let mut opened = open_source(&project.path, Path::new("preview.png")).unwrap();
        let decoded_bytes = inspect_png_header(&mut opened.file).unwrap();
        opened.file.seek(SeekFrom::Start(0)).unwrap();
        fs::rename(&source, project.path.join("original.png")).unwrap();
        symlink(outside.path.join("outside.png"), &source).unwrap();

        let decoded = decode_png(opened.file, decoded_bytes).unwrap();
        assert_eq!(decoded.rgba8, [1, 2, 3, 255]);
    }

    #[test]
    fn restored_workspace_decode_fanout_and_aggregate_bytes_are_bounded() {
        let project = TestProject::new();
        let mut workspace = project.workspace("preview-0.png", "preview zero");
        write_png(&project.path.join("preview-0.png"), 1, 1, &[0, 0, 0, 255]);
        for index in 1..8 {
            let source = format!("preview-{index}.png");
            write_png(&project.path.join(&source), 1, 1, &[index as u8, 0, 0, 255]);
            workspace
                .apply_action(CoreAction::CreateArtifactPane {
                    intent: ArtifactPaneIntent {
                        source: PathBuf::from(source),
                        title: format!("preview {index}"),
                        alt_text: format!("preview {index}"),
                        fit: ArtifactFit::Contain,
                    },
                })
                .unwrap();
        }
        let restored = Workspace::from_json(&workspace.to_json().unwrap()).unwrap();
        let mut store = ArtifactPreviewStore::default();
        let (sender, _rx) = sender();
        store.refresh_active(&restored, &sender);
        assert_eq!(
            store.active_reservations.len(),
            MAX_CONCURRENT_ARTIFACT_LOADS
        );
        assert_eq!(store.pending.len(), 8 - MAX_CONCURRENT_ARTIFACT_LOADS);
        assert_eq!(store.workers.len(), MAX_CONCURRENT_ARTIFACT_LOADS);
        assert_eq!(store.aggregate_decoded_bytes(), 8 * 4);
        store.shutdown();

        let three = {
            let mut workspace = project.workspace("preview-0.png", "first");
            for (source, title) in [("preview-1.png", "second"), ("preview-2.png", "third")] {
                workspace
                    .apply_action(CoreAction::CreateArtifactPane {
                        intent: ArtifactPaneIntent {
                            source: PathBuf::from(source),
                            title: title.to_owned(),
                            alt_text: title.to_owned(),
                            fit: ArtifactFit::Contain,
                        },
                    })
                    .unwrap();
            }
            workspace
        };
        let mut aggregate_store = ArtifactPreviewStore::default();
        let (first_pane, first_intent) = three
            .active_session()
            .panes()
            .iter()
            .find_map(|(pane_id, pane)| match pane.kind() {
                PaneKind::Artifact { intent } => Some((pane_id.clone(), intent.clone())),
                _ => None,
            })
            .unwrap();
        let first_observation = open_source(&project.path, &first_intent.source)
            .unwrap()
            .observation;
        aggregate_store.entries.insert(
            (three.active_session().id().clone(), first_pane),
            CachedArtifact {
                project_root: project.path.clone(),
                source: first_intent.source,
                observation: Ok(first_observation),
                state: ArtifactState::Loading,
                load_token: 99,
                retry_when_capacity: false,
            },
        );
        aggregate_store
            .active_reservations
            .insert(99, MAX_ARTIFACT_DECODED_BYTES - 4);
        aggregate_store.refresh_active(&three, &sender);
        assert_eq!(
            aggregate_store.aggregate_decoded_bytes(),
            MAX_ARTIFACT_DECODED_BYTES
        );
        assert_eq!(
            aggregate_store
                .entries
                .values()
                .filter(|entry| matches!(entry.state, ArtifactState::Failed { .. }))
                .count(),
            1
        );
        aggregate_store.shutdown();

        let mut capped = project.workspace("preview-0.png", "preview zero");
        for index in 1..=MAX_ARTIFACT_PANES {
            capped
                .apply_action(CoreAction::CreateArtifactPane {
                    intent: ArtifactPaneIntent {
                        source: PathBuf::from("preview-0.png"),
                        title: format!("preview {index}"),
                        alt_text: format!("preview {index}"),
                        fit: ArtifactFit::Contain,
                    },
                })
                .unwrap();
        }
        let mut capped_store = ArtifactPreviewStore::default();
        capped_store.refresh_active(&capped, &sender);
        assert_eq!(
            capped_store.active_reservations.len() + capped_store.pending.len(),
            MAX_ARTIFACT_PANES
        );
        assert_eq!(
            capped_store
                .entries
                .values()
                .filter(|entry| matches!(entry.state, ArtifactState::Failed { .. }))
                .count(),
            1
        );
        capped_store.shutdown();
    }

    #[test]
    fn dimension_and_decoded_caps_are_checked_before_allocation() {
        assert_eq!(
            validate_dimensions(4_096, 4_096).unwrap(),
            MAX_ARTIFACT_DECODED_BYTES
        );
        assert!(validate_dimensions(4_097, 1).is_err());
        assert!(validate_dimensions(1, 4_097).is_err());
        assert!(validate_dimensions(0, 1).is_err());
    }

    #[test]
    fn palette_prompt_opens_through_app_input_then_async_load_reaches_the_scene() {
        let project = TestProject::new();
        write_png(
            &project.path.join("preview.png"),
            2,
            1,
            &[255, 0, 0, 255, 0, 0, 255, 255],
        );
        let mut state = AppState::new(AppConfig {
            project_path: project.path.clone(),
            workspace_file: project.path.join(".mandatum/workspace.json"),
            ..AppConfig::default()
        });

        state.dispatch(CommandId::OpenArtifactPreview);
        state.handle_event(mandatum_scene::input::InputEvent::Paste(
            "preview.png".to_owned(),
        ));
        state.handle_event(mandatum_scene::input::InputEvent::Key(Key::plain(
            mandatum_scene::input::KeyCode::Enter,
        )));

        let loading = state.build_scene(SceneSize::new(80, 24));
        assert!(matches!(
            loading.panes.last().map(|pane| &pane.content),
            Some(PaneContent::Artifact(ArtifactContent {
                state: ArtifactState::Loading,
                ..
            }))
        ));
        assert!(state.wait_event(Duration::from_secs(2)));
        let ready = state.build_scene(SceneSize::new(80, 24));
        let Some(PaneContent::Artifact(content)) = ready.panes.last().map(|pane| &pane.content)
        else {
            panic!("artifact pane should reach the scene");
        };
        let ArtifactState::Ready(surface) = &content.state else {
            panic!("artifact should be ready: {:?}", content.state);
        };
        assert_eq!((surface.width, surface.height), (2, 1));
        assert_eq!(surface.rgba8.len(), 8);
        let first_revision = surface.revision;
        assert_eq!(content.source_label, "preview.png");

        write_png(
            &project.path.join("preview.png"),
            1,
            2,
            &[0, 255, 0, 255, 255, 255, 0, 255],
        );
        state.dispatch(CommandId::RestartPane);
        let reloading = state.build_scene(SceneSize::new(80, 24));
        assert!(matches!(
            reloading.panes.last().map(|pane| &pane.content),
            Some(PaneContent::Artifact(ArtifactContent {
                state: ArtifactState::Loading,
                ..
            }))
        ));
        assert!(state.wait_event(Duration::from_secs(2)));
        let reloaded = state.build_scene(SceneSize::new(80, 24));
        let Some(PaneContent::Artifact(content)) = reloaded.panes.last().map(|pane| &pane.content)
        else {
            panic!("artifact pane should remain in the scene");
        };
        let ArtifactState::Ready(surface) = &content.state else {
            panic!("reloaded artifact should be ready");
        };
        assert_eq!((surface.width, surface.height), (1, 2));
        assert!(surface.revision > first_revision);
        state.shutdown();
    }

    #[test]
    fn completion_for_a_cleared_workspace_releases_its_stale_reservation() {
        let project = TestProject::new();
        write_png(&project.path.join("preview.png"), 1, 1, &[1, 2, 3, 255]);
        let workspace = project.workspace("preview.png", "preview");
        let mut store = ArtifactPreviewStore::default();
        let (sender, rx) = sender();
        store.refresh_active(&workspace, &sender);
        let event = sender
            .recv_timeout(&rx, Duration::from_secs(2))
            .expect("worker should report");
        let AppEvent::Artifact(event) = event else {
            panic!("unexpected event");
        };

        store.clear();
        assert!(store.apply_load_event(event));
        assert!(store.active_reservations.is_empty());
        store.shutdown();
    }
}
