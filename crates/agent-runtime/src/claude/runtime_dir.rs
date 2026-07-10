//! Per-session runtime directory for the Claude CLI connector.
//!
//! Each launch gets `.mandatum/agent-runtime/<session-id>/` under the spec
//! cwd, holding the generated `settings.json` (PreToolUse approval hook) and,
//! when the path is short enough for a `sockaddr_un`, the approval socket.
//! `.mandatum/` is kept out of version control by writing
//! `.mandatum/.gitignore` on first use, mirroring how workspace.json
//! persistence treats `.mandatum` as untracked local state (and its
//! symlink-rejection convention from `crates/app/src/persistence.rs`).

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    connector::AgentConnectorError,
    spec::{ApprovalPolicy, ToolClass},
};

/// Unix domain socket paths must fit `sockaddr_un.sun_path` (104 bytes on
/// macOS including the NUL); beyond this we fall back to a short path.
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Fallback directory for approval sockets when the session directory path
/// is too long to bind (deep cwds, macOS temp dirs).
const SHORT_SOCKET_DIR: &str = "/tmp/mandatum-agent";

/// Tools the Claude CLI may use without an interactive permission prompt.
/// Gated classes stay in this list — the PreToolUse hook, not the allowlist,
/// is the enforcement point for them.
pub(crate) const ALLOWED_TOOLS: &str = "Read,Glob,Grep,Bash,Write,Edit,MultiEdit,NotebookEdit";

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Filesystem layout for one live session.
#[derive(Clone, Debug)]
pub(crate) struct SessionRuntimeDir {
    /// Where the approval listener binds; removed on shutdown.
    pub(crate) socket_path: PathBuf,
    /// The generated `--settings` file, inside
    /// `.mandatum/agent-runtime/<session-id>/`.
    pub(crate) settings_path: PathBuf,
}

/// Create the session runtime directory under `cwd` and pick a bindable
/// socket path.
pub(crate) fn prepare_session_dir(cwd: &Path) -> Result<SessionRuntimeDir, AgentConnectorError> {
    let session_id = generate_session_id();
    let mandatum_dir = cwd.join(".mandatum");
    create_dir_rejecting_symlinks(&mandatum_dir)?;
    ensure_gitignore(&mandatum_dir)?;
    let dir = mandatum_dir.join("agent-runtime").join(&session_id);
    fs::create_dir_all(&dir).map_err(|error| launch_error(&dir, &error))?;

    let colocated = dir.join("approval.sock");
    let socket_path = if colocated.as_os_str().len() <= MAX_SOCKET_PATH_BYTES {
        colocated
    } else {
        let short_dir = PathBuf::from(SHORT_SOCKET_DIR);
        create_private_dir(&short_dir)?;
        short_dir.join(format!("{session_id}.sock"))
    };

    Ok(SessionRuntimeDir {
        settings_path: dir.join("settings.json"),
        socket_path,
    })
}

/// Margin the bridge's own verdict timeout keeps under the hook timeout, so
/// the bridge always fails closed on its own clock before Claude's
/// hook-timeout policy (which Mandatum does not control) can kick in.
const BRIDGE_TIMEOUT_MARGIN_SECS: u64 = 30;

/// Write the Claude `--settings` file: a PreToolUse hook that runs the
/// approval bridge for every gated tool class. No gated classes, no hook.
pub(crate) fn write_settings_file(
    runtime: &SessionRuntimeDir,
    bridge_binary: &Path,
    policy: &ApprovalPolicy,
    hook_timeout_secs: u64,
) -> Result<(), AgentConnectorError> {
    let mut settings = serde_json::json!({});
    if let Some(matcher) = hook_matcher(policy) {
        // argv[2] bounds the bridge's verdict wait strictly under the hook
        // timeout; see `mandatum-approval-bridge`.
        let bridge_timeout_secs = hook_timeout_secs
            .saturating_sub(BRIDGE_TIMEOUT_MARGIN_SECS)
            .max(1);
        let command = format!(
            "{} {} {bridge_timeout_secs}",
            shell_quote(&bridge_binary.to_string_lossy()),
            shell_quote(&runtime.socket_path.to_string_lossy()),
        );
        settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": matcher,
                        "hooks": [
                            {
                                "type": "command",
                                "command": command,
                                "timeout": hook_timeout_secs,
                            }
                        ]
                    }
                ]
            }
        });
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|error| {
        AgentConnectorError::LaunchFailed {
            message: format!("failed to encode settings.json: {error}"),
        }
    })?;
    fs::write(&runtime.settings_path, json)
        .map_err(|error| launch_error(&runtime.settings_path, &error))
}

/// The PreToolUse matcher covering every gated tool class, or `None` when
/// nothing is gated.
pub(crate) fn hook_matcher(policy: &ApprovalPolicy) -> Option<String> {
    let mut names: Vec<&str> = Vec::new();
    for class in &policy.gated_classes {
        match class {
            ToolClass::ShellCommand => names.push("Bash"),
            ToolClass::FileWrite => {
                names.extend(["Write", "Edit", "MultiEdit", "NotebookEdit"]);
            }
            ToolClass::FileRead => names.extend(["Read", "Glob", "Grep"]),
        }
    }
    if names.is_empty() {
        None
    } else {
        Some(names.join("|"))
    }
}

