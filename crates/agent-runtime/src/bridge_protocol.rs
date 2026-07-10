//! Wire protocol between the `mandatum-approval-bridge` hook binary and the
//! per-session approval listener inside [`crate::ClaudeCliConnector`].
//!
//! The bridge runs as a Claude Code `PreToolUse` hook: it receives the hook
//! payload on stdin, forwards a [`BridgeApprovalRequest`] as one JSON line
//! over the session's Unix socket, blocks until one [`BridgeVerdict`] line
//! comes back, and prints the hook decision JSON on stdout. The protocol is
//! **fail closed**: every failure on either side must resolve to a deny.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One JSON line, bridge → listener: a gated tool call awaiting a verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeApprovalRequest {
    /// Claude tool name, e.g. `"Bash"` or `"Write"`.
    pub tool_name: String,
    /// The raw `tool_input` object from the hook payload.
    pub tool_input: serde_json::Value,
    /// Working directory reported by the hook, when present.
    pub cwd: Option<PathBuf>,
    /// Claude's id for this tool call, when present. Used as the approval id.
    pub tool_use_id: Option<String>,
}

/// One JSON line, listener → bridge: the user's verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeVerdict {
    /// `true` lets the tool run; anything else is a deny.
    pub allow: bool,
    /// Deny reason surfaced to the agent. Should name Mandatum.
    pub reason: Option<String>,
}

/// Parse the `PreToolUse` hook payload Claude Code writes to the bridge's
/// stdin into a [`BridgeApprovalRequest`].
pub fn bridge_request_from_hook_json(raw: &str) -> Result<BridgeApprovalRequest, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("hook payload is not JSON: {error}"))?;
    let tool_name = value
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "hook payload has no tool_name".to_owned())?
        .to_owned();
    Ok(BridgeApprovalRequest {
        tool_name,
        tool_input: value
            .get("tool_input")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        cwd: value
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from),
        tool_use_id: value
            .get("tool_use_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

/// The `hookSpecificOutput` JSON the bridge prints on stdout to allow the
/// gated tool call.
pub fn allow_hook_output() -> String {
    hook_output("allow", "Approved via Mandatum")
}

/// The `hookSpecificOutput` JSON the bridge prints on stdout to deny the
/// gated tool call. The reason always names Mandatum so the agent (and its
/// transcript) can attribute the block.
pub fn deny_hook_output(reason: &str) -> String {
    let reason = if reason.contains("Mandatum") {
        reason.to_owned()
    } else {
        format!("Mandatum: {reason}")
    };
    hook_output("deny", &reason)
}

fn hook_output(decision: &str, reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_payload_parses_into_a_bridge_request() {
        let raw = r#"{
            "session_id": "abc",
            "cwd": "/tmp/project",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi", "description": "greet"},
            "tool_use_id": "toolu_123"
        }"#;
        let request = bridge_request_from_hook_json(raw).unwrap();
        assert_eq!(request.tool_name, "Bash");
        assert_eq!(request.cwd, Some(PathBuf::from("/tmp/project")));
        assert_eq!(request.tool_use_id.as_deref(), Some("toolu_123"));
        assert_eq!(request.tool_input["command"], "echo hi");
    }

    #[test]
    fn hook_payload_without_tool_name_is_rejected() {
        assert!(bridge_request_from_hook_json(r#"{"cwd": "/tmp"}"#).is_err());
        assert!(bridge_request_from_hook_json("not json").is_err());
    }

    #[test]
    fn verdicts_round_trip_as_json_lines() {
        for verdict in [
            BridgeVerdict {
                allow: true,
                reason: None,
            },
            BridgeVerdict {
                allow: false,
                reason: Some("Mandatum rejected this command".to_owned()),
            },
        ] {
            let line = serde_json::to_string(&verdict).unwrap();
            assert!(!line.contains('\n'));
            let back: BridgeVerdict = serde_json::from_str(&line).unwrap();
            assert_eq!(back, verdict);
        }
    }

    #[test]
    fn deny_output_always_names_mandatum_and_allow_output_is_wellformed() {
        let deny = deny_hook_output("socket unreachable");
        let value: serde_json::Value = serde_json::from_str(&deny).unwrap();
        let output = &value["hookSpecificOutput"];
        assert_eq!(output["permissionDecision"], "deny");
        assert!(
            output["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("Mandatum")
        );

        let allow: serde_json::Value = serde_json::from_str(&allow_hook_output()).unwrap();
        assert_eq!(allow["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(allow["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    }

    #[test]
    fn deny_output_does_not_double_prefix_reasons_that_already_name_mandatum() {
        let deny = deny_hook_output("Mandatum rejected this command");
        let value: serde_json::Value = serde_json::from_str(&deny).unwrap();
        assert_eq!(
            value["hookSpecificOutput"]["permissionDecisionReason"],
            "Mandatum rejected this command"
        );
    }
}
