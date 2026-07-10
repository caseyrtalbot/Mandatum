# Decisions

## Format

Each decision should record:

- status: proposed, accepted, or rejected
- decision
- context
- rationale
- consequences
- verification impact

## Accepted: Engine And Frontend Separation

Status: accepted

Decision: Mandatum is structured as a workstation engine, runtime engine,
terminal engine, scene layer, workflow layer, command layer, and frontend
adapters.

Rationale: This keeps durable product behavior testable and lets the product
support terminal, native, GPU-backed, or platform-specific frontends without
duplicating session logic.

Consequences:

- frontend adapters render scenes and emit input
- product behavior belongs in engine/runtime modules
- `core` remains free of runtime, parser, and frontend dependencies
- scene types become the central interface for presentation

Verification:

- architecture boundary scans
- scene/frontend tests once scene types exist

## Accepted: Durable Intent Is Separate From Live Runtime

Status: accepted

Decision: Workspace persistence stores durable intent only. Live PTYs, parser
instances, process handles, runtime tokens, thread handles, output buffers, and
frontend resources are runtime state.

Rationale: Durable state must survive restarts without pretending that live
processes can be serialized.

Consequences:

- restore can recreate useful layout and command intent
- side-effecting work requires explicit relaunch policy
- events from replaced runtimes must be rejected

Verification:

- saved JSON exclusion tests
- restore transaction tests
- replaced-runtime event rejection tests

## Accepted: Agents Are Session Actors

Status: accepted

Decision: Agents are represented as session actors with objective, state,
approvals, changed files, commands, checks, blockers, and handoff data.

Rationale: Agent state should be visible alongside terminals and tasks without
turning the product into a chat-first surface.

Consequences:

- agent panes need compact state, detail expansion, and global attention signals
- approvals are first-class runtime events
- changed files and checks attach to the agent actor

Verification:

- agent pane state tests
- approval attention tests
- restore-with-agent-intent tests

## Accepted: Terminal Quality Lives Behind The Terminal Engine

Status: accepted

Decision: Terminal parser/backend choices stay behind the terminal engine
interface.

Rationale: Terminal correctness matters, but the workstation product should not
inherit another terminal emulator's application architecture.

Consequences:

- backend swaps require conformance tests
- parser details do not leak into `core`
- frontend adapters consume snapshots, not parser internals

Verification:

- terminal conformance suite
- backend fixture parity
- dependency boundary checks

## Accepted: Apache-2.0 License

Status: accepted (2026-07-09)

Decision: The repository is licensed Apache-2.0.

Rationale: Standard permissive license for the Rust ecosystem with an
explicit patent grant. The repo is pre-release; relicensing before any
public release remains possible, so this is a low-cost reversible default.

Consequences: LICENSE at repo root; contributions inherit it.

## Accepted: One Gate Script For Local And Remote CI

Status: accepted (2026-07-09)

Decision: `ci/gate.sh` is the single source of truth for the merge gate
(fmt, clippy -D warnings, build, test, conformance, doc-trace). GitHub
Actions (`.github/workflows/ci.yml`) runs exactly that script.

Rationale: Local runs and CI cannot drift if they execute the same script.
Constitution laws are executable gates: L1/L2 as dependency scans
(`ci/conformance.sh`), L3/L4/L5 as `[Lx-GATE]`-tagged tests, and
`ci/doc-trace.sh` fails the build if any law loses its docs or its gate.

Consequences: a merge that reddens a conformance gate does not land.

## Accepted: Commit Directly To main

Status: accepted (2026-07-09)

Decision: This solo repository commits directly to main, gated by
`ci/gate.sh` before each push, matching the repo's existing history.

Rationale: No collaborators; the gate script provides the protection a PR
flow would. Revisit when a second contributor appears.

## Accepted: Scene Lives In Its Own Engine-Side Crate

Status: accepted (2026-07-09)

Decision: The renderer-neutral frontend contract lives in a new
`mandatum-scene` crate: the full `WorkspaceScene` output model (geometry,
pane content, terminal cells, overlays, status, attention, hit targets)
and the neutral input model frontends translate into. It depends on
`mandatum-core` and serde only, and is listed as an engine-side crate in
the L1 conformance gate.

Context: `WorkspaceScene`/`PaneScene` currently live inside the ratatui
renderer and use ratatui geometry types, so the "scene" is owned by one
frontend — exactly what L1 forbids.

Rationale: Core stays durable-intent only (scene is ephemeral presentation
state, so it does not belong in core). Terminal cells are re-expressed as
neutral scene cell types rather than importing `mandatum-terminal-vt`,
because that crate carries the `vte` parser dependency (L4: no parser type
leaks past the terminal engine).

Consequences: frontends (ratatui today, GPU tomorrow) consume scenes and
emit neutral input events; per-frame grid conversion is an accepted cost
until damage tracking is needed.

