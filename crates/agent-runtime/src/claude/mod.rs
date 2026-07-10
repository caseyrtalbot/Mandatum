//! Reference [`AgentConnector`]: drives the Claude Code CLI headlessly.
//!
//! `launch` prepares a per-session runtime directory under the spec cwd
//! (settings.json with a PreToolUse approval hook), binds a Unix approval
//! socket, spawns `claude -p <objective> --output-format stream-json`, and
//! pumps its stdout/stderr into [`AgentSessionEvent`]s on plain OS threads.
//! Gated tool calls travel: claude hook → `mandatum-approval-bridge` binary →
//! Unix socket → [`AgentSessionEvent::ApprovalRequested`] → user decision →
//! hook allow/deny. The bridge fails closed: no verdict ever means allow.

mod parser;
mod runtime_dir;
mod session;

use std::{
    os::unix::net::UnixListener,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use crate::{
    connector::{AgentConnector, AgentConnectorError, AgentSession},
    spec::AgentLaunchSpec,
};

use runtime_dir::{ALLOWED_TOOLS, SessionRuntimeDir};
use session::{ClaudeSessionControl, Shared};

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
            return Ok(path.clone());
        }
        if let Ok(path) = std::env::var(BRIDGE_ENV_VAR)
            && !path.trim().is_empty()
        {
            return Ok(PathBuf::from(path));
        }
        if let Ok(current) = std::env::current_exe()
            && let Some(dir) = current.parent()
        {
            let sibling = dir.join(BRIDGE_BINARY_NAME);
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
        // Fall back to PATH resolution at hook time.
        Ok(PathBuf::from(BRIDGE_BINARY_NAME))
    }
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

        let runtime = runtime_dir::prepare_session_dir(&spec.cwd)?;
        let bridge_binary = self.resolve_bridge_binary()?;
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
        let child = Arc::new(Mutex::new(Some(child)));

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
                child_pid,
                vec![listener_thread, stdout_thread, stderr_thread],
                runtime.socket_path,
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
    command
        .spawn()
        .map_err(|error| AgentConnectorError::LaunchFailed {
            message: format!(
                "failed to spawn {}: {error}",
                config.claude_binary.display()
            ),
        })
}

fn stdio_error(stream: &str) -> AgentConnectorError {
    AgentConnectorError::LaunchFailed {
        message: format!("claude child has no piped {stream}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

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
        let connector = ClaudeCliConnector::new(ClaudeConnectorConfig {
            bridge_binary: Some(PathBuf::from("/custom/bridge")),
            ..ClaudeConnectorConfig::default()
        });
        assert_eq!(
            connector.resolve_bridge_binary().unwrap(),
            Path::new("/custom/bridge")
        );
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
