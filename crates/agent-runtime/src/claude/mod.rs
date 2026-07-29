//! Reference [`AgentConnector`]: drives the Claude Code CLI headlessly.
//!
//! `launch` prepares a private per-user runtime directory
//! (settings.json with a PreToolUse approval hook), binds a Unix approval
//! socket, spawns `claude -p <objective> --output-format stream-json`, and
//! pumps its stdout/stderr into [`AgentSessionEvent`]s on plain OS threads.
//! Gated tool calls travel: claude hook → `mandatum-approval-bridge` binary →
//! Unix socket → [`AgentSessionEvent::ApprovalRequested`] → user decision →
//! hook allow/deny. The bridge fails closed: only an explicit verdict can
//! allow a gated action.

mod parser;
mod runtime_dir;
mod session;

use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, mpsc},
    thread,
};

use crate::{
    connector::{AgentConnector, AgentConnectorError, AgentSession},
    spec::AgentLaunchSpec,
};

use runtime_dir::{ALLOWED_TOOLS, SessionRuntimeDir};
use session::{ChildHandle, ClaudeSessionControl, Shared};

/// Name of the approval-bridge binary shipped next to the workstation.
const BRIDGE_BINARY_NAME: &str = "mandatum-approval-bridge";

/// Environment variable that overrides bridge-binary resolution.
const BRIDGE_ENV_VAR: &str = "MANDATUM_APPROVAL_BRIDGE";

/// Configuration for [`ClaudeCliConnector`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeConnectorConfig {
    /// The `claude` executable (name resolved via PATH, or an absolute path).
    pub claude_binary: PathBuf,
    /// The approval-bridge executable. `None` resolves, in order: the
    /// `MANDATUM_APPROVAL_BRIDGE` env var, a sibling of the current
    /// executable, then PATH lookup by name.
    pub bridge_binary: Option<PathBuf>,
    /// `--max-turns` when the spec does not set one.
    pub default_max_turns: u32,
    /// PreToolUse hook timeout in seconds; the ceiling on how long an
    /// approval may stay pending before claude gives up on the bridge.
    pub hook_timeout_secs: u64,
}

impl Default for ClaudeConnectorConfig {
    fn default() -> Self {
        Self {
            claude_binary: PathBuf::from("claude"),
            bridge_binary: None,
            default_max_turns: 25,
            hook_timeout_secs: 600,
        }
    }
}

/// [`AgentConnector`] backed by the Claude Code CLI.
#[derive(Clone, Debug, Default)]
pub struct ClaudeCliConnector {
    config: ClaudeConnectorConfig,
}

impl ClaudeCliConnector {
    pub fn new(config: ClaudeConnectorConfig) -> Self {
        Self { config }
    }

    fn resolve_bridge_binary(&self) -> Result<PathBuf, AgentConnectorError> {
        if let Some(path) = &self.config.bridge_binary {
            return resolve_required_executable(path, "configured approval bridge");
        }
        if let Ok(path) = std::env::var(BRIDGE_ENV_VAR)
            && !path.trim().is_empty()
        {
            return resolve_required_executable(Path::new(&path), "MANDATUM_APPROVAL_BRIDGE");
        }
        if let Ok(current) = std::env::current_exe()
            && let Some(dir) = current.parent()
        {
            let sibling = dir.join(BRIDGE_BINARY_NAME);
            match fs::symlink_metadata(&sibling) {
                Ok(_) => return validate_executable(sibling, "sibling approval bridge"),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(AgentConnectorError::LaunchFailed {
                        message: format!(
                            "could not inspect sibling approval bridge {}: {error}",
                            sibling.display()
                        ),
                    });
                }
            }
        }
        resolve_on_path(Path::new(BRIDGE_BINARY_NAME))?.ok_or_else(|| {
            AgentConnectorError::LaunchFailed {
                message: format!(
                    "{BRIDGE_BINARY_NAME} is required for gated agent sessions; \
                     install it beside Mandatum, put it on PATH, or set {BRIDGE_ENV_VAR}"
                ),
            }
        })
    }
}