## Accepted: Agent Runtime Uses Threads And Channels, Not An Async Runtime

Status: accepted (2026-07-09)

Decision: `mandatum-agent-runtime` uses OS threads and std channels,
mirroring the PTY runtime. No tokio/async-std anywhere in the workspace.

Rationale: The workload is a handful of subprocess streams, not thousands
of sockets. Threads keep the dependency tree small, match the existing
runtime architecture, and keep the L1 forbidden-crate list enforceable.

## Accepted: Approval Gate Via Connector-Side Permission Bridge

Status: accepted (2026-07-09)

Decision: The reference agent connector runs Claude Code headless
(`claude -p --output-format stream-json`) with a generated settings file
whose PreToolUse hook calls a Mandatum bridge. The bridge blocks on a Unix
socket until the workstation user approves or rejects, then returns the
hook permission decision. The connector protocol itself stays
model-agnostic: any connector that can emit `ApprovalRequested` and accept
a decision fits the trait.

Evidence (probe, 2026-07-09): headless `claude -p` with a deny-returning
PreToolUse hook streamed the tool_use event with the full command, blocked
execution, and surfaced the deny reason in the result stream. Hook input
carries tool_name, tool_input, cwd, and tool_use_id — enough to render
command/scope/risk in the approval surface.

Consequences: approvals are enforced at the connector boundary (the agent
process cannot bypass the gate); hook timeout is set high and a timeout
maps to rejection; a FakeConnector provides deterministic approval flows
for tests and red-team runs.

## Accepted: Scene Output Contract Adopted; Neutral Input Wiring Deferred To The Pointer Outcome

Status: accepted (2026-07-09)

Decision: `mandatum-scene` now owns the full output contract — the
`WorkspaceScene` model (geometry, styled terminal surfaces, pane content,
overlays, header/status, hit targets) plus all pane-rect layout math in
`scene::layout`. The app builds the scene each frame (`scene_builder`
converts terminal-engine grids into neutral surfaces app-side), and
`mandatum-renderer` is reduced to one ratatui adapter with a single
`render(frame, &scene)` entry point and no direct terminal-engine
dependency. The neutral input types (`scene::input`: keys, pointer events,
paste, resize, focus) ship as types only; the app keeps consuming crossterm
events directly.

Rationale: the drawing-side seam lands first because it unblocks GPU
frontends and the visibility surfaces immediately and is provable today
(the frontend-parity test renders one real session scene through both the
ratatui adapter and a plain-text frontend). Input neutrality lands with
mouse support, which forces the event-translation layer anyway — wiring it
now would add a translation shim with no consumer.

Consequences:

- frontends depend on `mandatum-scene` alone and never compute layout
- the L1 gate additionally bans a direct `mandatum-renderer` ->
  `mandatum-terminal-vt` dependency
- split-separator hit targets are deliberately absent until drag-to-resize
  (the percentage layout has no separator cells)
- per-frame grid-to-surface conversion remains the accepted cost until
  damage tracking is needed

Verification: scene layout parity tests (geometry captured from the
previous ratatui math), scene-builder content tests, renderer TestBackend
tests, and the cross-frontend parity test in
`crates/app/tests/frontend_parity.rs`.

## Accepted: Agent Runtime Contract

Status: accepted (2026-07-09)

Decision: `mandatum-agent-runtime` (engine-side; deps: `mandatum-core`,
serde, serde_json) owns the connector contract. `AgentConnector::launch`
takes an `AgentLaunchSpec` (objective, cwd, model hint, approval policy —
default gates shell commands, auto-allows reads) and returns an
`AgentSession`: a `std::sync::mpsc::Receiver<AgentSessionEvent>` plus a
boxed `AgentSessionControl` (decide / interrupt / shutdown / is_alive).
Approvals are first-class events: `ApprovalRequested` carries an approval
id, the verbatim command, its scope (cwd + affected path), and a
connector-side heuristic `RiskAssessment` (Low/Medium/High + basis); the
workstation answers through the control handle with an `ApprovalDecision
{ approval_id, Approved | Rejected { reason } }`.

Context: durable agent intent (`mandatum_core::AgentPaneIntent`) already
exists. Connectors need a runtime shape that never leaks into persistence
(the durable-intent law) and never drags a frontend or async runtime into
engine crates (L1).

