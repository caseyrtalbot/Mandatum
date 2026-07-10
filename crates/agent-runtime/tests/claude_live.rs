//! Live integration tests for [`ClaudeCliConnector`] against the real,
//! authenticated `claude` CLI on this machine. Ignored by default; run with:
//!
//! ```sh
//! cargo test -p mandatum-agent-runtime -- --ignored
//! ```
//!
//! Cost is kept low: haiku model hint, tiny objective, max 4 turns.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use mandatum_agent_runtime::{
    AgentConnector, AgentLaunchSpec, AgentSession, AgentSessionEvent, ApprovalDecision,
    ApprovalVerdict, ClaudeCliConnector, ClaudeConnectorConfig, RiskLevel,
};

const OBJECTIVE: &str = "Run the shell command: echo MANDATUM_LIVE_OK";
const EVENT_TIMEOUT: Duration = Duration::from_secs(180);

fn live_connector() -> ClaudeCliConnector {
    ClaudeCliConnector::new(ClaudeConnectorConfig {
        bridge_binary: Some(PathBuf::from(env!(
            "CARGO_BIN_EXE_mandatum-approval-bridge"
        ))),
        ..ClaudeConnectorConfig::default()
    })
}

fn live_spec(tag: &str) -> AgentLaunchSpec {
    let cwd =
        std::env::temp_dir().join(format!("mandatum-claude-live-{tag}-{}", std::process::id()));
    fs::create_dir_all(&cwd).unwrap();
    let mut spec = AgentLaunchSpec::new(OBJECTIVE, cwd);
    spec.model = Some("haiku".to_owned());
    spec.max_turns = Some(4);
    spec
}

/// Drain events until `Closed` (or the deadline), deciding the first
/// approval with `verdict` and asserting on the approval request itself.
fn drive_session(mut session: AgentSession, verdict: ApprovalVerdict) -> Vec<AgentSessionEvent> {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    let mut events = Vec::new();
    let mut decided = false;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("live session timed out before Closed");
        let event = session
            .events
            .recv_timeout(remaining)
            .expect("event stream ended without Closed");
        if std::env::var_os("MANDATUM_LIVE_DEBUG").is_some() {
            eprintln!("[live] {event:?}");
        }
        events.push(event.clone());
        match event {
            AgentSessionEvent::ApprovalRequested(request) => {
                assert!(
                    request.command.contains("echo MANDATUM_LIVE_OK"),
                    "approval should carry the echo command, got {request:?}",
                );
                assert!(
                    matches!(request.risk.level, RiskLevel::Low | RiskLevel::Medium),
                    "echo should band Low/Medium, got {:?}",
                    request.risk,
                );
                assert!(!decided, "only one approval expected for this objective");
                session
                    .control
                    .decide(ApprovalDecision {
                        approval_id: request.approval_id,
                        verdict: verdict.clone(),
                    })
                    .expect("decide on the pending approval");
                decided = true;
            }
            AgentSessionEvent::Closed => break,
            _ => {}
        }
    }
    assert!(decided, "the run never requested approval: {events:#?}");
    session.control.shutdown();
    events
}

#[test]
#[ignore = "spawns the real claude CLI (authenticated, costs tokens)"]
fn live_approved_command_runs_and_completes() {
    let connector = live_connector();
    let session = connector.launch(&live_spec("approve")).unwrap();
    let events = drive_session(session, ApprovalVerdict::Approved);

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentSessionEvent::CommandRun { command } if command.contains("MANDATUM_LIVE_OK")
        )),
        "approved echo must surface as CommandRun: {events:#?}",
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentSessionEvent::Completed { .. })),
        "run must complete: {events:#?}",
    );
}

#[test]
#[ignore = "spawns the real claude CLI (authenticated, costs tokens)"]
fn live_rejected_command_never_runs_and_names_mandatum() {
    let connector = live_connector();
    let session = connector.launch(&live_spec("reject")).unwrap();
    let events = drive_session(
        session,
        ApprovalVerdict::Rejected {
            reason: Some("outside the approved mandate".to_owned()),
        },
    );

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentSessionEvent::CommandRun { .. })),
        "a rejected command must never report as run: {events:#?}",
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentSessionEvent::Completed { .. } | AgentSessionEvent::Failed { .. }
        )),
        "run must end in a terminal event: {events:#?}",
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentSessionEvent::OutputChunk(text) if text.contains("Mandatum")
        )),
        "the deny reason naming Mandatum should surface in the stream: {events:#?}",
    );
}
