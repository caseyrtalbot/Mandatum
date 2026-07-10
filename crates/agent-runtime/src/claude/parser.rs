//! Maps `claude --output-format stream-json` JSONL lines to
//! [`AgentSessionEvent`]s.
//!
//! The parser is stateful: a `tool_use` block records what the agent *asked*
//! to run, and the matching `tool_result` decides what actually happened.
//! [`AgentSessionEvent::CommandRun`] is therefore emitted only when a Bash
//! tool call returns without error — a hook-denied command never reports as
//! run, which is what the approval gate promises. File-change events are
//! emitted at `tool_use` time (the write intent is the signal there).
//!
//! Never panics on garbage: lines that are not JSON become raw
//! [`AgentSessionEvent::OutputChunk`]s; well-formed stream noise
//! (`thinking_tokens`, `rate_limit_event`, hook bookkeeping) is dropped.

use std::{collections::HashMap, path::PathBuf};

use mandatum_core::AgentStatus;
use serde_json::Value;

use crate::events::{AgentSessionEvent, FileChange, FileChangeKind};

/// Maximum characters of an assistant text block that become a
/// [`AgentSessionEvent::Summary`].
const SUMMARY_CHARS: usize = 200;

/// Tool names whose `tool_use` blocks represent file writes.
const FILE_WRITE_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// What a pending tool call asked for, keyed by `tool_use` id until its
/// `tool_result` arrives.
#[derive(Clone, Debug)]
enum PendingTool {
    Bash { command: String },
    FileWrite,
    Other,
}

/// Stateful JSONL → event mapper for one Claude CLI session stream.
#[derive(Debug, Default)]
pub(crate) struct StreamParser {
    pending_tools: HashMap<String, PendingTool>,
}

impl StreamParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Map one stdout line to zero or more session events.
    pub(crate) fn parse_line(&mut self, line: &str) -> Vec<AgentSessionEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            return vec![AgentSessionEvent::OutputChunk(trimmed.to_owned())];
        };
        match value.get("type").and_then(Value::as_str) {
            Some("system") => self.parse_system(&value),
            Some("assistant") => self.parse_assistant(&value),
            Some("user") => self.parse_user(&value),
            Some("result") => self.parse_result(&value),
            // Known stream noise (rate_limit_event, …) and unknown structured
            // lines are dropped rather than surfaced as output.
            Some(_) => Vec::new(),
            None => vec![AgentSessionEvent::OutputChunk(trimmed.to_owned())],
        }
    }

    fn parse_system(&self, value: &Value) -> Vec<AgentSessionEvent> {
        match value.get("subtype").and_then(Value::as_str) {
            Some("init") => vec![AgentSessionEvent::Status(AgentStatus::Running)],
            _ => Vec::new(),
        }
    }

    fn parse_assistant(&mut self, value: &Value) -> Vec<AgentSessionEvent> {
        let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if !text.is_empty() {
                        events.push(AgentSessionEvent::Summary(truncate_chars(
                            text,
                            SUMMARY_CHARS,
                        )));
                        events.push(AgentSessionEvent::OutputChunk(text.to_owned()));
                    }
                }
                Some("tool_use") => events.extend(self.parse_tool_use(block)),
                // `thinking` and other block types carry no surfaced signal.
                _ => {}
            }
        }
        events
    }

    fn parse_tool_use(&mut self, block: &Value) -> Vec<AgentSessionEvent> {
        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
        let id = block.get("id").and_then(Value::as_str);
        let input = block.get("input").cloned().unwrap_or(Value::Null);

        if name == "Bash" {
            let command = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if let Some(id) = id {
                self.pending_tools.insert(
                    id.to_owned(),
                    PendingTool::Bash {
                        command: command.clone(),
                    },
                );
            }
            return vec![AgentSessionEvent::Action {
                description: format!("Bash: {command}"),
            }];
        }

        if FILE_WRITE_TOOLS.contains(&name) {
            if let Some(id) = id {
                self.pending_tools
                    .insert(id.to_owned(), PendingTool::FileWrite);
            }
            let path = file_write_path(&input);
            let change_kind = if name == "Write" {
                // Write creates the file (or replaces it wholesale); the
                // other write tools modify an existing one.
                FileChangeKind::Added
            } else {
                FileChangeKind::Modified
            };
            let description = match &path {
                Some(path) => format!("{name}: {}", path.display()),
                None => name.to_owned(),
            };
            let mut events = vec![AgentSessionEvent::Action { description }];
            if let Some(path) = path {
                events.push(AgentSessionEvent::FilesChanged(vec![FileChange {
                    path,
                    change_kind,
                }]));
            }
            return events;
        }

        if let Some(id) = id {
            self.pending_tools.insert(id.to_owned(), PendingTool::Other);
        }
        vec![AgentSessionEvent::Action {
            description: name.to_owned(),
        }]
    }

    fn parse_user(&mut self, value: &Value) -> Vec<AgentSessionEvent> {
        let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let pending = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .and_then(|id| self.pending_tools.remove(id));
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if is_error {
                let text = content_text(block.get("content"));
                events.push(AgentSessionEvent::OutputChunk(format!(
                    "[tool error] {text}"
                )));
                continue;
            }
            if let Some(PendingTool::Bash { command }) = pending {
                events.push(AgentSessionEvent::CommandRun { command });
            }
        }
        events
    }

    fn parse_result(&self, value: &Value) -> Vec<AgentSessionEvent> {
        let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or("");
        let is_error = value
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let text = value
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if subtype == "success" && !is_error {
            vec![AgentSessionEvent::Completed { summary: text }]
        } else {
            let error = if text.is_empty() {
                format!("claude run failed ({subtype})")
            } else {
                text
            };
            vec![AgentSessionEvent::Failed { error }]
        }
    }
}

