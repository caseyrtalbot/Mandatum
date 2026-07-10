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

## Accepted: Neutral Input Wiring Landed At The Frontend Boundary

Status: accepted (2026-07-09)

Decision: the app consumes `mandatum_scene::input` values exclusively.
`AppState::handle_event` takes `InputEvent`; key routing, palette
resolution, copy mode, and dispatch all operate on the neutral `Key` type.
The terminal frontend translates crossterm Key/Mouse/Paste/Resize/Focus
events into neutral values in `crates/app/src/frontend.rs`, at the
`app_shell` event loop. Pointer events resolve against the last built
scene's hit targets; children that request mouse reporting (DECSET
9/1000/1002/1003, tracked behind `TerminalAdapter::mouse_mode`) get pointer
events forwarded to their PTY instead of workspace handling, with alt+click
and copy mode as the explicit workspace overrides ([L5-GATE] tests in
`app_state`).

Enforcement choice: the seam is inside one crate, so the L1 dependency scan
cannot see it. `ci/conformance.sh` adds an `[L1-GATE]` source scan instead:
inside `crates/app`, only `app_shell.rs` and `frontend.rs` may use crossterm
(imports or `crossterm::` paths). Module-level enforcement via a separate
frontend crate was considered and rejected for now: it would force the
event-loop/PTY/render coordination apart before a second frontend exists.

Consequences: a native or GPU frontend plugs in by writing its own
translation to `InputEvent`; the 37+ app-state tests now speak neutral
input via `Key::plain`/`Key::ctrl` helpers.

## Accepted: Config Files, Remappable Keymap, And Semantic Themes

Status: accepted (2026-07-09)

Decision: `~/.config/mandatum/config.toml` (honoring `XDG_CONFIG_HOME`)
overlaid by `<project>/.mandatum/config.toml` (project wins), validated at
the boundary (`crates/app/src/config.rs`): unknown keys, bad chords, and
bad colors each produce a status-line warning naming the exact problem and
the affected setting keeps its default — a broken config never blocks
launch. Sections: `[keymap]` (global chords per command, kebab-case names
from the `BUILT_IN_COMMANDS` table, modifier required so bare keys never
steal terminal typing — L5), `[keymap.palette]` (single letters),
`[theme]` (named built-in — mandatum-dark / mandatum-light /
mandatum-high-contrast — plus per-role color overrides), `[ui]`
`reduced_motion`, `[shell] program`, `[task] default_command`,
`[agent] connector/model`. Conflicts: later binding wins, with a warning.
"Reload Config" (palette `e`) re-reads config live.

Theme placement: the scene stays color-semantic (`AgentContent` gained
`status_role`); the `Theme` type (neutral `SceneColor` roles, defined in
`mandatum-scene`) is resolved to concrete paint colors only in the
frontend adapter (`mandatum-renderer`). Keymap defaults live as data in
one place: the `name`/`palette_key` columns of `BUILT_IN_COMMANDS`.

Consequences: every `CommandId` is remappable; palette entries display
their bound letter and chord; `render()` takes `&Theme`; the default
theme reproduces the pre-theme output exactly.

## Accepted: Fuzzy Palette With First-Keystroke Fast Paths

Status: accepted (2026-07-09)

Decision: the palette is a real fuzzy command palette. Ctrl+P opens an
input field; typing filters all commands by a hand-rolled case-insensitive
subsequence scorer (`mandatum_commands::fuzzy`: DP over query x label with
word-boundary, prefix, and contiguous-run bonuses and a linear gap
penalty, returning matched char indices for highlighting). Ranking adds a
small context bonus so commands matching the focused pane kind lead;
impossible commands stay listed but greyed with the reason in the detail
text. The scene's `PaletteOverlay` carries query, entries (label, detail,
live key hint, match indices, enabled), selection, and a footer;
`layout::palette_item_window` is the shared scroll-window math so drawn
rows and `PaletteItem` hit targets can never disagree.

Fast-path resolution: with an empty input, the first keystroke goes
through `resolve_palette_key` unchanged — bound letters dispatch (task
substitutions included), `q` quits, Tab/BackTab cycle focus — preserving
the existing muscle memory exactly. The ambiguity with typed queries is
resolved by two escape hatches: unbound letters seed the filter, and
Shift+letter always seeds the filter. While the palette is open Ctrl+N and
Ctrl+P are fixed selection keys (Ctrl+P therefore navigates rather than
toggling; Esc closes; a non-default toggle chord still closes).

Consequences: palette key routing moved out of `crates/app/src/input.rs`
into the palette model (`crates/app/src/palette.rs` + `app_state`);
`RuntimeInput` lost its palette variants; command labels are verb-first
sentence case ("Split pane right").

