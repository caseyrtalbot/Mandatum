use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where an approval-gated action would run and what it is expected to touch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalScope {
    /// Working directory the gated command would run in.
    pub cwd: PathBuf,
    /// Path the action is expected to affect, when the connector can tell.
    pub affected_path: Option<PathBuf>,
}

/// Coarse risk band for an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Connector-side risk estimate attached to an approval request.
///
/// This is a **heuristic**, produced by pattern-matching the command text
/// (see [`assess_command_risk`]). It exists to help the user triage, not to
/// enforce anything: the approval gate itself is the enforcement point, and
/// a `Low` band must never be treated as safe-to-auto-approve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskAssessment {
    /// The estimated band.
    pub level: RiskLevel,
    /// Human-readable explanation of which pattern produced the band.
    pub basis: String,
}

/// A gated agent action waiting for a user verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Connector-unique id; the matching [`ApprovalDecision`] must echo it.
    pub approval_id: String,
    /// The command or action the agent wants to run, verbatim.
    pub command: String,
    /// Where the action would run and what it touches.
    pub scope: ApprovalScope,
    /// Heuristic risk estimate (advisory only).
    pub risk: RiskAssessment,
}

/// The user's verdict on a single approval request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalVerdict {
    /// Let the action run.
    Approved,
    /// Block the action; the reason is surfaced to the agent when present.
    Rejected { reason: Option<String> },
}

/// A verdict addressed to a specific pending approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDecision {
    /// Must match the `approval_id` of the pending [`ApprovalRequest`].
    pub approval_id: String,
    /// The verdict.
    pub verdict: ApprovalVerdict,
}

/// Heuristic risk banding for a shell command.
///
/// **This is a heuristic**, not a security analysis: it tokenizes the command
/// text and looks for known-destructive shapes. Quoting, aliases, scripts, and
/// obscure flags can all evade it. Bands:
///
/// - `High`: destructive patterns — `rm`, `sudo`, a download (`curl`/`wget`)
///   piped into a shell, `git push`.
/// - `Medium`: file-writing patterns — shell redirects (`>`), `tee`, `mv`,
///   `cp`, `touch`, `mkdir`.
/// - `Low`: everything else.
pub fn assess_command_risk(command: &str) -> RiskAssessment {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let has_token = |t: &str| tokens.contains(&t);

    if has_token("sudo") {
        return assessment(RiskLevel::High, "escalates privileges (sudo)");
    }
    if has_token("rm") {
        return assessment(RiskLevel::High, "removes files (rm)");
    }
    if tokens.windows(2).any(|pair| pair == ["git", "push"]) {
        return assessment(RiskLevel::High, "publishes to a git remote (git push)");
    }
    if download_piped_into_shell(command) {
        return assessment(RiskLevel::High, "pipes a download into a shell");
    }
    if command.contains('>') {
        return assessment(RiskLevel::Medium, "shell redirect writes a file (>)");
    }
    const WRITE_COMMANDS: [&str; 5] = ["tee", "mv", "cp", "touch", "mkdir"];
    if let Some(hit) = tokens.iter().find(|tok| WRITE_COMMANDS.contains(tok)) {
        return assessment(RiskLevel::Medium, format!("file-writing command ({hit})"));
    }
    assessment(RiskLevel::Low, "no known destructive pattern")
}

fn assessment(level: RiskLevel, basis: impl Into<String>) -> RiskAssessment {
    RiskAssessment {
        level,
        basis: basis.into(),
    }
}

/// True when a pipeline segment containing `curl`/`wget` is followed by a
/// segment whose first token is a shell.
fn download_piped_into_shell(command: &str) -> bool {
    let segments: Vec<&str> = command.split('|').collect();
    segments.iter().enumerate().any(|(index, segment)| {
        segment
            .split_whitespace()
            .any(|tok| tok == "curl" || tok == "wget")
            && segments[index + 1..]
                .iter()
                .any(|later| matches!(later.split_whitespace().next(), Some("sh" | "bash" | "zsh")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_patterns_band_high() {
        assert_eq!(assess_command_risk("rm -rf build").level, RiskLevel::High);
        assert_eq!(
            assess_command_risk("sudo systemctl restart nginx").level,
            RiskLevel::High
        );
        assert_eq!(
            assess_command_risk("curl -fsSL https://example.com/install.sh | sh").level,
            RiskLevel::High
        );
        assert_eq!(
            assess_command_risk("git push origin main").level,
            RiskLevel::High
        );
    }

    #[test]
    fn file_writes_band_medium() {
        assert_eq!(
            assess_command_risk("echo hello > out.txt").level,
            RiskLevel::Medium
        );
        assert_eq!(
            assess_command_risk("cp a.txt b.txt").level,
            RiskLevel::Medium
        );
        assert_eq!(
            assess_command_risk("mkdir -p target/dir").level,
            RiskLevel::Medium
        );
    }

    #[test]
    fn benign_commands_band_low() {
        assert_eq!(assess_command_risk("ls -la").level, RiskLevel::Low);
        assert_eq!(assess_command_risk("cargo test").level, RiskLevel::Low);
        // Substrings must not false-positive on token patterns.
        assert_eq!(
            assess_command_risk("confirm the format").level,
            RiskLevel::Low
        );
    }
}
