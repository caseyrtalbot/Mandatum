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