Rationale: threads plus std channels mirror the PTY runtime
(`crates/app/src/process_events.rs`) — one worker thread per agent
stream, events drained into the app loop; no tokio/async-std anywhere in
the workspace (see "Agent Runtime Uses Threads And Channels, Not An Async
Runtime"). Both traits are object-safe so the app can hold heterogeneous
connectors behind trait objects, and `FakeConnector` scripts
deterministic happy and pathological flows (double-decide,
decide-after-shutdown, event floods) for tests without a live agent.

Consequences:

- `AgentSession` is runtime state: never serialized; the durable subset
  of events folds into `AgentPaneIntent` app-side
- risk levels are advisory heuristics only; the approval gate itself is
  the enforcement point, and Low never means auto-approve
- `mandatum-agent-runtime` joins the ENGINE_SIDE list in the L1
  conformance gate

Verification: FakeConnector unit tests (happy path, approve and reject
branches, wrong-id decide, double-decide, decide-after-shutdown, shutdown
mid-script closes the receiver, is_alive semantics, 10k-event flood),
risk-heuristic banding tests, event JSON round-trip, and the L1/L2
dependency scan in `ci/conformance.sh`.

## Accepted: Agent Runtime Registry Mirrors The PTY Runtime Discipline

Status: accepted (2026-07-09)

Decision: Live agent sessions are integrated through an
`AgentRuntimeRegistry` in `crates/app/src/agent_runtime.rs` that mirrors
`task_runtime.rs` / `process_events.rs` exactly: one forwarder thread per
live session pumps `AgentSessionEvent`s into the app event loop wrapped as
`AgentRuntimeEvent { pane_id, restart_generation, runtime_token, event }`,
and `app_state` applies an event only if the pane's current generation and
token match — anything else is dropped. Agent events travel on their own
`std::sync::mpsc` channel drained by the same `tick_runtime` pass that
drains PTY events, keeping the existing `PtyRuntimeEvent` type untouched.

Rationale: the (generation, token) stamp is the workspace's proven L3
mechanism for rejecting events from replaced runtimes; reusing it verbatim
means one discipline to audit instead of two. A relaunch of a live agent
bumps the pane's restart generation (like Restart Pane) and always takes a
fresh runtime token, so a killed session's buffered events can never match
again.

Consequences:

- registry state (control handle, forwarder join handle, current action,
  ~200-line output tail, full pending `ApprovalRequest`) is live-only and
  never serialized
- the durable subset of events folds into `AgentPaneIntent` at the moment
  an event is accepted; a stale event therefore cannot touch durable intent
- `[L3-GATE]` tags: `stale_agent_events_after_restart_are_ignored` and
  `agent_runtime_state_is_not_serialized_with_workspace_intent` in
  `crates/app/src/app_state.rs`

Verification: FakeConnector-driven app tests (start / approve / reject /
stop / restart / save-restore round trip), scene-builder assertions for the
approval surface and status strip, no network anywhere.

## Accepted: Approval History Persists In Durable Agent Intent

Status: accepted (2026-07-09)

Decision: decided approvals are appended to
`AgentPaneIntent.approval_history` as `AgentApprovalRecord { approval_id,
command, approved }` (oldest first), and the currently-pending approval is
durable only as a count plus id list (`pending_approvals`,
`pending_approval_ids`). The full `ApprovalRequest` detail — scope, risk
band, risk basis — stays in the live registry and dies with the session.

Rationale: past decisions are execution history the user must be able to
audit after a restart ("what did I let this agent run?"), so they are
durable facts: the id, the verbatim command, and the verdict. Scope and
risk are advisory context computed for the moment of decision; persisting
them would freeze a heuristic as durable truth. The pending id list lets a
restored workspace say *which* approval was interrupted without pretending
the gated action is still decidable — restore invents no live runtime, so
a pending approval at save time restores as an unresolved id with `unknown`
status once the session is gone.

Consequences:

- `AgentPaneIntent` gained `pending_approval_ids` and `approval_history`
  (both `#[serde(default)]`, so pre-existing workspace files still load)
- history grows without bound for now; a cap becomes a real decision when
  long-running agents make files noticeably large
- the save/restore round-trip test asserts decided approvals remain
  visible after restart


## Accepted: GPU Frontend Spike Verdict — Terminal Frontend Stays v1

Status: accepted (2026-07-09)

Decision: The winit+wgpu frontend spike (spikes/frontend-wgpu) proved
feasibility and a measured latency win (key-to-GPU-present p50 21.6 ms vs
the TUI's key-to-bytes-out p50 42.9 ms, an understated >2x gap), rendering
purely from the mandatum-scene contract as a second conforming frontend.
The terminal frontend nevertheless remains v1.

Rationale: A large share of the measured gap is the product's own 40 ms
input poll loop, which the terminal frontend can cut without any GPU work
(queued for the brilliance pass); and a production GPU adapter still owes
substantial work the spike skipped (full multi-pane/overlay scene binding,
grapheme widths, IME, DPI, surface-loss recovery, damage tracking). The
gains become decisive only when the roadmap needs GPU-only capability or
sets sub-20 ms end-to-end latency as a goal.

Consequences: the adapter stays warm behind the scene contract with its
measurement harness (tui_probe) reusable for latency regressions; evidence
in spikes/frontend-wgpu/RESULTS.md.
