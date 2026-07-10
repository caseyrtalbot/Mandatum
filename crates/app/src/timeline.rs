//! The execution timeline: an append-only JSONL log of durable workstation
//! facts at `<project>/.mandatum/timeline.jsonl`. The overlay model that
//! reads it back lives in `crate::timeline_view`.
//!
//! # Durable format
//!
//! One JSON object per line: `{"at_ms": <unix epoch millis>, "event":
//! "<kind>", ...fields}`. Events hold plain strings and numbers only — pane
//! ids, command text, verdicts, paths. Live handles, tokens, and socket
//! paths cannot appear because [`TimelineEventKind`] is a separate serde
//! type built from copies of durable facts (the L3 discipline, by
//! construction).
//!
//! # Write discipline
//!
//! Appends use an `O_APPEND` write of one complete line. This is the
//! documented deviation from the temp+fsync+rename convention in
//! `persistence.rs`: the log is a single-writer audit trail, a complete-line
//! append never corrupts previous lines, and a torn final line is tolerated
//! by the reader (skipped and counted, even when torn mid multi-byte
//! character) and healed by the writer (an append onto a file whose last
//! byte is not a newline starts on a fresh line, so a torn tail costs
//! exactly one counted line, never a merged event). Per-line fsync is
//! deliberately skipped for throughput; the workspace file remains the
//! durable source of truth for intent.
//!
//! # Rotation
//!
//! Before an append, a file at or over [`ROTATE_BYTES`] is renamed to
//! `timeline.1.jsonl` (replacing any previous rotation) and a fresh file
//! starts. At most two files ever exist. The reader stitches the rotated
//! tail in when the active file is short.

use std::{
    fs,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Rotation threshold for the active log file.
pub(crate) const ROTATE_BYTES: u64 = 2 * 1024 * 1024;
/// Hard cap a reader will load from one file (rotation keeps files well
/// under this; a larger file is reported, never loaded).
const READ_CAP_BYTES: u64 = 4 * 1024 * 1024;
/// How many events the overlay reads back.
pub(crate) const TAIL_EVENTS: usize = 500;

/// One durable timeline fact with its timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TimelineEvent {
    /// Unix epoch milliseconds.
    pub(crate) at_ms: u64,
    #[serde(flatten)]
    pub(crate) kind: TimelineEventKind,
}

/// The durable fact kinds the timeline records. Plain data only (L3): no
/// runtime-only field exists on these types, so serialization cannot leak
/// handles, tokens, or socket paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum TimelineEventKind {
    CommandDispatched {
        command: String,
        pane: Option<String>,
    },
    TaskStarted {
        pane: String,
        command: String,
    },
    TaskExited {
        pane: String,
        command: String,
        exit: String,
    },
    AgentStatus {
        pane: String,
        status: String,
    },
    ApprovalRequested {
        pane: String,
        command: String,
        scope: String,
        risk: String,
    },
    ApprovalDecided {
        pane: String,
        command: String,
        verdict: String,
        decided_by: String,
    },
    AgentObjectiveSet {
        pane: String,
        objective: String,
    },
    AgentLaunchRefused {
        pane: String,
        reason: String,
    },
    WorkspaceSaved {
        path: String,
    },
    WorkspaceRestored {
        path: String,
    },
    PaneCreated {
        pane: String,
        kind: String,
    },
    PaneClosed {
        pane: String,
    },
    ConfigReloaded {
        warnings: usize,
    },
}

impl TimelineEventKind {
    /// The pane this event names, if any (the overlay's jump target).
    pub(crate) fn pane(&self) -> Option<&str> {
        match self {
            Self::CommandDispatched { pane, .. } => pane.as_deref(),
            Self::TaskStarted { pane, .. }
            | Self::TaskExited { pane, .. }
            | Self::AgentStatus { pane, .. }
            | Self::ApprovalRequested { pane, .. }
            | Self::ApprovalDecided { pane, .. }
            | Self::AgentObjectiveSet { pane, .. }
            | Self::AgentLaunchRefused { pane, .. }
            | Self::PaneCreated { pane, .. }
            | Self::PaneClosed { pane } => Some(pane),
            Self::WorkspaceSaved { .. }
            | Self::WorkspaceRestored { .. }
            | Self::ConfigReloaded { .. } => None,
        }
    }