fn resolve_required_executable(
    configured: &Path,
    label: &str,
) -> Result<PathBuf, AgentConnectorError> {
    if configured.as_os_str().is_empty() {
        return Err(AgentConnectorError::LaunchFailed {
            message: format!("{label} path is empty"),
        });
    }
    if configured.is_absolute() || configured.components().count() > 1 {
        let absolute = if configured.is_absolute() {
            configured.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| AgentConnectorError::LaunchFailed {
                    message: format!("could not resolve {label}: {error}"),
                })?
                .join(configured)
        };
        return validate_executable(absolute, label);
    }
    resolve_on_path(configured)?.ok_or_else(|| AgentConnectorError::LaunchFailed {
        message: format!("{label} {} was not found on PATH", configured.display()),
    })
}

fn resolve_on_path(name: &Path) -> Result<Option<PathBuf>, AgentConnectorError> {
    let Some(path_value) = std::env::var_os("PATH") else {
        return Ok(None);
    };
    for directory in std::env::split_paths(&path_value) {
        let candidate = if directory.is_absolute() {
            directory.join(name)
        } else {
            std::env::current_dir()
                .map_err(|error| AgentConnectorError::LaunchFailed {
                    message: format!("could not resolve approval bridge on PATH: {error}"),
                })?
                .join(directory)
                .join(name)
        };
        match fs::symlink_metadata(&candidate) {
            Ok(_) => return validate_executable(candidate, "approval bridge on PATH").map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AgentConnectorError::LaunchFailed {
                    message: format!(
                        "could not inspect approval bridge {}: {error}",
                        candidate.display()
                    ),
                });
            }
        }
    }
    Ok(None)
}

fn validate_executable(path: PathBuf, label: &str) -> Result<PathBuf, AgentConnectorError> {
    let metadata =
        fs::symlink_metadata(&path).map_err(|error| AgentConnectorError::LaunchFailed {
            message: format!("{label} {} is unavailable: {error}", path.display()),
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AgentConnectorError::LaunchFailed {
            message: format!("{label} {} must be a regular file", path.display()),
        });
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(AgentConnectorError::LaunchFailed {
            message: format!("{label} {} is not executable", path.display()),
        });
    }
    Ok(path)
}

impl AgentConnector for ClaudeCliConnector {
    fn launch(&self, spec: &AgentLaunchSpec) -> Result<AgentSession, AgentConnectorError> {
        if spec.objective.trim().is_empty() {
            return Err(AgentConnectorError::InvalidSpec {
                message: "objective must not be empty".to_owned(),
            });
        }
        if !spec.cwd.is_dir() {
            return Err(AgentConnectorError::InvalidSpec {
                message: format!("cwd {} is not a directory", spec.cwd.display()),
            });
        }

        let bridge_binary = if spec.approval_policy.gated_classes.is_empty() {
            PathBuf::from(BRIDGE_BINARY_NAME)
        } else {
            self.resolve_bridge_binary()?
        };
        let runtime = runtime_dir::prepare_session_dir(&spec.cwd)?;
        runtime_dir::write_settings_file(
            &runtime,
            &bridge_binary,
            &spec.approval_policy,
            self.config.hook_timeout_secs,
        )?;

        let _ = std::fs::remove_file(&runtime.socket_path);
        let listener = UnixListener::bind(&runtime.socket_path).map_err(|error| {
            AgentConnectorError::LaunchFailed {
                message: format!(
                    "failed to bind approval socket {}: {error}",
                    runtime.socket_path.display()
                ),
            }
        })?;

        let mut child = spawn_claude(&self.config, spec, &runtime)?;
        let child_pid = child.id();
        let stdout = child.stdout.take().ok_or_else(|| stdio_error("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| stdio_error("stderr"))?;

        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::new());
        // The child was spawned into its own process group, so its pid is the
        // pgid every signal path addresses — until the pump reaps it.
        let child = Arc::new(ChildHandle::new(Some(child), Some(child_pid)));

        let listener_shared = Arc::clone(&shared);
        let listener_tx = tx.clone();
        let session_cwd = spec.cwd.clone();
        let listener_thread = thread::spawn(move || {
            session::run_listener(listener, &listener_shared, &listener_tx, &session_cwd);
        });

        let stdout_shared = Arc::clone(&shared);
        let stdout_child = Arc::clone(&child);
        let stdout_tx = tx.clone();
        let stdout_thread = thread::spawn(move || {
            session::run_stdout_pump(stdout, &stdout_tx, &stdout_shared, &stdout_child);
        });

        let stderr_thread = thread::spawn(move || {
            session::run_stderr_pump(stderr, &tx);
        });

        Ok(AgentSession {
            events: rx,
            control: Box::new(ClaudeSessionControl::new(
                shared,
                child,
                vec![listener_thread, stdout_thread, stderr_thread],
                runtime,
            )),
        })
    }

    fn name(&self) -> &str {
        "claude-cli"
    }
}

fn spawn_claude(
    config: &ClaudeConnectorConfig,
    spec: &AgentLaunchSpec,
    runtime: &SessionRuntimeDir,
) -> Result<std::process::Child, AgentConnectorError> {
    use std::os::unix::process::CommandExt;

    let max_turns = spec.max_turns.unwrap_or(config.default_max_turns);
    let mut command = std::process::Command::new(&config.claude_binary);
    command
        .arg("-p")
        .arg(&spec.objective)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--settings")
        .arg(&runtime.settings_path)
        .arg("--max-turns")
        .arg(max_turns.to_string())
        .arg("--allowedTools")
        .arg(ALLOWED_TOOLS)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Own process group so interrupt/shutdown can signal claude and every
        // process it spawns, without touching the workstation's group.
        .process_group(0);
    if let Some(model) = &spec.model {
        command.arg("--model").arg(model);
    }
    command.spawn().map_err(|error| {
        let binary = config.claude_binary.display();
        // A missing binary is the one launch failure every new install hits;
        // name the fix instead of echoing "os error 2" at the user.
        let message = if error.kind() == io::ErrorKind::NotFound {
            format!("`{binary}` was not found on PATH — install Claude Code or add it to PATH")
        } else {
            format!("could not start `{binary}`: {error}")
        };
        AgentConnectorError::LaunchFailed { message }
    })
}

fn stdio_error(stream: &str) -> AgentConnectorError {
    AgentConnectorError::LaunchFailed {
        message: format!("claude child has no piped {stream}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mandatum-bridge-{tag}-{}", std::process::id()))
    }

