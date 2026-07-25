//! Per-session runtime directory for the Claude CLI connector.
//!
//! Each launch gets an owner-only directory beneath
//! `/tmp/mandatum-agent-<euid>/`, holding the generated `settings.json`
//! (PreToolUse approval hook) and approval socket. Keeping this
//! security-sensitive state below a sticky system temporary directory and an
//! owner-only root avoids traversing project directories that may be shared or
//! writable by another local user.

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

/// Prefix for the private per-user runtime directory.
const SHORT_SOCKET_DIR_PREFIX: &str = "/tmp/mandatum-agent";

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
    /// The generated `--settings` file, inside the private session directory.
    pub(crate) settings_path: PathBuf,
}

/// Create an owner-only session directory below the sticky system temporary
/// directory. The project cwd is deliberately not part of this trust boundary.
pub(crate) fn prepare_session_dir(_cwd: &Path) -> Result<SessionRuntimeDir, AgentConnectorError> {
    let session_id = generate_session_id();
    let root = short_socket_dir();
    create_private_dir(&root)?;
    let dir = root.join(&session_id);
    create_private_dir(&dir)?;

    Ok(SessionRuntimeDir {
        settings_path: dir.join("settings.json"),
        socket_path: dir.join("approval.sock"),
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
            "{} {} {bridge_timeout_secs} || {{ \
             printf '%s\\n' 'Mandatum approval bridge failed; action denied' >&2; \
             exit 2; }}",
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

fn short_socket_dir() -> PathBuf {
    PathBuf::from(format!("{SHORT_SOCKET_DIR_PREFIX}-{}", effective_uid()))
}

/// Create or validate an owner-only runtime directory without following a
/// symlink. Existing directories owned by this user are tightened to `0700`;
/// paths owned by another user fail closed.
fn create_private_dir(dir: &Path) -> Result<(), AgentConnectorError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(AgentConnectorError::LaunchFailed {
                message: format!("{} must not be a symlink", dir.display()),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(AgentConnectorError::LaunchFailed {
                message: format!("{} is not a directory", dir.display()),
            });
        }
        Ok(metadata) => {
            if metadata.uid() != effective_uid() {
                return Err(AgentConnectorError::LaunchFailed {
                    message: format!("{} is not owned by the current user", dir.display()),
                });
            }
            if metadata.permissions().mode() & 0o777 != 0o700 {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                    .map_err(|error| launch_error(dir, &error))?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return create_private_dir(dir);
                }
                Err(error) => return Err(launch_error(dir, &error)),
            }
        }
        Err(error) => return Err(launch_error(dir, &error)),
    }
    let metadata = fs::symlink_metadata(dir).map_err(|error| launch_error(dir, &error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != effective_uid()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(AgentConnectorError::LaunchFailed {
            message: format!("{} is not a private runtime directory", dir.display()),
        });
    }
    Ok(())
}

fn effective_uid() -> u32 {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: `geteuid` takes no arguments and has no failure mode.
    unsafe { geteuid() }
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
    fn prepare_creates_private_session_directories() {
        use std::os::unix::fs::PermissionsExt;

        let cwd = temp_cwd("prepare");
        let runtime = prepare_session_dir(&cwd).unwrap();
        let session_dir = runtime.settings_path.parent().unwrap();
        assert!(session_dir.starts_with(short_socket_dir()));
        assert!(session_dir.is_dir());
        assert_eq!(
            fs::symlink_metadata(short_socket_dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::symlink_metadata(session_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn deep_cwds_still_use_a_short_private_socket_path() {
        let mut cwd = temp_cwd("deep");
        for _ in 0..6 {
            cwd = cwd.join("very-long-directory-segment-for-socket-paths");
        }
        fs::create_dir_all(&cwd).unwrap();
        let runtime = prepare_session_dir(&cwd).unwrap();
        assert!(runtime.socket_path.starts_with(short_socket_dir()));
        fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn private_directory_helper_rejects_symlinks_and_tightens_owned_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let cwd = temp_cwd("hostile");
        let private_dir = cwd.join("runtime");
        fs::create_dir(&private_dir).unwrap();
        fs::set_permissions(&private_dir, fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&private_dir).unwrap();
        assert_eq!(
            fs::symlink_metadata(&private_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::remove_dir_all(&cwd).unwrap();

        let cwd = temp_cwd("symlink");
        let outside = temp_cwd("outside");
        let symlink_path = cwd.join("runtime");
        symlink(&outside, &symlink_path).unwrap();
        let problem =
            create_private_dir(&symlink_path).expect_err("runtime symlink must fail closed");
        assert!(problem.to_string().contains("must not be a symlink"));
        fs::remove_dir_all(&cwd).unwrap();
        fs::remove_dir_all(&outside).unwrap();
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
        assert!(command.contains(" 570 ||"), "command was: {command}");
        assert!(
            command.ends_with("exit 2; }"),
            "bridge failure must become a blocking hook exit: {command}"
        );
        assert_eq!(hook["hooks"][0]["timeout"], 600);
        fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn hook_command_blocks_when_a_preflighted_bridge_disappears() {
        use std::os::unix::fs::PermissionsExt;

        let cwd = temp_cwd("missing-after-preflight");
        let bridge = cwd.join("bridge");
        fs::write(&bridge, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&bridge, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(bridge.is_file());

        let runtime = prepare_session_dir(&cwd).unwrap();
        write_settings_file(&runtime, &bridge, &ApprovalPolicy::default(), 600).unwrap();
        fs::remove_file(&bridge).unwrap();

        let raw = fs::read_to_string(&runtime.settings_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let command = value["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2));
        fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn shell_quote_survives_embedded_single_quotes() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }
}