    /// Coarse family label for the `kind:` filter prefix.
    pub(crate) fn kind_label(&self) -> &'static str {
        match self {
            Self::CommandDispatched { .. } => "command",
            Self::TaskStarted { .. } | Self::TaskExited { .. } => "task",
            Self::AgentStatus { .. }
            | Self::AgentObjectiveSet { .. }
            | Self::AgentLaunchRefused { .. } => "agent",
            Self::ApprovalRequested { .. } | Self::ApprovalDecided { .. } => "approval",
            Self::WorkspaceSaved { .. } | Self::WorkspaceRestored { .. } => "workspace",
            Self::PaneCreated { .. } | Self::PaneClosed { .. } => "pane",
            Self::ConfigReloaded { .. } => "config",
        }
    }

    /// One-cell glyph for the overlay row.
    pub(crate) fn glyph(&self) -> &'static str {
        match self {
            Self::CommandDispatched { .. } => "»",
            Self::TaskStarted { .. } => "▶",
            Self::TaskExited { exit, .. } => {
                if exit.starts_with("succeeded") {
                    "✓"
                } else {
                    "✗"
                }
            }
            Self::AgentStatus { status, .. } => match status.as_str() {
                "failed" => "✗",
                "complete" => "✓",
                _ => "◆",
            },
            Self::ApprovalRequested { .. } => "?",
            Self::ApprovalDecided { verdict, .. } => {
                if verdict == "approved" {
                    "✓"
                } else {
                    "✗"
                }
            }
            Self::AgentObjectiveSet { .. } => "◆",
            Self::AgentLaunchRefused { .. } => "!",
            Self::WorkspaceSaved { .. } => "▪",
            Self::WorkspaceRestored { .. } => "⟲",
            Self::PaneCreated { .. } => "+",
            Self::PaneClosed { .. } => "×",
            Self::ConfigReloaded { .. } => "⟳",
        }
    }

    /// Human description of the fact, used for display and text filtering.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::CommandDispatched { command, pane } => match pane {
                Some(pane) => format!("dispatched {command} ({pane})"),
                None => format!("dispatched {command}"),
            },
            Self::TaskStarted { pane, command } => format!("task {pane} started: {command}"),
            Self::TaskExited {
                pane,
                command,
                exit,
            } => format!("task {pane} {exit}: {command}"),
            Self::AgentStatus { pane, status } => format!("agent {pane} is {status}"),
            Self::ApprovalRequested {
                pane,
                command,
                scope,
                risk,
            } => format!("agent {pane} requests approval: {command} (scope {scope}; risk {risk})"),
            Self::ApprovalDecided {
                pane,
                command,
                verdict,
                decided_by,
            } => format!("{verdict} '{command}' for {pane} (by {decided_by})"),
            Self::AgentObjectiveSet { pane, objective } => {
                format!("agent {pane} objective set: {objective}")
            }
            Self::AgentLaunchRefused { pane, reason } => {
                format!("agent launch refused for {pane}: {reason}")
            }
            Self::WorkspaceSaved { path } => format!("workspace saved to {path}"),
            Self::WorkspaceRestored { path } => format!("workspace restored from {path}"),
            Self::PaneCreated { pane, kind } => format!("pane {pane} created ({kind})"),
            Self::PaneClosed { pane } => format!("pane {pane} closed"),
            Self::ConfigReloaded { warnings } => {
                if *warnings == 0 {
                    "config reloaded".to_owned()
                } else {
                    format!("config reloaded with {warnings} warning(s)")
                }
            }
        }
    }
}

/// The append side of the timeline. `file: None` disables recording (test
/// baselines without a workspace directory).
pub(crate) struct TimelineLog {
    file: Option<PathBuf>,
    rotate_bytes: u64,
    /// The most recent append/read failure, surfaced in the overlay footer.
    pub(crate) last_error: Option<String>,
}