/// The path a file-write tool call targets, from its input object.
fn file_write_path(input: &Value) -> Option<PathBuf> {
    ["file_path", "notebook_path"]
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .map(PathBuf::from)
}

/// Flatten a `tool_result` content value (string or block list) to text.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real `claude -p … --output-format stream-json` run on
    /// this machine (claude CLI 2.1.205): approved echo command.
    const ALLOW_FIXTURE: &str = include_str!("../../tests/fixtures/claude_stream_allow.jsonl");
    /// Same objective, PreToolUse hook denied the Bash call.
    const DENY_FIXTURE: &str = include_str!("../../tests/fixtures/claude_stream_deny.jsonl");
    /// A Write tool creating hello.txt.
    const WRITE_FIXTURE: &str = include_str!("../../tests/fixtures/claude_stream_write.jsonl");

    fn parse_all(fixture: &str) -> Vec<AgentSessionEvent> {
        let mut parser = StreamParser::new();
        fixture
            .lines()
            .flat_map(|line| parser.parse_line(line))
            .collect()
    }

    #[test]
    fn allow_fixture_maps_init_action_command_run_summary_and_completed() {
        let events = parse_all(ALLOW_FIXTURE);
        assert_eq!(events[0], AgentSessionEvent::Status(AgentStatus::Running));
        assert!(events.contains(&AgentSessionEvent::Action {
            description: "Bash: echo MANDATUM_PROBE_OK".to_owned(),
        }));
        assert!(events.contains(&AgentSessionEvent::CommandRun {
            command: "echo MANDATUM_PROBE_OK".to_owned(),
        }));
        assert!(events.contains(&AgentSessionEvent::Summary("Done.".to_owned())));
        assert!(events.contains(&AgentSessionEvent::OutputChunk("Done.".to_owned())));
        assert_eq!(
            events.last(),
            Some(&AgentSessionEvent::Completed {
                summary: "Done.".to_owned(),
            })
        );
        // CommandRun comes from the tool_result, after the request Action.
        let action = events
            .iter()
            .position(|e| matches!(e, AgentSessionEvent::Action { .. }))
            .unwrap();
        let run = events
            .iter()
            .position(|e| matches!(e, AgentSessionEvent::CommandRun { .. }))
            .unwrap();
        assert!(action < run);
    }

    #[test]
    fn deny_fixture_reports_the_error_and_never_claims_the_command_ran() {
        let events = parse_all(DENY_FIXTURE);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentSessionEvent::CommandRun { .. })),
            "a denied command must not produce CommandRun",
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentSessionEvent::OutputChunk(text) if text.starts_with("[tool error]")
                && text.contains("Mandatum probe deny")
        )));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentSessionEvent::Completed { .. }))
        );
    }

    #[test]
    fn write_fixture_maps_to_files_changed_with_added_kind() {
        let events = parse_all(WRITE_FIXTURE);
        let changes = events.iter().find_map(|e| match e {
            AgentSessionEvent::FilesChanged(changes) => Some(changes),
            _ => None,
        });
        let changes = changes.expect("Write tool_use must produce FilesChanged");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_kind, FileChangeKind::Added);
        assert!(changes[0].path.ends_with("hello.txt"));
    }

    #[test]
    fn garbage_and_blank_lines_never_panic() {
        let mut parser = StreamParser::new();
        assert_eq!(parser.parse_line("   "), Vec::new());
        assert_eq!(
            parser.parse_line("not json at all"),
            vec![AgentSessionEvent::OutputChunk("not json at all".to_owned())]
        );
        assert_eq!(
            parser.parse_line(r#"{"no_type_field": true}"#),
            vec![AgentSessionEvent::OutputChunk(
                r#"{"no_type_field": true}"#.to_owned()
            )]
        );
        // Structured noise is dropped.
        assert_eq!(
            parser.parse_line(r#"{"type":"rate_limit_event","rate_limit_info":{}}"#),
            Vec::new()
        );
        assert_eq!(
            parser.parse_line(r#"{"type":"system","subtype":"thinking_tokens"}"#),
            Vec::new()
        );
    }

    #[test]
    fn error_results_map_to_failed() {
        let mut parser = StreamParser::new();
        assert_eq!(
            parser.parse_line(
                r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":null}"#
            ),
            vec![AgentSessionEvent::Failed {
                error: "claude run failed (error_max_turns)".to_owned(),
            }]
        );
        assert_eq!(
            parser.parse_line(
                r#"{"type":"result","subtype":"success","is_error":true,"result":"boom"}"#
            ),
            vec![AgentSessionEvent::Failed {
                error: "boom".to_owned(),
            }]
        );
    }

    #[test]
    fn long_assistant_text_is_summarized_to_200_chars_with_full_output() {
        let text = "x".repeat(500);
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": text}]}
        })
        .to_string();
        let mut parser = StreamParser::new();
        let events = parser.parse_line(&line);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], AgentSessionEvent::Summary("x".repeat(200)));
        assert_eq!(events[1], AgentSessionEvent::OutputChunk("x".repeat(500)));
    }
}