/// Unique-enough session id: time, pid, and a process-wide counter. Short so
/// the colocated socket path stays bindable in most working directories.
fn generate_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:08x}{:04x}{:02x}",
        (nanos as u64) ^ ((nanos >> 64) as u64),
        std::process::id() as u16,
        counter as u8,
    )
}

/// `create_dir_all` with the persistence convention: an existing path must be
/// a real directory, never a symlink.
fn create_dir_rejecting_symlinks(dir: &Path) -> Result<(), AgentConnectorError> {
    match fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(AgentConnectorError::LaunchFailed {
                message: format!("{} must not be a symlink", dir.display()),
            })
        }
        Ok(metadata) if !metadata.is_dir() => Err(AgentConnectorError::LaunchFailed {
            message: format!("{} is not a directory", dir.display()),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(dir).map_err(|error| launch_error(dir, &error))
        }
        Err(error) => Err(launch_error(dir, &error)),
    }
}

/// Write `.mandatum/.gitignore` ignoring everything, if absent.
fn ensure_gitignore(mandatum_dir: &Path) -> Result<(), AgentConnectorError> {
    let gitignore = mandatum_dir.join(".gitignore");
    if gitignore.exists() {
        return Ok(());
    }
    fs::write(&gitignore, "*\n").map_err(|error| launch_error(&gitignore, &error))
}

/// Create the shared short socket directory, owner-only.
fn create_private_dir(dir: &Path) -> Result<(), AgentConnectorError> {
    use std::os::unix::fs::DirBuilderExt;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(launch_error(dir, &error)),
    }
}

/// Single-quote a string for use inside a hook shell command.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn launch_error(path: &Path, error: &io::Error) -> AgentConnectorError {
    AgentConnectorError::LaunchFailed {
        message: format!("{}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cwd(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mandatum-runtime-dir-{tag}-{}-{}",
            std::process::id(),
            SESSION_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prepare_creates_session_dir_and_gitignore_once() {
        let cwd = temp_cwd("prepare");
        let runtime = prepare_session_dir(&cwd).unwrap();
        let session_dir = runtime.settings_path.parent().unwrap();
        assert!(session_dir.starts_with(cwd.join(".mandatum/agent-runtime")));
        assert!(session_dir.is_dir());
        let gitignore = cwd.join(".mandatum/.gitignore");
        assert_eq!(fs::read_to_string(&gitignore).unwrap(), "*\n");

        // A pre-existing gitignore is left alone.
        fs::write(&gitignore, "workspace.json\n").unwrap();
        let second = prepare_session_dir(&cwd).unwrap();
        assert_ne!(second.settings_path, runtime.settings_path);
        assert_eq!(fs::read_to_string(&gitignore).unwrap(), "workspace.json\n");
        fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn deep_cwds_fall_back_to_a_short_socket_path() {
        let mut cwd = temp_cwd("deep");
        for _ in 0..6 {
            cwd = cwd.join("very-long-directory-segment-for-socket-paths");
        }
        fs::create_dir_all(&cwd).unwrap();
        let runtime = prepare_session_dir(&cwd).unwrap();
        assert!(runtime.socket_path.as_os_str().len() <= MAX_SOCKET_PATH_BYTES);
        assert!(runtime.socket_path.starts_with(SHORT_SOCKET_DIR));
    }

    #[test]
    fn matcher_covers_gated_classes_only() {
        assert_eq!(
            hook_matcher(&ApprovalPolicy::default()).as_deref(),
            Some("Bash")
        );
        assert_eq!(
            hook_matcher(&ApprovalPolicy {
                gated_classes: vec![ToolClass::ShellCommand, ToolClass::FileWrite],
            })
            .as_deref(),
            Some("Bash|Write|Edit|MultiEdit|NotebookEdit")
        );
        assert_eq!(
            hook_matcher(&ApprovalPolicy {
                gated_classes: vec![],
            }),
            None
        );
    }

    #[test]
    fn settings_file_embeds_the_bridge_command_and_timeout() {
        let cwd = temp_cwd("settings");
        let runtime = prepare_session_dir(&cwd).unwrap();
        write_settings_file(
            &runtime,
            Path::new("/opt/bin/mandatum-approval-bridge"),
            &ApprovalPolicy::default(),
            600,
        )
        .unwrap();
        let raw = fs::read_to_string(&runtime.settings_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hook = &value["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], "Bash");
        let command = hook["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains("mandatum-approval-bridge"));
        assert!(command.contains(runtime.socket_path.to_str().unwrap()));
        // The bridge's own verdict bound rides along as argv[2], strictly
        // under the hook timeout.
        assert!(command.ends_with(" 570"), "command was: {command}");
        assert_eq!(hook["hooks"][0]["timeout"], 600);
        fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn shell_quote_survives_embedded_single_quotes() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }
}