## Accepted: Pointer Support Reuses The Copy-Mode Viewing Model, Not The Mode

Status: accepted (2026-07-09)

Decision: pointer scrollback and selection reuse copy mode's data model —
absolute buffer coordinates through the same viewport windowing and the
same `selected_text` extraction — without entering the copy-mode modal
keymap. A separate `PointerView` (per-pane wheel scroll offset plus an
anchor/cursor selection) feeds `pane_view_state`; copy mode wins when both
exist. The alternative, entering full copy mode on wheel or drag, was
rejected because it silently steals subsequent typing from the child
terminal (L5): pointer viewing must leave the keyboard path untouched.

Routing: pointer events resolve against the last built scene's hit
targets, emitted bottom-up (status, tiled panes, split separators,
floating panes, overlay rows) and scanned in reverse so the topmost
surface wins. Split separators carry the preorder split index that
`mandatum_core::Layout::set_split_percent` addresses, making drag-resize
durable layout intent (`CoreAction::SetSplitRatio`, clamped 5–95%), and
float moves land as `CoreAction::MoveFloatingPane`.

Terminal soul: `TerminalAdapter::mouse_mode` exposes the child's DECSET
9/1000/1002/1003(+1006 SGR) request; while tracking is on, pointer events
over that pane's grid are encoded (SGR or legacy X10) and written to its
PTY — no focus steal, button gestures stay with the pane that received
the press. Explicit workspace overrides: alt+pointer, copy mode, and the
pane chrome (titles, separators, status, overlays), which is never the
child's surface. The right-click context menu lists pane-relevant
commands with their keyboard routes and is keyboard-navigable and
clickable; Esc dismisses.

## Accepted: Execution Timeline Is Append-Only JSONL With Two-File Rotation

Status: accepted (2026-07-09)

Decision: durable execution facts append to
`<project>/.mandatum/timeline.jsonl`, one JSON object per line:
`{"at_ms": <unix epoch millis>, "event": "<kind>", ...fields}` (an
internally tagged serde enum, `crates/app/src/timeline.rs`). Recorded
kinds: command_dispatched, task_started, task_exited (command + exit
status), agent_status, approval_requested (command/scope/risk),
approval_decided (verdict + decided_by), agent_objective_set,
agent_launch_refused (reason — refusal previously left no durable trace),
workspace_saved/restored, pane_created/closed, config_reloaded.

Write discipline — the documented deviation from the temp+fsync+rename
convention in `persistence.rs`: appends are `O_APPEND` writes of one
complete line, without per-line fsync. A single-writer audit log cannot
corrupt previous lines this way, a torn final line is skipped and counted
by the reader, and per-event fsync would tax every dispatch. Symlink and
non-regular-file rejection mirror the persistence module; reads are capped
(4 MiB) and malformed lines are skipped with a visible count, never a
crash.

Rotation: before an append, a file at/over 2 MiB is renamed to
`timeline.1.jsonl` (replacing any previous rotation) and a fresh file
starts — at most two files ever exist, and the overlay's tail read (last
~500 events) stitches the rotated file in when the active one is short.
Repeated rotation drops the oldest window by design.

L3: the event types hold plain strings and numbers copied from durable
facts; no live handle, token, or socket path exists on them, so
serialization excludes runtime state by construction.

Consequences: the timeline is evidence, not truth — the workspace file
remains the durable source of intent; a concurrent second process could
lose a rotation race (accepted for a single-writer workstation log).

## Accepted: The Header Is a Scene-Carried Attention Strip

Status: accepted (2026-07-09)

Decision: `WorkspaceScene` now carries fully composed chrome:
`HeaderScene` gained its area, the composed strip text, the workspace
name, the connector label, and `attention: Vec<AttentionSegment>` (label,
resolved rect, jump pane); `status` became `StatusScene { area, text }`.
Frontends paint scene text at scene rects and restyle attention segments
in the theme's attention color — closing the WF2 finding that frontends
derived header/status content and areas themselves. `&WorkspaceScene`
alone suffices to paint a frame.

Attention aggregation (in `crates/app/src/attention.rs`, severity order):
approvals waiting (count + first pane), failed tasks (count + first
pane), blocked/failed agents (count). Segments are hit targets
(`HitTargetKind::AttentionSegment` carries the jump pane); when calm the
strip shows session facts (session name, pane count, connector kind) —
never blank, never noisy.

Verification: attention aggregation tests in the scene builder, the
segment-restyle renderer test, the attention click test in `app_state`,
and the cross-frontend parity tests, which now assert the header text and
attention segments survive both frontends.