impl TimelineLog {
    pub(crate) fn new(file: Option<PathBuf>) -> Self {
        Self {
            file,
            rotate_bytes: ROTATE_BYTES,
            last_error: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_rotate_bytes(file: PathBuf, rotate_bytes: u64) -> Self {
        Self {
            file: Some(file),
            rotate_bytes,
            last_error: None,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.file.is_some()
    }

    /// Append one fact, stamped now. Failures never interrupt the app; the
    /// last error is kept for the overlay to show.
    pub(crate) fn record(&mut self, kind: TimelineEventKind) {
        let Some(file) = self.file.clone() else {
            return;
        };
        let event = TimelineEvent {
            at_ms: now_ms(),
            kind,
        };
        match append_event(&file, self.rotate_bytes, &event) {
            Ok(()) => {}
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }

    /// Read the last [`TAIL_EVENTS`] events, oldest first, stitching in the
    /// rotated file when the active one is short. Malformed lines are
    /// skipped and counted, never a crash.
    pub(crate) fn read_tail(&self) -> TimelineTail {
        let Some(file) = self.file.as_deref() else {
            return TimelineTail {
                events: Vec::new(),
                malformed: 0,
                error: Some("timeline disabled: no workspace directory".to_owned()),
            };
        };

        let mut malformed = 0;
        let mut error = self.last_error.clone();
        let mut events = match read_events(file) {
            Ok((events, bad)) => {
                malformed += bad;
                events
            }
            Err(read_error) => {
                error = Some(read_error.to_string());
                Vec::new()
            }
        };

        if events.len() < TAIL_EVENTS {
            let rotated = rotated_path(file);
            match read_events(&rotated) {
                Ok((mut older, bad)) => {
                    malformed += bad;
                    older.extend(events);
                    events = older;
                }
                Err(TimelineFileError::Io { source, .. })
                    if source.kind() == io::ErrorKind::NotFound => {}
                Err(read_error) => {
                    error = Some(read_error.to_string());
                }
            }
        }

        let skip = events.len().saturating_sub(TAIL_EVENTS);
        TimelineTail {
            events: events.split_off(skip),
            malformed,
            error,
        }
    }
}

/// What a tail read produced.
pub(crate) struct TimelineTail {
    /// Oldest first.
    pub(crate) events: Vec<TimelineEvent>,
    pub(crate) malformed: usize,
    pub(crate) error: Option<String>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn rotated_path(file: &Path) -> PathBuf {
    file.with_file_name("timeline.1.jsonl")
}

#[derive(Debug)]
enum TimelineFileError {
    Io { path: PathBuf, source: io::Error },
    UnsafePath { path: PathBuf, message: String },
    Encode { source: serde_json::Error },
}

impl std::fmt::Display for TimelineFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::UnsafePath { path, message } => {
                write!(formatter, "{}: {message}", path.display())
            }
            Self::Encode { source } => write!(formatter, "timeline encode failed: {source}"),
        }
    }
}

/// Reject symlinks and non-regular files, mirroring `persistence.rs`.
fn reject_unsafe_timeline_file(path: &Path) -> Result<Option<u64>, TimelineFileError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(TimelineFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "timeline file must not be a symlink".to_owned(),
        }),
        Ok(metadata) if !metadata.is_file() => Err(TimelineFileError::UnsafePath {
            path: path.to_path_buf(),
            message: "timeline path is not a regular file".to_owned(),
        }),
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(TimelineFileError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn append_event(
    file: &Path,
    rotate_bytes: u64,
    event: &TimelineEvent,
) -> Result<(), TimelineFileError> {
    crate::persistence::ensure_parent_dir(file).map_err(|error| TimelineFileError::UnsafePath {
        path: file.to_path_buf(),
        message: error.to_string(),
    })?;
    let existing_len = reject_unsafe_timeline_file(file)?;

    // Rotate before the append so the active file stays under the cap: the
    // full file replaces any previous rotation, keeping at most two files.
    let rotated_now = existing_len.is_some_and(|len| len >= rotate_bytes);
    if rotated_now {
        fs::rename(file, rotated_path(file)).map_err(|source| TimelineFileError::Io {
            path: file.to_path_buf(),
            source,
        })?;
    }

    // A crash mid-append leaves a torn final line with no trailing newline.
    // Starting the next event on a fresh line heals the tail: the torn
    // fragment stays one counted malformed line instead of swallowing this
    // event.
    let needs_leading_newline = match existing_len {
        Some(len) if !rotated_now && len > 0 => !ends_with_newline(file, len)?,
        _ => false,
    };

    let mut line =
        serde_json::to_string(event).map_err(|source| TimelineFileError::Encode { source })?;
    line.push('\n');
    if needs_leading_newline {
        line.insert(0, '\n');
    }

    let mut handle = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(file)
        .map_err(|source| TimelineFileError::Io {
            path: file.to_path_buf(),
            source,
        })?;
    handle
        .write_all(line.as_bytes())
        .map_err(|source| TimelineFileError::Io {
            path: file.to_path_buf(),
            source,
        })
}

/// Whether the file's last byte is `b'\n'` (a clean line ending). `len` is
/// the measured size and must be > 0.
fn ends_with_newline(file: &Path, len: u64) -> Result<bool, TimelineFileError> {
    let mut handle = fs::File::open(file).map_err(|source| TimelineFileError::Io {
        path: file.to_path_buf(),
        source,
    })?;
    handle
        .seek(io::SeekFrom::Start(len - 1))
        .map_err(|source| TimelineFileError::Io {
            path: file.to_path_buf(),
            source,
        })?;
    let mut last = [0u8; 1];
    match handle.read_exact(&mut last) {
        Ok(()) => Ok(last[0] == b'\n'),
        // The file shrank under us; nothing to heal.
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(true),
        Err(source) => Err(TimelineFileError::Io {
            path: file.to_path_buf(),
            source,
        }),
    }
}

/// Read and parse one log file: `(events oldest first, malformed count)`.
fn read_events(file: &Path) -> Result<(Vec<TimelineEvent>, usize), TimelineFileError> {
    let len = match reject_unsafe_timeline_file(file)? {
        Some(len) => len,
        None => {
            return Err(TimelineFileError::Io {
                path: file.to_path_buf(),
                source: io::Error::new(io::ErrorKind::NotFound, "no timeline file"),
            });
        }
    };
    if len > READ_CAP_BYTES {
        return Err(TimelineFileError::UnsafePath {
            path: file.to_path_buf(),
            message: format!("timeline file is too large: {len} byte(s), max {READ_CAP_BYTES}"),
        });
    }

    // Raw bytes, decoded per line: a line torn mid multi-byte character
    // costs exactly one counted malformed line, never the whole file (a
    // whole-file `read_to_string` would fail wholesale on invalid UTF-8).
    let mut bytes = Vec::new();
    let handle = fs::File::open(file).map_err(|source| TimelineFileError::Io {
        path: file.to_path_buf(),
        source,
    })?;
    handle
        .take(READ_CAP_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|source| TimelineFileError::Io {
            path: file.to_path_buf(),
            source,
        })?;

    let mut events = Vec::new();
    let mut malformed = 0;
    for raw_line in bytes.split(|&byte| byte == b'\n') {
        let Ok(line) = std::str::from_utf8(raw_line) else {
            malformed += 1;
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TimelineEvent>(line) {
            Ok(event) => events.push(event),
            Err(_) => malformed += 1,
        }
    }
    Ok((events, malformed))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mandatum-timeline-test-{}-{counter}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("test temp dir");
            Self { path }
        }

        fn file(&self) -> PathBuf {
            self.path.join(".mandatum").join("timeline.jsonl")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn task_exit(pane: &str, exit: &str) -> TimelineEventKind {
        TimelineEventKind::TaskExited {
            pane: pane.to_owned(),
            command: "cargo test".to_owned(),
            exit: exit.to_owned(),
        }
    }

    #[test]
    fn events_round_trip_through_the_jsonl_file() {
        let dir = TempDir::new();
        let mut log = TimelineLog::new(Some(dir.file()));

        log.record(TimelineEventKind::CommandDispatched {
            command: "split-right".to_owned(),
            pane: Some("pane-1".to_owned()),
        });
        log.record(task_exit("pane-2", "failed: exit 3"));
        assert!(log.last_error.is_none(), "{:?}", log.last_error);

        let tail = log.read_tail();
        assert_eq!(tail.malformed, 0);
        assert_eq!(tail.events.len(), 2);
        assert_eq!(
            tail.events[1].kind,
            task_exit("pane-2", "failed: exit 3"),
            "events read back oldest first"
        );
        assert!(tail.events[0].at_ms > 0);

        // The line format is self-describing JSON with a tag field.
        let raw = fs::read_to_string(dir.file()).unwrap();
        assert!(raw.lines().count() == 2);
        assert!(raw.contains(r#""event":"task_exited""#));
        assert!(raw.contains(r#""exit":"failed: exit 3""#));
    }

    // The durable log stores facts only: no live handle, token, or socket
    // field exists on the serialized types (L3 by construction).
    #[test]
    fn serialized_events_carry_no_runtime_fields() {
        let event = TimelineEvent {
            at_ms: 42,
            kind: TimelineEventKind::ApprovalRequested {
                pane: "pane-3".to_owned(),
                command: "rm -rf target".to_owned(),
                scope: "/tmp/project -> target".to_owned(),
                risk: "high (removes files (rm))".to_owned(),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        for forbidden in [
            "runtime_token",
            "restart_generation",
            "socket",
            "JoinHandle",
            "control",
            "thread",
        ] {
            assert!(!json.contains(forbidden), "{json}");
        }
        let back: TimelineEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn malformed_lines_are_skipped_and_counted_never_a_crash() {
        let dir = TempDir::new();
        let mut log = TimelineLog::new(Some(dir.file()));
        log.record(task_exit("pane-1", "succeeded: exit 0"));

        let mut raw = fs::read_to_string(dir.file()).unwrap();
        raw.push_str("{ not json\n");
        raw.push_str("{\"at_ms\":1,\"event\":\"not_a_known_kind\"}\n");
        fs::write(dir.file(), raw).unwrap();
        log.record(task_exit("pane-1", "failed: exit 2"));

        let tail = log.read_tail();
        assert_eq!(tail.malformed, 2);
        assert_eq!(tail.events.len(), 2);
        assert!(tail.error.is_none());
    }

    // A crash mid-append leaves a torn final line without a trailing
    // newline; the next append must start on a fresh line so its event is
    // never merged into the torn fragment.
    #[test]
    fn torn_final_line_does_not_swallow_the_next_event() {
        let dir = TempDir::new();
        let mut log = TimelineLog::new(Some(dir.file()));
        log.record(task_exit("pane-1", "succeeded: exit 0"));

        let mut raw = fs::read_to_string(dir.file()).unwrap();
        raw.push_str(r#"{"at_ms":1,"event":"pane_close"#); // torn, no newline
        fs::write(dir.file(), raw).unwrap();

        log.record(task_exit("pane-2", "failed: exit 1"));
        assert!(log.last_error.is_none(), "{:?}", log.last_error);

        let tail = log.read_tail();
        assert_eq!(tail.malformed, 1, "the torn fragment costs one line");
        assert_eq!(tail.events.len(), 2);
        assert_eq!(tail.events[1].kind, task_exit("pane-2", "failed: exit 1"));
    }

    // A final line torn inside a multi-byte character makes the tail
    // invalid UTF-8; that must cost one counted malformed line, not the
    // whole file, and the next append must still land readable.
    #[test]
    fn torn_multibyte_character_costs_one_line_not_the_file() {
        let dir = TempDir::new();
        let mut log = TimelineLog::new(Some(dir.file()));
        log.record(task_exit("pane-1", "succeeded: exit 0"));

        // 0xC3 without its continuation byte: 'é' cut in half.
        let mut raw = fs::read(dir.file()).unwrap();
        raw.extend_from_slice(b"{\"at_ms\":1,\"event\":\"agent_objective_set\",\"pane\":\"p\",\"objective\":\"caf\xC3");
        fs::write(dir.file(), raw).unwrap();

        let tail = log.read_tail();
        assert!(tail.error.is_none(), "{:?}", tail.error);
        assert_eq!(tail.malformed, 1);
        assert_eq!(tail.events.len(), 1, "valid preceding lines survive");

        log.record(task_exit("pane-2", "failed: exit 1"));
        assert!(log.last_error.is_none(), "{:?}", log.last_error);
        let tail = log.read_tail();
        assert_eq!(tail.malformed, 1);
        assert_eq!(tail.events.len(), 2);
        assert_eq!(tail.events[1].kind, task_exit("pane-2", "failed: exit 1"));
    }

    #[test]
    fn rotation_replaces_the_previous_rotation_and_starts_fresh() {
        let dir = TempDir::new();
        // A tiny cap so a handful of events crosses it.
        let mut log = TimelineLog::with_rotate_bytes(dir.file(), 256);
        for index in 0..12 {
            log.record(task_exit(&format!("pane-{index}"), "succeeded: exit 0"));
        }
        assert!(log.last_error.is_none(), "{:?}", log.last_error);

        let rotated = dir.file().with_file_name("timeline.1.jsonl");
        assert!(rotated.exists(), "rotation must produce timeline.1.jsonl");
        assert!(
            fs::metadata(dir.file()).unwrap().len() < 256 + 200,
            "the active file restarts near-empty after rotation"
        );

        // At most two files exist, so only the most recent window survives
        // repeated rotation — and the tail read stitches the rotated file
        // in front of the active one, newest events always intact.
        let tail = log.read_tail();
        assert!(!tail.events.is_empty());
        assert!(tail.events.len() < 12, "older rotations are dropped");
        assert_eq!(
            tail.events.last().unwrap().kind,
            task_exit("pane-11", "succeeded: exit 0")
        );
        // The surviving window is the contiguous most-recent suffix.
        let first_index = 12 - tail.events.len();
        assert_eq!(
            tail.events.first().unwrap().kind,
            task_exit(&format!("pane-{first_index}"), "succeeded: exit 0")
        );
    }

    #[test]
    fn tail_is_capped_at_the_configured_event_count() {
        let dir = TempDir::new();
        let mut log = TimelineLog::new(Some(dir.file()));
        for index in 0..(TAIL_EVENTS + 25) {
            log.record(TimelineEventKind::PaneClosed {
                pane: format!("pane-{index}"),
            });
        }
        let tail = log.read_tail();
        assert_eq!(tail.events.len(), TAIL_EVENTS);
        assert_eq!(
            tail.events[0].kind,
            TimelineEventKind::PaneClosed {
                pane: "pane-25".to_owned()
            },
            "the oldest overflow events fall off the tail"
        );
    }

    #[test]
    fn symlinked_timeline_files_are_rejected() {
        let dir = TempDir::new();
        let target = dir.path.join("elsewhere.jsonl");
        fs::write(&target, "").unwrap();
        fs::create_dir_all(dir.file().parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, dir.file()).unwrap();

        let mut log = TimelineLog::new(Some(dir.file()));
        log.record(task_exit("pane-1", "succeeded: exit 0"));
        assert!(
            log.last_error
                .as_deref()
                .is_some_and(|error| error.contains("symlink")),
            "{:?}",
            log.last_error
        );
        let tail = log.read_tail();
        assert!(tail.events.is_empty());
        assert!(tail.error.is_some());
    }

    #[test]
    fn disabled_log_records_nothing_and_reports_why() {
        let mut log = TimelineLog::new(None);
        log.record(task_exit("pane-1", "succeeded: exit 0"));
        assert!(log.last_error.is_none());
        let tail = log.read_tail();
        assert!(tail.events.is_empty());
        assert!(tail.error.as_deref().unwrap().contains("disabled"));
    }
}