    #[test]
    fn empty_objective_and_missing_cwd_are_rejected_before_any_spawn() {
        let connector = ClaudeCliConnector::default();
        let empty = AgentLaunchSpec::new("   ", std::env::temp_dir());
        assert!(matches!(
            connector.launch(&empty),
            Err(AgentConnectorError::InvalidSpec { .. })
        ));

        let missing_cwd = AgentLaunchSpec::new("do things", "/nonexistent/mandatum/cwd");
        assert!(matches!(
            connector.launch(&missing_cwd),
            Err(AgentConnectorError::InvalidSpec { .. })
        ));
        assert_eq!(connector.name(), "claude-cli");
    }

    #[test]
    fn bridge_binary_resolution_prefers_the_config_override() {
        let bridge = temp_path("configured");
        fs::write(&bridge, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&bridge, fs::Permissions::from_mode(0o755)).unwrap();
        let connector = ClaudeCliConnector::new(ClaudeConnectorConfig {
            bridge_binary: Some(bridge.clone()),
            ..ClaudeConnectorConfig::default()
        });
        assert_eq!(connector.resolve_bridge_binary().unwrap(), bridge);
        fs::remove_file(bridge).unwrap();
    }

    #[test]
    fn gated_launch_fails_before_claude_when_bridge_is_missing_or_not_executable() {
        let cwd = temp_path("cwd");
        fs::create_dir(&cwd).unwrap();

        for (tag, mode) in [("missing", None), ("not-executable", Some(0o644))] {
            let bridge = temp_path(tag);
            if let Some(mode) = mode {
                fs::write(&bridge, "#!/bin/sh\nexit 0\n").unwrap();
                fs::set_permissions(&bridge, fs::Permissions::from_mode(mode)).unwrap();
            }
            let connector = ClaudeCliConnector::new(ClaudeConnectorConfig {
                claude_binary: PathBuf::from("/bin/false"),
                bridge_binary: Some(bridge.clone()),
                ..ClaudeConnectorConfig::default()
            });
            let result = connector.launch(&AgentLaunchSpec::new("test", &cwd));
            assert!(matches!(
                result,
                Err(AgentConnectorError::LaunchFailed { .. })
            ));
            if bridge.exists() {
                fs::remove_file(&bridge).unwrap();
            }
        }

        fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn default_config_matches_the_documented_defaults() {
        let config = ClaudeConnectorConfig::default();
        assert_eq!(config.claude_binary, Path::new("claude"));
        assert_eq!(config.default_max_turns, 25);
        assert_eq!(config.hook_timeout_secs, 600);
        assert_eq!(config.bridge_binary, None);
    }
}
