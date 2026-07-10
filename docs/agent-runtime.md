# Agent Runtime

## Product Role

Agents are session actors. They run work, request approvals, change files, run
commands, surface verification, and produce handoffs.

An agent pane is not a chat sidebar. It is a work surface that makes an
agent's state inspectable in the same spatial model as terminals and tasks.

## What Is Built

### Contract crate (`mandatum-agent-runtime`)

Engine-side (deps: `mandatum-core`, serde, serde_json; enforced by the L1/L2
conformance gate). Owns:

- `AgentConnector::launch(&AgentLaunchSpec) -> AgentSession` — object-safe
  connector trait; a session is a `std::sync::mpsc::Receiver<AgentSessionEvent>`
  plus a boxed `AgentSessionControl` (decide / interrupt / shutdown / is_alive).
- `AgentSessionEvent` — Status, Action, Summary, OutputChunk, CommandRun,
  FilesChanged, ApprovalRequested, Completed, Failed, Closed.
- The approval protocol: `ApprovalRequest { approval_id, command, scope,
  risk }` answered by `ApprovalDecision { approval_id, Approved | Rejected }`.
  Risk bands are advisory heuristics (`assess_command_risk`); the gate itself
  is the enforcement point.
- `FakeConnector` — deterministic scripted connector used by every test.

### App integration (`crates/app`)

- `AgentRuntimeRegistry` (`crates/app/src/agent_runtime.rs`) mirrors the PTY
  runtime discipline: one forwarder thread per live session pumps events into
  the app channel wrapped as `AgentRuntimeEvent { pane_id, restart_generation,
  runtime_token, event }`; `app_state` applies an event only when the pane's
  current generation and token match, else drops it ([L3-GATE] tested by
  `stale_agent_events_after_restart_are_ignored`).
- Durable folding: accepted events update `mandatum_core::AgentPaneIntent`
  (status, latest summary, changed-file path list, pending approval count and
  ids, decided-approval history). Live-only state (current action, ~200-line
  output tail, full `ApprovalRequest` detail) lives in the registry and is
  never serialized ([L3-GATE] tested by
  `agent_runtime_state_is_not_serialized_with_workspace_intent`).
- Approval history: every decision appends an `AgentApprovalRecord
  { approval_id, command, approved }` to the durable intent, so past
  decisions remain visible after restart.
- Connector selection: `AppConfig.agent_connector` (`fake | claude`,
  default `claude`; both kinds are wired — gated by
  `every_configured_connector_kind_is_wired`). Tests wire `fake` everywhere.
- Model hint: `AppConfig.agent_model` flows into `AgentLaunchSpec.model`
  (`--model` for the Claude CLI). `AppConfig::from_current_dir` reads it from
  `MANDATUM_AGENT_MODEL`; `None` uses the connector's account default.
- Session-replacement invariant: a relaunch bumps the pane's restart
  generation only after the new session launches. A failed relaunch leaves
  the previous session live and authoritative under its unchanged generation
  ([L3-GATE] tested by
  `failed_relaunch_keeps_the_previous_session_authoritative`).
- Detach folding: whenever a live session is discarded without an outcome —
  Stop Agent, session `Closed`, OpenProject reconciling away a runtime whose
  pane is no longer in the active session, or loading a workspace from disk —
  `AgentPaneIntent::detach_live_session` folds running/waiting to `unknown`
  and clears pending approval count/ids (approval history stays).

### Commands

Built and dispatched through the palette (`crates/commands`):

- New Agent Pane (`a`) — creates a draft agent pane with the configured
  default objective
- Start Agent (`g`) — launches the connector for the focused agent pane's
  objective; creates a pane first if none exists
- Stop Agent (`k`)
- Approve Agent Action (`y`) / Reject Agent Action (`d`) — also available as
  direct keys `y` / `n` (no palette) while the focused pane has a pending
  approval; the palette detail text documents this
- Focus Next Waiting Agent (`j`)

### Agent Pane Surface

The scene (`PaneContent::Agent` in `mandatum-scene`) carries objective,
status, current action, latest summary, pending approval detail (command,
scope, risk band + basis, key hints), the last ~10 changed files, and the
output tail. The ratatui renderer draws the approval block visually distinct
(yellow, bold header) and flags the pane title with `approval` while waiting.

A pane waiting for approval also surfaces in the status strip:
`1 approval waiting — <pane>`.

## Agent States

`AgentStatus`: draft, running, waiting for approval, blocked, failed,
complete, unknown. A stopped or silently-closed session folds to `unknown`
(no invented outcome).

Not yet built: distinct `queued` and `stopped` states.

## Runtime Boundaries

The runtime registry owns (live, never persisted):

- live session control handles and forwarder threads
- approval wait state (the full pending request)
- current action and output tail

The workspace engine persists (durable intent):

- agent pane intent: objective, thread reference, durable status,
  last summary, changed-file paths
- pending approval count and ids (informational only once persisted: a
  loaded workspace has no live session, so these are cleared — and
  running/waiting statuses fold to unknown — on every restore)
- decided-approval history

Live handles and transient output streams are not durable truth.

## Not Yet Built (aspirational)

- **Commands:** Open Agent Thread, Show Agent Changes, Show Agent Checks,
  Write Agent Handoff, Pin Agent Pane.
- **Checks surface:** tests run, checks failed, checks not run, verification
  results attached to the actor.
- **Last update time** on the pane.
- **Jump from an agent pane to a changed-file summary.**
- **Session map / palette-context visibility** of agent states beyond the
  pane and status strip.

## Verification

Covered today (all with `FakeConnector`, no network):

- start agent updates status/summary/changed files through the pane
- approval request surfaces in the scene and the status strip
- approve resolves and the script continues; reject records the rejection
- direct-key reject while focused
- stop agent tears down the live session
- kill-then-restart runs under a new generation/token; buffered stale events
  are dropped ([L3-GATE])
- a failed relaunch keeps the previous session live and authoritative; the
  pane's generation never diverges from the accepted-event generation
  ([L3-GATE])
- durable intent (including approval history) survives a save/restore round
  trip; restore invents no live runtime
- restore and OpenProject detach live-session claims (running/waiting,
  pending approval ids) instead of persisting them as actionable truth
- live agent runtime state never serializes with workspace intent

Connector-side (no live Claude process):

- the approval bridge fails closed on its own clock: a stalled listener
  times out into a deny (`stalled_listener_times_out_into_a_deny`), and the
  bridge's argv[2] verdict bound is written strictly under the hook timeout
- child death (stdout EOF without shutdown) denies a connected bridge
  immediately and the listener thread exits
  (`child_death_denies_a_waiting_bridge_without_shutdown`)
