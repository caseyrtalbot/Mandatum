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
`AgentRuntimeRegistry` Implementation in `crates/app/src/agent_runtime.rs`
that mirrors `task_runtime.rs` / `process_events.rs`: one forwarder thread per
live session pumps `AgentSessionEvent`s into the unified app event channel
wrapped as `AgentRuntimeEvent { pane_id, restart_generation, runtime_token,
event }`. `RuntimeEngine` accepts an event only if the pane's current
generation and token match — anything else is dropped — then returns the
durable event for `AppState` to fold. The existing `PtyRuntimeEvent` type stays
untouched.

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

Maintenance addendum (2026-07-14): scene-contract compile drift in the excluded
spike was repaired, and `./ci/gpu-spike.sh` now provides an explicit opt-in
format, locked all-target test, and structural renderer-boundary check. The GPU
paint path is a separate spike-local crate whose dependency tree cannot reach
PTY or parser packages. Heavy GPU frontend
dependencies remain outside the product workspace/build/release and merge gate;
the merge gate instead fails closed if a listed GPU frontend dependency enters
a production member before an accepted decision has either a typed pixel-native
scene surface with executable adapter tests, or a sub-20 ms key-to-present
product target with symmetric end-to-end evidence. The dependency list is a
known-stack tripwire, not an exhaustive taxonomy.
Conformance resolves all workspace features and separately allowlists the two
release package/binary pairs, archive members, and installer binaries, so an
optional dependency or excluded-manifest release cannot silently bypass the
admission decision.
Neither production trigger is met: no roadmap item requires a GPU-only or
pixel-native surface, and sub-20 ms end-to-end latency is not a stated product
goal. The current terminal refresh (p50 11.30 ms / p95 13.08 ms) is
key-to-bytes-out only, with host-terminal paint excluded.

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
`[agent] connector/model`, `[font] family/size` (native only, read once at
launch, CLI flag > config > built-in default). Conflicts: later binding
wins, with a warning. "Reload Config" (palette `e`) re-reads config live,
except `[font]`, which the resolved font atlas fixes at startup.

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

## Accepted: Event-Driven Main Loop With Heartbeat And Redraw Cap

Status: accepted (2026-07-09)

Decision: the terminal frontend's run loop (`crates/app/src/app_shell.rs`)
is event-driven. A dedicated input thread — frontend-layer, the only new
code that names crossterm — reads events, translates them to neutral
`mandatum_scene::input` values, and forwards them over the app's unified
event channel. PTY reader and agent forwarder threads send into the same
channel (`crates/app/src/events.rs`: `AppEvent::Input | Pty | Agent`), so
the main loop has exactly one blocking wait (`mpsc::recv_timeout`) and can
never miss a wake. Three constants govern cadence: a 250 ms heartbeat
(child-exit polling and clock-driven UI when nothing arrives), an 8 ms
redraw cap (~120 fps: under a flood the loop keeps absorbing events —
blocking between arrivals, never spinning — and repaints at most once per
interval), and a 100 ms input-thread stop-flag check (bounds shutdown
latency only; `event::poll` wakes the instant an event arrives).

Context: the previous loop woke on a fixed 40 ms `event::poll`, taxing
every keystroke with up to one poll interval before it was even read. The
GPU spike measured the cost: key-to-bytes-out p50 42.9 ms, with roughly
half attributed to the poll loop (see "GPU Frontend Spike Verdict", which
queued this fix).

Rationale: one unified channel instead of a per-source waker keeps the
wake path race-free with plain std mpsc (no async runtime — see "Agent
Runtime Uses Threads And Channels"). The heartbeat replaces the poll as
the only periodic work, so idle cost drops instead of rising. The redraw
cap bounds worst-case paint work under PTY floods and 1000 Hz pointer
drags while costing an isolated keystroke nothing (its first repaint is
immediate; only the echo repaint can wait out the remainder of the 8 ms
window). Burst draining before each draw is preserved for drag
responsiveness. L5 is untouched: the input thread only moves where events
are *read*; routing still happens in `app_state`.

Consequences:

- measured on the external probe: key-to-bytes-out p50 42.6 ms -> 13.3 ms
  (p95 44.1 -> 15.0, max 45.5 -> 15.3); idle CPU 0.4% -> 0.1% over 30 s
- `AppState` owns the channel; `event_sender()` hands the send side to the
  frontend; `wait_event`/`drain_events`/`poll_child_exits` are the loop's
  primitives, and `tick_runtime` (drain + child poll) keeps its test-facing
  semantics
- child exits surface within one heartbeat (~250 ms) instead of ~40 ms —
  acceptable for a status line
- the app quits ~100 ms after the final keystroke at worst (input-thread
  join), imperceptible at exit
- the latency floor now sits at echo round-trip plus the redraw window;
  cutting deeper means lowering the cap or skipping the input-triggered
  draw, neither needed today

Verification: `docs/verification.md` "Input Latency Regression Check" (the
tui_probe procedure and the before/after table); the full app test suite
passes unchanged, including the [L3-GATE] stale-event and [L5-GATE]
routing tests.

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
strip shows session facts (session name, pane count) — never blank,
never noisy. Amended 2026-07-26: the connector kind appears only while
the session contains an agent pane. The label is configuration, not
activity; a workspace of plain terminals (which may host any CLI,
including other vendors' agents) must not claim an agent connector.

Verification: attention aggregation tests in the scene builder, the
segment-restyle renderer test, the attention click test in `app_state`,
and the cross-frontend parity tests, which now assert the header text and
attention segments survive both frontends.

## Accepted: Session Search Runs Over An Open-Time Snapshot

Status: accepted (2026-07-09)

Decision: "Search session output" (`CommandId::SearchSession`; default
chord `ctrl+shift+f`, the fuzzy palette, every pane's context menu — no
palette letter, deliberately: binding the last free letter would end
bare-letter filter seeding in the empty palette) searches
across the active session's live terminal grids (scrollback+screen via
the existing grid text APIs, app-side — the scene crate gains no engine
dependency), task output grids, agent output tails, and the
execution-timeline tail. The engine (`crates/app/src/search.rs`)
snapshots that text once when the overlay opens; each keystroke filters
the snapshot with the timeline's query grammar (plain tokens
fuzzy-subsequence-match with highlight indices; `pane:` /
`kind:(terminal|task|agent|timeline)` filter structurally; tokens AND).
Results group by source in pane order (timeline last), most recent first
within a group, capped at 200 with an honest "+N beyond cap" count.

Rationale: the snapshot is what makes search calm under load — a
flooding pane cannot reshuffle results mid-read, and the flood test
asserts exactly that. A per-keystroke live re-index was rejected as a
latency tax with jitter for no workflow gain (reopen re-snapshots).
Subsequence matching reuses `mandatum_commands::fuzzy` for consistency
with the palette and timeline; a cheap linear pre-check gates the DP
scorer so only the ≤200 displayed hits pay for highlight indices. The
command label says "Search session output" because that is what it is —
exact/fuzzy text search, not embeddings.

Consequences:

- Enter on a terminal hit focuses the pane and scrolls its viewport to
  the matched row through the pointer-view mechanics (no copy-mode modal
  keymap, so typing keeps flowing to the child — L5); the matched span
  renders as a selection. Jumps verify the row still holds the matched
  text and say "output moved since the search snapshot" when the bounded
  scrollback (2000 rows) has evicted or shifted it, instead of pretending.
- Task output and agent tails have no scrollable viewport; focus is the
  jump there. Timeline hits open the timeline overlay positioned at the
  matched entry.
- The default `ctrl+shift+f` chord never collides with readline's
  `ctrl+f`: chord matching requires the shift modifier, so terminals that
  cannot report it simply never produce the chord.

Verification: engine unit tests (grouping/recency, filter grammar, cap
and overflow honesty, zero-hit calm, grid snapshot coverage, jump-offset
math, pre-check/matcher agreement), app tests (open/Esc, chord and menu
routes, timeline positioning, live scrollback jump with a real PTY,
flood-stability with a rolled scrollback ring, agent-tail hits and
`pane:`/`kind:` narrowing, clickable rows), a scene-builder hit-target
test, a renderer test, and the search arm of the cross-frontend parity
text renderer.

## Accepted: PTY Backpressure Via Flow Credits Plus A Bounded Drain

Status: accepted (2026-07-09)

Decision: two bounds make the event loop calm under a PTY flood. (1) Each
PTY reader thread owns a flow gate (`PtyFlowControl`,
`crates/app/src/process_events.rs`): it must acquire a credit for every
chunk before sending, blocks at 64 KiB in flight per pane — leaving the
flooding child blocked in the kernel pipe instead of ballooning the app
heap — and each credit travels with its event and releases on drop, so
applied, stale-rejected, discarded, and channel-torn-down events all
return capacity. `stop()` aborts a blocked acquire before the reader
join, so shutdown and Stop task can never deadlock on a full gate. (2)
`drain_events` applies at most 256 events per call, so a producer that
outruns the consumer cannot pin the main loop inside the drain and starve
the draw/redraw-cap checks.

Context: the brilliance-pass external probe showed the previous unbounded
pipeline wedging the whole workstation under `yes`: zero repaints, RSS
3.8 GB in 12 s, quit requiring SIGKILL — despite the "Event-Driven Main
Loop" decision's claim that a flood "repaints at most once per interval".
That claim only became true with these bounds.

Consequences:

- worst-case queued PTY memory is 64 KiB per pane plus one chunk
- input events use the later priority-lane decision, so the quit chord and
  typing do not wait for even this bounded runtime backlog
- a finite flood drains at full parser speed; only an infinite producer
  is throttled, and it throttles in the child, not the workstation

Verification: `process_events` gate unit tests (blocks at capacity,
release unblocks, stop aborts), `drain_events_bounds_work_per_call`, and
`pty_flood_stays_bounded_responsive_and_quittable` — a live `yes` flood
asserting bounded in-flight bytes, quit within two seconds, and a
non-deadlocking shutdown join.

## Accepted: Help, First-Run, And Legends Are Generated Surfaces

Status: accepted (2026-07-09)

Decision: every orientation surface is generated from live data at the
moment it is shown, never hand-maintained text. The help overlay
(`crates/app/src/help.rs`; default chord `f1`, palette `?`, status-strip
hint, last context-menu row) composes the command table joined with the
live keymap (rebinds included), the palette fast-path rules, the mouse
gestures with the L5 alt+click override note, and the glyph legends —
filterable with the palette input pattern. The session-map and timeline
overlays append a footer legend covering exactly the glyphs on screen,
generated from the same tables (`SESSION_MAP_GLYPH_LEGEND`,
`TIMELINE_GLYPH_LEGEND`) their rows draw from; completeness tests
construct every event branch and pane kind and fail on any glyph missing
a legend entry. The first-run note (shown only when a launch that asked
to restore found no saved workspace) is eight generated lines naming the
four doors — palette chord, right-click menu, help key, quit chord — and
is not modal: any key, paste, or click dismisses it and the action still
lands; a saved workspace suppresses it forever.

Rationale: hand-written key text drifts the first time someone rebinds a
chord; generated text plus drift-failing tests make staleness a compile
or test failure instead of a stranger's confusion. F1 becomes the one
default command chord because function keys are already workspace keys
(the config boundary accepts them without a modifier) and F1 is the
universal help key; it is rebindable like any chord.

Accessibility in the same slice: Move float left/right/up/down close the
last keyboard-parity gap (pointer-only float placement); the
high-contrast theme's focus border becomes bright yellow (it was
white-on-white with only a bold modifier), with per-theme distinctness
asserted at the theme and renderer levels; and there is deliberately no
`[ui] font_scale` key — the terminal frontend inherits the host
terminal's font, so the key would be silently inert, which is worse than
the loud unknown-key warning the config boundary produces today. The GPU
adapter defines its own scaling contract when it lands.

Verification: help-generation tests (rebound chord reflected, every
command routed, L5 note present, both legends covered), first-run gating
tests (fresh dir shows the note and orienting status; any action
dismisses; a saved workspace suppresses on relaunch), glyph-legend
completeness tests in `timeline.rs`/`session_map.rs`, focus-border
distinctness tests, keyboard float-move tests, and the scene-equality
reduced-motion test.

## Accepted: The Gate Toolchain Is Pinned

Status: accepted (2026-07-10)

Decision: rust-toolchain.toml pins the exact compiler (1.96.0) for local
gates and CI alike.

Context: CI on floating "stable" advanced to 1.97 and a new clippy lint
(byte_char_slices) reddened CI while the identical local gate stayed green
on 1.96.

Rationale: the gate's guarantee is that local and CI run the same checks;
that includes the toolchain. Bumps are deliberate: update the pin and fix
any new lints in the same change.

## Accepted: Public Distribution Ships The App And Approval Bridge Together

Status: accepted (2026-07-10)

Decision: the Cargo package remains `mandatum-app`, but its explicit public
binary target is `mandatum`. Release archives and the installer always place
`mandatum-approval-bridge` beside it; the Claude connector already resolves
that sibling before falling back to `PATH`.

Context: the inferred binary name was `mandatum-app`, which leaked an internal
workspace role into the command users type. Installing only that package also
omitted the separate approval bridge, leaving the advertised agent approval
path incomplete. The project is not ready for a crates.io claim: its internal
path dependencies are intentionally workspace-local and do not carry registry
versions.

Rationale: package names organize the repository; executable names are product
interfaces. Keeping the package stable avoids churn in gates, probes, and
developer commands, while an explicit binary target gives users the single
`mandatum` entry point. Shipping the bridge in the same archive makes the
secure agent path work without a second manual discovery step.

Consequences:

- tags matching `v*` run the full gate on macOS, then Apple Silicon and Intel
  macOS builds; each architecture receives a common archive and a separate
  native archive
- every archive has a SHA-256 sidecar, and `install.sh` verifies it before
  installing into `MANDATUM_INSTALL_DIR` (default `~/.local/bin`)
- source installs remain documented as three explicit Cargo installs, one per
  executable package, because Cargo requires package selection in this
  multi-package workspace
- `cargo install mandatum` is not advertised until a separately verified
  crates.io publication decision exists

Verification: the distribution procedure in `docs/verification.md`, the full
merge gate, a disposable-root source-install smoke proving both executable
names, release-workflow archive-content checks, and an unauthenticated
latest-release installer smoke after publishing.

## Accepted: The Public Executable Has A Non-Interactive CLI Contract

Status: accepted (2026-07-14)

Decision: `mandatum --help`/`-h` and `mandatum --version`/`-V` print to stdout
and exit zero without entering terminal mode. Unknown or excess arguments
print a concise error to stderr and exit 2. No arguments retain the current
workspace launch behavior.

Context: the released executable previously treated every invocation as a TUI
launch, so ordinary package-manager, shell-discovery, and automation probes
could enter raw mode instead of returning information.

Rationale: a public developer tool needs a predictable non-interactive edge
before a larger automation API exists.

Consequences: argument parsing stays deliberately small; adding project or
recipe automation requires a separate command-surface decision rather than
silently overloading TUI behavior.

Verification: `crates/app/tests/distribution.rs` executes all four information
flags plus unknown and excess argument cases against the built public binary.

## Accepted: New Session Is Not A Project Chooser

Status: accepted (2026-07-14)

Decision: the former Open project command is exposed as New session. It
creates and focuses a fresh session inside the active project and never
duplicates that project. The old `open-project` config name resolves to New
session as a compatibility alias; `new-session` is canonical. Because pane ids
repeat across sessions, every active-session switch retires all live terminal,
task, and agent registries before reconciling the destination session.

Context: the previous command dispatched the current project name and path
back into core, which appended a duplicate project while presenting a chooser
that did not exist.

Rationale: command labels are product truth. A real project chooser needs an
explicit path-selection and runtime-reconciliation design; session creation is
already useful and accurately describes the shipped behavior.

Consequences: user bindings do not break, saved workspaces avoid duplicate
projects, a same-id pane never inherits another session's process/parser/actor,
and project selection remains honestly listed as unbuilt.

Verification: core proves project reuse and fresh session creation; command
routing proves the canonical name and compatibility alias; a live-PTY L3 test
proves New session and session-map activation each replace same-id runtime
tokens while keeping only one active shell.

## Accepted: Reload Resolves A Complete Effective Runtime Snapshot

Status: accepted (2026-07-14)

Decision: startup and Reload config share one resolution function for shell,
task command, agent connector, and model. Every reload replaces all four
effective settings, applying explicit values or product defaults.

Context: optional fields were previously assigned only when the new parsed
value was `Some`. Deleting an override or making it invalid could therefore
leave the prior value active even while the file and warning said otherwise.

Rationale: a reload is a snapshot transition, not a patch over invisible
history. One resolution seam prevents startup and reload semantics from
drifting.

Consequences: correcting or removing config takes effect immediately for
future launches; existing live runtimes are not silently restarted.

Verification: the config reload test exercises valid overrides followed by
deleted/invalid values and asserts the effective defaults and warnings.

## Accepted: Frontend Input Failure Is A Fatal, Restorative Exit

Status: accepted (2026-07-14)

Decision: the input reader reports poll/read/thread failures to the main loop.
The app stops live terminal, task, and agent runtimes, stops the reader,
restores the host terminal, and returns the original input error. A secondary
restore error never hides the primary failure.

Context: the reader previously exited silently. The heartbeat kept drawing
forever with no possible keyboard input, leaving the user trapped in the
alternate screen while child runtimes remained active.

Rationale: losing the only input channel makes the interactive session
inoperable. Exiting visibly and restoring the shell is the only honest state.

Consequences: transient frontend input failure ends the workstation session;
durable intent remains available for the next launch, while live work is not
left orphaned.

Verification: deterministic unit tests cover poll, read, stopped, and
disconnected outcomes. A lifecycle-coordinator test proves runtime shutdown,
reader stop, then terminal restore ordering and proves a secondary restore
error cannot replace the primary input failure.

## Accepted: Failed Task Evidence Becomes A Bounded Agent Mandate

Status: accepted (2026-07-14)

Decision: Investigate task failure with agent creates a new durable agent pane
from the focused task's command, resolved cwd, known failure status, and at
most the last 24 nonblank output lines capped at 240 characters each. The
workflow caps command/cwd/failure fields too, serializes all facts as JSON,
prefixes every physical evidence line, and marks the entire block as untrusted
task evidence, not instructions. The app launches it only through the
configured connector and normal approval gate.

Context: Mandatum could show, rerun, stop, and search a failure but could not
turn that evidence into the next supervised action. Keeping this assembly in
app state would also leave `mandatum-workflows` as a shallow conversion crate.

Rationale: failure-to-investigation is a high-leverage developer workflow.
The workflow Module owns the cross-actor handoff policy while the app retains
runtime facts and launch authority; that Interface preserves L2/L3 and makes
prompt-injection boundaries explicit.

Consequences: the handoff is discoverable only for a typed non-success process
exit or a launch/rerun failure. Parser, reader, resize, and wait diagnostics do
not claim a still-running child failed. Save and restore keep the mandate but
fold status to unknown and never replay the agent. Named recipe catalogs and
richer failure classification remain future work.

Verification: workflow tests prove bounds, the no-output case, and that
newlines/framing markers cannot escape the prefixed JSON evidence block;
palette and transient-error tests prove eligibility; the end-to-end app test
proves task failure, mandate content, connector approval, and honest restore.

## Accepted: RuntimeEngine Is The Deep Live-Lifecycle Module

Status: accepted (2026-07-14)

Decision: `crates/app/src/runtime_engine.rs` owns the terminal, task, and agent
runtime registries; the unified event channel; runtime token allocation and
identity checks; reconciliation, replacement, approval control, event folding,
child polling, shutdown, and transactional restore. Its production Interface
exposes product-shaped operations and observations rather than concrete
registry handles. `AppState` owns durable workspace changes, timeline entries,
status text, and presentation state by applying typed runtime effects.

Context: the earlier Gate 2 decomposition isolated three registries but left
their cross-registry lifecycle policy spread through a broad `AppState`.
Session switches, restore ordering, approval decisions, event authentication,
and replacement semantics therefore lacked one local authority. A future
recovery cockpit also needs renderer-neutral facts that say whether a runtime
was freshly created, deferred, detached, or not replayed without reconstructing
those judgments from UI strings. Restore staging failures are typed errors and
commit no lifecycle facts because no replacement occurred.

Rationale: one deep Module increases Locality and gives lifecycle replacement
one testable Seam. Terminal, task, and agent runtimes remain distinct
Implementations because their behavior is materially different; forcing them
through one generic registry abstraction would make the Interface wider and
shallower. Typed effects keep durable and presentation policy outside the live
engine, preserving L2 and L3.

Consequences: all live mutation and concrete control handles stay behind
`RuntimeEngine`; runtime tokens remain monotonic across runtime kinds; restore
is staged before existing runtimes are retired; and lifecycle facts carry a
typed epoch, trigger, session/pane target, disposition, reason, and optional
next action. The recovery cockpit and connector/control catalog remain separate
future workflows; this decision supplies a stable lifecycle boundary but does
not claim either surface exists.

Verification: runtime-engine tests prove shared token identity, stale-event
discard, transactional restore rollback, outgoing-live versus incoming-cold
classification, geometry-deferred promotion in one epoch, inactive-session
classification, valid recovery actions, and session retirement. App tests
retain the L3 stale-event, same-id session replacement, approval, task, live
PTY, and honest-restore coverage. The standard merge gate and latency probe
remain required because the unified event plumbing moved behind the Module.

## Accepted: Dark-Theme Focus Uses Bright Blue

Status: superseded (2026-07-14) by “Focus And Overlays Use Layered Chrome”

Decision: `mandatum-dark` uses ANSI bright blue (`SceneColor::Ansi(12)`) for
the focused-pane border. `mandatum-light` keeps ANSI blue, and
`mandatum-high-contrast` keeps bright yellow because its unfocused borders are
bright white. The bold border modifier and the explicit `focused` title word
remain unchanged.

Context: the dark theme's ANSI yellow focus border read as a warning-colored
frame and dominated otherwise calm terminal content. Yellow also carries the
agent-waiting semantic role.

Rationale: bright blue reads as navigation and selection, stays distinct from
red attention, yellow waiting, green running, and cyan overlay chrome, and
continues to respect each host terminal's ANSI palette instead of imposing a
fixed RGB value.

Consequences: existing user overrides remain authoritative; only the built-in
dark default changes. Focus remains redundant across color, bold weight, and
text, so the accessibility contract does not weaken.

Verification: scene-theme tests keep focus distinct from unfocused and
attention roles in every built-in theme; the renderer test asserts that the
dark focused-border cell resolves to ratatui `LightBlue`; the full merge gate
remains required.

## Accepted: First-Run Footer Composes Shared Guidance Once

Status: accepted (2026-07-14)

Decision: first-run startup status stores only the state label `new workspace`.
`scene_builder::status_text` remains the single composition point that appends
the permanent, live-keymap-derived control hint for the command palette,
right-click menu, and help.

Context: first-run startup embedded the palette and help routes in `AppState`
while scene construction appended the permanent control hint containing the
same routes. The rendered footer therefore repeated `ctrl+p commands` and `f1
help`, with terminal-width clipping sometimes hiding the second help phrase.

Rationale: status messages should describe state; permanent control guidance
should have one owner. Keeping route text in `control_hint` preserves rebind
correctness without adding string-level deduplication to the renderer.

Consequences: the first-run footer reads `new workspace — ctrl+p commands ·
right-click menu · f1 help` under the default keymap. Other status messages
continue to compose with the same hint unchanged.

Verification: the scene-level first-run regression asserts the complete
default footer and counts both the palette and help phrases exactly once; it
failed against the duplicated composition before the fix. The full merge gate
remains required.

## Accepted: Focus And Overlays Use Layered Chrome

Status: accepted (2026-07-14)

Decision: normal-width pane focus accents only the title with the theme's
`focus_title` color and bold weight; every pane perimeter uses the calm
`pane_border` role. At one-to-three-column widths, where no title content is
visible, one accented corner cell is the compact fallback. The explicit
`focused` title word remains. The former `focus_border` config key is a
compatibility alias for `focus_title`. All eight overlays share explicit
`overlay_foreground` and `overlay_background` surface roles while retaining
`palette_border` as edge chrome. The first-run scene carries an introduction,
typed key/description entries, and dismissal guidance instead of flattened
strings; the renderer accents keys, keeps descriptions normal, and dims the
dismissal line.

Context: a bright bold frame around every focused pane dominated terminal
content even after its color moved from warning-yellow to navigation-blue.
Overlays used only `Clear` plus a border, so their interiors inherited the
same terminal surface as panes and read as nested panes. The welcome card had
the right live-keymap content but no semantic structure from which a frontend
could express hierarchy.

Rationale: layered chrome should communicate navigation without competing
with the work. A focused title plus literal label is a lighter redundant
signal; an explicit overlay surface establishes depth; typed welcome entries
preserve renderer neutrality and prevent frontends from parsing whitespace.
Explicit overlay foregrounds protect contrast once backgrounds stop inheriting
the host terminal default.

Consequences: built-in dark, light, and high-contrast themes each own an
overlay surface palette. Custom themes can override the new roles. Existing
`focus_border` overrides continue to work but now color the focused title.
Legacy serialized themes accept `focus_border` and default the new overlay
roles; downstream Rust struct literals must adopt the new public fields.
The welcome structure changes the shared scene contract, so the deferred GPU
adapter fixture must stay source-compatible even though that adapter still
rejects overlays explicitly.

Verification: renderer tests assert focused-title accent/bold plus calm equal
borders in every built-in theme, the one-cell fallback at widths one through
three, key/description/dismissal hierarchy, and background containment for
every overlay variant. Scene-theme tests assert explicit overlay
foreground/background roles; app tests preserve live-keymap generation,
refuse to advertise reserved-chord shadows, migrate the legacy serialized
theme shape, preserve first-run dismissal/config compatibility, and retain
frontend parity. Run `./ci/gpu-spike.sh` for the scene-contract fixture and
`./ci/gate.sh` as the merge gate.

## Accepted: Updating Consumes A Release; Publishing Remains Tag-Driven

Status: accepted (2026-07-15)

Decision: `mandatum update` installs the latest published GitHub Release beside
the running executable, including `mandatum-approval-bridge`. It runs the
checksum-verifying `install.sh` embedded at compile time, targeting the current
executable's directory. Publishing remains a maintainer-only, version-tagged
GitHub Actions operation; there is no public `mandatum push` command. All Cargo
workspace crates inherit one root package version. The updater passes that
running version to the installer, which refuses an unidentifiable or older
release before replacing either executable.

Context: release consumers had two manual choices: rerun the remote one-line
installer or pull a source checkout and reinstall both binaries. The existing
release workflow already built and verified the correct four platform archives,
but a normal push to `main` did not—and should not silently—become a user
release. The repeated version in every crate also made a consistent version
bump needlessly error-prone.

Rationale: update and publish are different authority boundaries. A user should
need no checkout, GitHub account, or repository permission to consume a stable,
rollbackable release. A maintainer should explicitly select the version that
ships. Embedding the reviewed installer avoids downloading and executing a
mutable installer script during self-update while preserving the established
checksum, archive-allowlist, sibling-binary, and staged-replacement checks.

Consequences: installer-based and Cargo-based users can converge on the latest
prebuilt release with one command. Builds predating the command need one final
installer rerun. Updates replace the installation containing the executable;
non-writable system locations fail rather than escalating privileges. Maintainers
bump one root version, pass the gate, and push the matching annotated tag; the
existing workflow publishes the release consumed by users. A development build
ahead of the latest published tag cannot silently downgrade itself.

Verification: CLI distribution tests keep `update` visible in help, parser
tests prove it is non-interactive, updater tests prove exact install-directory
and running-version forwarding plus non-zero status propagation, and the full
merge gate checks the embedded installer and release/install artifact
allowlists. The standing post-publish smoke installs into a disposable
directory and then exercises the public update path against the latest release.

## Accepted: Shift+Tab Uses The Baseline Xterm BackTab Sequence

Status: accepted (2026-07-16)

Decision: after explicit workspace-chord resolution, neutral `BackTab` and
Shift+Tab input encode to `ESC [ Z` for the focused child. BackTab normalizes
to Shift+Tab during chord comparison so crossterm's representation still
matches a configured route such as `ctrl+shift+tab`. Mandatum does not claim
modifyOtherKeys, CSI-u, or another enhanced keyboard protocol without an
explicit capability and conformance contract.

Context: the terminal frontend already translated crossterm Shift+Tab events
to neutral BackTab input, but the child-byte encoder had no BackTab arm. The
event therefore became `Noop`, preventing terminal agents such as Codex and
Claude from receiving a common mode-cycling command. Frontend adapters can
also reasonably represent the same physical key as Tab with the Shift bit.

Rationale: L5 requires ordinary terminal input to reach the focused child.
Both neutral representations should produce the `xterm-256color` baseline
sequence that Mandatum advertises to child processes, while an explicitly
configured workspace control must retain precedence. Limiting the change to a
standard sequence avoids pretending richer modifier combinations work before
keyboard-protocol negotiation exists.

Consequences: Shift+Tab works in child TUIs and agent CLIs instead of being
dropped. Plain Tab remains `HT`. Configured workspace chords remain
authoritative and BackTab representation differences no longer make them
unreliable. Other modified special keys remain subject to the current
baseline encoder and future capability work.

Verification: the L5 input-routing test covers crossterm BackTab with Shift,
plain neutral BackTab, neutral Tab with Shift, and explicit
`ctrl+shift+tab` interception. A frontend-boundary test pins crossterm's
modifier-preserving translation. Run the app test suite, the latency procedure
in `docs/verification.md`, and `./ci/gate.sh` before completion.

## Accepted: Native GPU Capability Branch Is Selected; Production Admission Remains Gated

Status: accepted (2026-07-21)

Decision: select the capability branch, not the latency branch. The first
pixel-native capability is an Artifact Preview Pane: a task- or agent-produced
PNG screenshot, diagram, chart, or visual diff can become a reviewable
workspace pane without leaving Mandatum. The planned renderer-neutral contract
persists a project-relative `ArtifactPaneIntent`, keeps bounded decoded image
state in the app, and carries typed loading/ready/failed artifact content plus
an RGBA8 sRGB raster surface in `WorkspaceScene`. The terminal renderer must
provide a deterministic labeled fallback; the native renderer may upload the
same surface as a texture.

Context: the intended product is richer and may eventually operate without a
terminal pane. Artifact previews are a concrete non-text workstation capability
for UI-test screenshots, browser automation, diagrams, generated charts, and
visual diffs. They justify pixel-native rendering without using vague polish or
an asymmetric latency comparison as the reason. `RuntimeEngine` and
`WorkspaceScene` remain the product-state and paint boundaries; the old spike
still duplicates PTY/parser/input behavior and does not prove this capability.

Rationale: a typed artifact surface advances the workstation beyond character
cells while keeping every frontend behind the same state and scene contracts.
The terminal fallback preserves SSH/headless usefulness. Separating product
trigger selection from production dependency admission lets renderer-neutral
host and scene work proceed without silently authorizing wgpu or a release
change.

Consequences: Phase 0 product-trigger selection is complete, and Phase 1 host
extraction is authorized without native/GPU dependencies. Phase 1A now emits
FIFO `FrontendEffect::SetClipboard(String)` values from `AppState` and confines
OSC 52 encoding to `app_shell.rs`. The first artifact slice is PNG-only,
project-relative, contain-fit, bounded to 16 MiB encoded, 4096×4096 pixels, and
64 MiB decoded; path escapes, remote/active formats, malformed input, and
oversized input fail visibly. macOS arm64 is the first displayed development
reference. Native stays explicit opt-in, and terminal stays default on all four
current release targets. Fallback occurs only before live runtime creation; no
transparent mid-session process switch is promised.

Production GPU admission remains unproven. No artifact scene type, fallback
test, or excluded-GPU render-plan test exists yet; `ci/conformance.sh` and all
release allowlists remain fail-closed. A later Phase 6 decision must review
that evidence before any production GPU dependency enters. This supersedes
only the earlier “neither trigger is met” current-status addendum, not the
historical spike verdict or measurements.

Evidence correction: `docs/verification.md` owns the 2026-07-14 terminal
refresh at p50 11.71 ms / p95 13.56 ms / max 17.84 ms, 100 samples with zero
misses. Earlier 11.30/13.08 mentions in this append-only log were not the
authoritative recorded refresh. All terminal probe figures exclude host paint
and cannot satisfy the GPU admission gate.

Verification: Phase 1A tests must prove FIFO/drain-once effects, both copy
paths, restore clearing, and terminal-boundary OSC 52 encoding. `./ci/gate.sh`
remains the merge gate. The typed artifact surface later requires persistence
without bytes/resources, path/size/decode failure coverage, `WorkspaceScene`
sufficiency, a terminal fallback test, an excluded-GPU render-plan test, and
`./ci/gpu-spike.sh`. The Phase 1A release probe measured p50 11.58 ms / p95
13.35 ms / max 16.14 ms over 100 samples with zero misses, still at the
key-to-app-output endpoint. The terminal latency branch remains unselected.

## Accepted: The Shipped Terminal Frontend Exercises The Shared Host

Status: accepted (2026-07-22)

Decision: `FrontendHost` is the frontend-neutral owner of exactly one private
`AppState` and its `RuntimeEngine`. It accepts neutral input, exposes a blocking
unified-event wait and bounded nonblocking drain, performs heartbeat work when
the platform shell schedules it, returns owned `FrameSnapshot` values, drains
typed effects in FIFO order, exposes quit, and makes shutdown behaviorally
idempotent. `FrameSnapshot` carries `WorkspaceScene`, `Theme`, and a monotonic
revision that identifies snapshot production order, not semantic dirtiness.
The shipped terminal loop now uses this host for all covered state, input,
frame, effect, quit, event-drain, heartbeat, and shutdown behavior.

Context: Phase 1A proved a renderer-neutral platform effect, but
`app_shell.rs` still constructed and drove `AppState` directly. A facade used
only by tests would not prove that a second frontend can share the real state
machine. The loop also has no honest semantic dirty detector: it redraws after
event wakes and heartbeats, so a content-change revision would overclaim what
the implementation knows.

Rationale: migrating the shipped path first forces the host to carry the real
lifecycle without duplicating PTYs, parsers, commands, approvals, persistence,
or recovery. Snapshot-order revisions are sufficient to identify frames and
stay honest until profiling and a native event loop justify richer
invalidation. `FrontendHost::frame` uses `AppState::build_scene`; the terminal
requests and renders that same snapshot inside its draw callback, preserving
the exact-painted-frame hit-target rule.

Consequences: `app_shell.rs` retains crossterm, terminal guard and input-reader
lifecycle, the 250 ms heartbeat schedule, 8 ms redraw cap, ratatui rendering,
terminal effect encoding, reader join, restoration, and primary-error
precedence. Concrete runtime registries do not escape. The existing raw event
sender remains crate-private for the terminal reader only. Phase 1C must wrap
it in an app-owned sender with an optional coalesced wake callback and prove
input, PTY, and agent wake behavior. No platform waker, Artifact Preview scene
type, native window, native/GPU production dependency, or release-surface
change is admitted by this decision.

Verification: focused host tests cover owned frames and revision order,
FIFO effects, unified-channel input, the 256-event drain bound, exact-prior-
frame hit testing, and idempotent shutdown. Existing shell tests retain error
cleanup ordering and primary-error precedence. All 6 focused host tests and all
244 app library tests passed. The 2026-07-22 fresh-release `tui_probe` measured
p50 11.14 ms / p95 12.58 ms / max 13.05 ms over 100 samples with zero misses;
it remains key-to-app-output evidence only. `./ci/gate.sh` passed 463 tests with
2 intentionally ignored live-Claude-CLI tests, plus formatting, Clippy with
warnings denied, build, conformance, and doc trace.

## Accepted: Phase Completion Requires Synchronized Docs, Handoff, And Commit

Status: accepted (2026-07-22)

Decision: active-document drift is a defect. A phase or implementation slice
is complete only after its required tests pass, every affected source-of-truth
document is updated with verified facts, the project handoff records the
verified stop point and one exact next task, the final repo documentation has
passed `./ci/gate.sh`, diff/status hygiene has been inspected, and the code,
tests, and synchronized repo documentation are committed together.

Context: implementation, verification, plans, decisions, and the next-agent
handoff are one operational state. Allowing any of them to lag makes a green
build misleading and forces the next session to reconstruct which claims are
current.

Rationale: Mandatum's architecture and admission gates depend on precise
boundaries and dated evidence. Keeping documentation and handoffs inside the
same completion transaction makes the repository self-describing and prevents
completed work from being left as an ambiguous dirty worktree.

Consequences: `AGENTS.md` is the canonical operating rule. Doc sync and the
handoff are not optional follow-up tasks, and a completed slice does not stop
before its commit. Verification claims must still describe only commands that
actually ran; the gate is rerun after the final repo documentation edits.

Verification impact: every phase completion checks `./ci/gate.sh`,
`git diff --check`, `git status --short`, the current handoff, and the resulting
commit identity before reporting completion.

## Accepted: Unified Events Use One Coalesced Wake-Aware Sender

Status: accepted (2026-07-22)

Decision: `AppEventSender` is the sole send side for terminal input, PTY
readers, restore-preserved input, and agent forwarders. It preserves the one
`std::sync::mpsc` event stream as product truth and may invoke a
frontend-neutral callback when the queue changes from empty to non-empty.
Clones share queued-event and pending-wake accounting; receives pass through
the same state so consuming the final queued event and enqueueing the next one
are serialized. `FrontendHost::new_with_wake_callback` is the public injection
point. No GUI event type enters app or runtime state.

Context: the terminal loop already blocks on the unified channel, but a winit
event loop cannot block on that receiver. Exposing the raw sender or giving
each runtime source its own platform callback would either leak private event
types or create independent wake races. A plain atomic pending flag also has a
lost-wakeup window when a producer observes `pending = true` immediately
before the consumer clears it after an empty drain. The 256-event drain budget
adds another boundary: a batch ending exactly at the cap must not leave the
next enqueue silently coalesced forever.

Rationale: queue-transition accounting keeps the callback a disposable
notification while the channel owns ordering, payloads, flow credits, and
runtime generation/token stamps. One small shared lock spans channel send or
receive plus the queue count transition, closing the clear/enqueue race without
polling, an async runtime, platform dependencies, or changes to terminal-loop
timing.

Consequences: all existing producer signatures take `AppEventSender`; raw
receiver access was also removed from restore cleanup so sender accounting
cannot drift. A burst receives one wake while non-empty, every event remains
FIFO on the channel, and the next event after a full drain can wake again.
The terminal frontend still uses channel blocking and supplies no callback.
Phase 2 may bind the neutral callback to the excluded spike's event-loop proxy.
No winit, wgpu, glyphon, Artifact Preview type, production dependency, runtime
stamp, PTY flow-credit, drain-budget, heartbeat, or redraw-policy change is
accepted here.

Verification: controlled tests cover input callback plus channel truth, a
64-event burst with one callback and every FIFO event, 4,096 concurrent
send/drain events with no stranded wake, real PTY and agent producers sharing
one sender, and callback injection through `FrontendHost`. All 248 app library
tests passed. The fresh-release `tui_probe` measured p50 10.60 ms / p95 12.06
ms / max 13.38 ms over 100 samples with zero misses; as before, this is
key-to-app-output evidence and excludes host-terminal paint. `./ci/gate.sh`
passed 467 tests with 2 intentionally ignored live-Claude-CLI tests, plus
formatting, Clippy with warnings denied, build, conformance, and doc trace.

## Accepted: The Excluded Native Adapter Exercises The Real Workstation Host

Status: accepted (2026-07-22)

Decision: Phase 2 is complete. The excluded winit/wgpu adapter owns platform
windowing, GPU resources, clipboard access, event translation, paint scheduling,
heartbeat cadence, and latency instrumentation, while one
`FrontendHost`/`RuntimeEngine` owns workstation behavior. The host's coalesced
wake callback sends `UserEvent::Wake` through `EventLoopProxy`; winit keyboard,
pointer, paste, resize, and focus events cross the boundary only as neutral
`mandatum_scene::input::InputEvent` values. The renderer consumes the real
`FrameSnapshot` scene and theme and paints the real header, one terminal pane,
status strip, and command-palette overlay. Typed `FrontendEffect` values return
clipboard writes to the native shell.

Context: the feasibility spike had a parallel `TerminalSession`, a direct VT
parser dependency, a spike-local grid-to-scene bridge, duplicate terminal-byte
input encoding, and a separate `AtomicBool` wake coalescer. That architecture
proved GPU feasibility but could not prove that a native shell could operate the
real workstation state machine or share its wake, runtime, recovery, command,
and scene boundaries.

Rationale: binding the excluded adapter to the public host proves the smallest
real native workstation slice without admitting GUI dependencies into product
crates or copying product behavior into the spike. Queue-transition truth stays
inside `AppEventSender`; `EventLoopProxy` is only a disposable platform wake.
The native renderer receives product-composed chrome and palette data rather
than deriving workstation presentation from PTY state.

Consequences: `TerminalSession`, `scene_bridge`, the direct
`mandatum-terminal-vt` dependency, the duplicate key-to-byte encoder, and the
duplicate `AtomicBool` wake latch are removed. The standalone `tui_probe` keeps
its direct `mandatum-pty` dependency as a terminal latency harness; the displayed
native workstation path does not own a PTY or parser. Startup restore is
deliberately disabled for this one-terminal proof. Restore, multiple panes,
task/agent content, remaining overlays, and broader input parity stay in Phase
3. The spike remains excluded from the workspace and release artifacts.
Artifact Preview is still unbuilt, and this decision does not admit production
GPU dependencies.

Verification: the focused
`cargo test --manifest-path spikes/frontend-wgpu/Cargo.toml --test host_wake`
run passed one test proving a real host PTY wakes the callback without interval
polling and reaches a real terminal `FrameSnapshot`. `./ci/gpu-spike.sh` passed
six tests plus the renderer dependency-boundary scan. `cargo test -p
mandatum-app --lib` passed 248 tests, and the full `./ci/gate.sh` was green. The
displayed macOS smoke built with
`cargo build --release --manifest-path spikes/frontend-wgpu/Cargo.toml --bin mandatum-frontend-wgpu-spike`
and ran
`spikes/frontend-wgpu/target/release/mandatum-frontend-wgpu-spike --exit-after 120`;
`printf GPU_HOST_OK`, Ctrl+P, Escape, and Ctrl+Q exercised terminal output,
palette open/close, and clean quit, after which no native-spike or child-shell
process remained. The fresh `tui_probe` measured p50 11.39 ms / p95 12.56 ms /
max 13.69 ms over 100 samples with zero misses; that endpoint remains
key-to-app-output bytes and excludes host-terminal paint.

## Accepted: The Excluded Native Render Plan Covers Real Task And Agent Pane Content

Status: accepted (2026-07-22)

Decision: Phase 3 is underway. Its first narrow increment extends only the
excluded `spikes/frontend-wgpu` render plan to accept and paint real one-pane
`PaneContent::Task` and `PaneContent::Agent` scenes emitted by `FrontendHost`.
Task detail entries keep a one-row, tail-preserving fit and optional live output
uses the remaining scene-budgeted rows. Agent detail text wraps inside the pane
body. Header, terminal, one-pane geometry, status, theme, and command-palette
behavior remain covered.

Context: Phase 2 proved one fresh terminal slice on the shared host but rejected
task and agent content as `UnsupportedScene`. The existing scene contract
already carries the required task command/cwd/runtime/output data and agent
objective/status/action/approval/changed-file detail through
`PaneScene::detail_lines`; reaching back into app/runtime state or expanding the
scene contract would have duplicated product behavior for renderer convenience.

Rationale: preparing all three supported pane bodies from `WorkspaceScene` plus
`Theme` keeps the GPU adapter scene-only. Content-specific shaping preserves the
terminal frontend's semantics: terminal surfaces and task rows do not wrap,
task metadata retains its load-bearing tail, task output remains aligned to its
cell quads, and agent prose may wrap. Pane-body clipping and explicit row/column
bounds prevent text or surface quads from crossing chrome or status.

Consequences: no app, runtime, scene, workspace, production dependency,
allowlist, installer, default command, or release surface changes. Empty pane
content, multiple panes and broader layouts, remaining overlays, full
input/theme/style parity, restore, Artifact Preview, and production GPU
admission remain unsupported and separately gated. The next slice is Empty
content only.

Verification: real-host tests recorded the initial task and agent
`UnsupportedScene::PaneContent` failures, then passed with live task output and
agent detail retained by the prepared plan. `./ci/gpu-spike.sh` passed ten tests
plus the renderer dependency-boundary scan, and `cargo test -p mandatum-app
--lib` passed all 248 tests. Displayed release smokes showed the real task
metadata/live output and real agent state, then quit cleanly without a native or
task child process. The final merge-gate result is recorded in
`docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers The Product Empty Fallback

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and paints
a real one-pane `PaneContent::Empty` scene emitted by `FrontendHost`. The
renderer uses only `PaneScene::detail_lines` for the existing cwd, restart
generation, and no-live-PTY message, with word-or-glyph wrapping inside the
pane body. Terminal, task, agent, header, one-pane geometry, status, theme, and
command-palette behavior remain covered.

Context: the shared scene builder already emits Empty content whenever a
terminal intent has no live runtime grid, including a fresh host with PTY
spawning disabled or a product-path PTY spawn failure. The excluded renderer
still rejected that valid product scene even though every displayed fact and
its geometry were already present in `WorkspaceScene`.

Rationale: consuming the existing detail-line contract keeps the increment at
the renderer boundary and makes the same prepared value drive headless proof
and displayed paint. No Empty-specific app query, runtime handle, parser type,
or replacement presentation model is needed. Wrapping matches other
scene-composed prose and the established pane-body bounds keep it inside
product-owned geometry.

Consequences: no app, runtime, scene, workspace, production dependency,
allowlist, installer, default command, or release surface changes. Multiple
panes and broader layouts, remaining overlays, full input/theme/style parity,
restore, Artifact Preview, and production GPU admission remain unsupported and
separately gated. The next slice is the existing one-pane context-menu overlay
only.

Verification: the real-host test recorded the initial
`UnsupportedScene::PaneContent("empty")` failure, then passed with the product
Empty detail retained by the prepared plan. `./ci/gpu-spike.sh` passed eleven
tests plus the renderer dependency-boundary scan, and `cargo test -p
mandatum-app --lib` passed all 248 tests. A displayed release smoke showed the
real failed-PTY Empty state and all three detail lines, then quit cleanly with
no native or attempted-shell process. The final `./ci/gate.sh` passed after
these synchronized documentation edits.

## Accepted: The Excluded Native Render Plan Covers The Product Context Menu

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and
paints a real `OverlayScene::ContextMenu` emitted by `FrontendHost` over any
already-supported one-pane scene. The prepared plan retains the existing
resolved area, ordered labels and chord hints, and selected index. Displayed
paint uses the existing overlay background, palette border, foreground, and
selection theme roles without changing the scene contract.

Context: the app already opens the menu from neutral right-click input resolved
against the exact prior frame's pane hit targets. It already composes the
pane-relevant rows, state-aware labels, keyboard routes, clamped menu area, and
row hit targets. The excluded renderer was rejecting that complete product
scene even though no additional app or runtime data was required.

Rationale: borrowing the existing `ContextMenuOverlay` in the headless paint
plan keeps menu behavior in the app and geometry in the scene layer. The same
plan drives displayed background, border, selection, one-row labels, and
right-aligned chord hints. Matching the current scalar-character alignment is
deliberate; grapheme and wide-cell correctness remain Phase 4 work.

Consequences: no app, runtime, scene, workspace, production dependency,
allowlist, installer, default command, or release surface changes. Multiple
panes, the remaining overlay variants, full input/theme/style parity, restore,
Artifact Preview, and production GPU admission remain separately gated. The
next slice is the existing one-pane timeline overlay only.

Verification: the real-host test recorded the initial
`UnsupportedScene::Overlay("context menu")` failure, then passed with the
product menu retained unchanged by the prepared plan. The isolated renderer
test covers area, rows, selection, and right-aligned chord text.
`./ci/gpu-spike.sh` passed thirteen tests plus the renderer dependency-boundary
scan, and `cargo test -p mandatum-app --lib` passed all 248 tests. A displayed
release smoke showed the real menu over the failed-PTY Empty state, then Escape
and Ctrl+Q closed it and the process cleanly. The final merge-gate result is
recorded in `docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers The Product Timeline

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and
paints a real `OverlayScene::Timeline` emitted by `FrontendHost` over any
already-supported one-pane scene. The prepared plan retains the existing
resolved area, query, ordered glyph/time/text rows, selected index,
skipped-malformed count, and footer. Displayed paint uses the existing overlay
background, palette border, foreground, and selection theme roles without
changing the scene contract.

Context: the app already records a command dispatch before it opens the durable
timeline, reads the tail from the writable project surface, composes the filter
query and visible event window, and builds row hit targets from shared layout
math. The excluded renderer was rejecting that complete product scene even
though no additional app, runtime, or timeline-log access was required.

Rationale: retaining `TimelineOverlay` in the headless paint plan keeps durable
history, filtering, selection, glyph meaning, relative-time text, and geometry
in the app and scene layers. The same prepared data drives the displayed
background, border, title, filter prompt, selected event row, and pinned footer.
Scalar-character fitting remains deliberate here; grapheme and wide-cell
correctness remain Phase 4 work.

Consequences: no app, runtime, scene, workspace, production dependency,
allowlist, installer, default command, or release surface changes. Multiple
panes, the remaining overlay variants, full input/theme/style parity, restore,
Artifact Preview, and production GPU admission remain separately gated. The
next slice is the existing one-pane session-map overlay only.

Verification: the real-host test recorded the initial
`UnsupportedScene::Overlay("timeline")` failure, then passed with the product
timeline retained unchanged by the prepared plan. The isolated renderer test
covers area, query, rows, selection, footer, and row alignment.
`./ci/gpu-spike.sh` passed sixteen tests plus the renderer dependency-boundary
scan, and `cargo test -p mandatum-app --lib` passed all 248 tests. A displayed
release smoke showed the recorded event, live `show` filter, and bounded
`no matching events` state over the failed-PTY Empty state, then Escape and
Ctrl+Q closed it and the process cleanly. The final merge-gate result is
recorded in `docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers The Product Session Map

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and
paints a real `OverlayScene::SessionMap` emitted by `FrontendHost` over any
already-supported one-pane scene. The prepared plan retains the existing
resolved area, ordered session/pane rows, tree depth, glyph, label, live state,
focus marker, layout badges, selected index, and footer. Displayed paint uses
the existing overlay background, palette border, foreground, and selection
theme roles without changing the scene contract.

Context: the app already owns session/pane tree construction, live-state words,
focus and layout facts, selection, footer legend, centered geometry, keyboard
navigation, activation, and row hit targets. The excluded renderer was
rejecting that complete product scene even though no additional app, runtime,
workspace, or session-map model access was required.

Rationale: retaining `SessionMapOverlay` in the headless paint plan keeps
workspace visibility and navigation semantics in the app and scene layers. The
same prepared data drives the displayed background, border, title, windowed
tree rows, selected-row highlight, focus marker, state/badge text, and pinned
footer. Scalar-character fitting remains deliberate here; grapheme and
wide-cell correctness remain Phase 4 work.

Consequences: no app, runtime, scene, workspace, production dependency,
allowlist, installer, default command, or release surface changes. Multiple
panes, the remaining overlay variants, full input/theme/style parity, restore,
Artifact Preview, and production GPU admission remain separately gated. The
next slice is the existing one-pane objective prompt only.

Verification: the real-host test recorded the initial
`UnsupportedScene::Overlay("session map")` failure, then passed with the product
map retained unchanged by the prepared plan. The isolated renderer test covers
area, tree rows, depth, glyph, state, focus, badges, selection, footer, and row
alignment. `./ci/gpu-spike.sh` passed eighteen tests plus the renderer
dependency-boundary scan, and `cargo test -p mandatum-app --lib` passed all 248
tests. A displayed release smoke showed the real active session and selected
focused pane over the failed-PTY Empty state, then Escape and Ctrl+Q closed it
and the process cleanly. The final merge-gate result is recorded in
`docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers The Objective Prompt

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and
paints the real `OverlayScene::Prompt` emitted by `FrontendHost` over a
supported zoomed agent pane. The prepared plan retains the existing resolved
area, title naming the focused pane, configured objective input, and footer.
Displayed paint adds the existing block-cursor convention and uses the
semantic overlay background, palette border, and overlay foreground roles
without changing the scene contract.

Context: the app already owns prompt modality, focused-agent gating, configured
objective text, editing, save/cancel behavior, title, footer, and centered
geometry. The excluded renderer rejected that complete product scene even
though no app, runtime, agent connector, or command-model access was required.

Rationale: retaining `PromptOverlay` in the headless paint plan keeps prompt
content and behavior in the app and scene layers. The same prepared data drives
the displayed background, border, title, input, bounded cursor cell, and pinned
footer. Scalar-character cursor placement remains deliberate here; grapheme,
wide-cell, and IME correctness remain Phase 4 work.

Consequences: no app, runtime, scene, agent, production dependency, allowlist,
installer, default command, or release surface changes. Multiple panes, the
remaining overlay variants, full input/theme/style parity, restore, Artifact
Preview, and production GPU admission remain separately gated. The next slice
is the existing one-pane session-output search overlay only.

Verification: the real-host test recorded the initial
`UnsupportedScene::Overlay("prompt")` failure, then passed with the product
prompt retained unchanged by the prepared plan. The isolated renderer test
covers area, title, input, cursor cell, footer, and row alignment.
`./ci/gpu-spike.sh` passed twenty tests (two native-shell, eight real-host, and
ten isolated-renderer) plus the renderer dependency-boundary scan, and `cargo
test -p mandatum-app --lib` passed all 248 tests. A displayed release smoke
showed the real zoomed agent objective prompt, block cursor, and bounded footer,
then Escape and Ctrl+Q closed it and the process cleanly. The final merge-gate
result is recorded in `docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers Session-Output Search

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and
paints the real `OverlayScene::Search` emitted by `FrontendHost` over a
supported zoomed agent pane. The prepared plan retains the existing resolved
area, live query, grouped source labels, matched output text and char indices,
selected index, overflow, footer, and row alignment. Displayed paint adds the
existing block-cursor convention, clips base pane glyphs around the opaque
Search rectangle, and uses the semantic overlay background, palette border,
selection, and overlay foreground roles without changing the scene contract.

Context: the app already owns open-time snapshot construction, query parsing,
source grouping, match indices, result cap and overflow honesty, selection,
activation, footer, centered geometry, keyboard editing, and row hit targets.
The excluded renderer rejected that complete product scene even though no app,
runtime, Search model, or command-table access was required. Search indexes
terminal/task grids, agent runtime output tails, and timeline events; it does
not index durable agent-objective text.

Rationale: retaining `SearchOverlay` in the headless paint plan keeps Search
content and behavior in the app and scene layers. The real-host tracer bullet
uses the deterministic `search-session` timeline event beneath a zoomed agent
rather than expanding product Search semantics to satisfy an incorrect handoff
assumption about objective text. The same prepared data drives the displayed
surface, border, title, query cursor, grouped result rows, selected-row
highlight, and pinned footer. Scalar-character fitting remains deliberate;
grapheme, wide-cell, and full style correctness remain Phase 4 work.

Consequences: no app, runtime, scene, Search behavior, agent behavior,
production dependency, allowlist, installer, default command, or release
surface changes. Multiple panes, Help/Welcome and other remaining overlay
variants, full input/theme/style parity, restore, Artifact Preview, and
production GPU admission remain separately gated. The next slice is the
existing one-pane Help overlay only.

Verification: the real-host test recorded the initial
`UnsupportedScene::Overlay("search")` failure, then passed with the product
Search retained unchanged by the prepared plan. Isolated renderer tests cover
geometry, query and cursor, grouped-source elision, result text and match
indices, selection, overflow/footer state, empty states, bounded lines, and
Search-only pane-text occlusion. `./ci/gpu-spike.sh` passed 24 tests (two
native-shell, nine real-host, and thirteen isolated-renderer) plus the renderer
dependency-boundary scan, and `cargo test -p mandatum-app --lib` passed all 248
tests. A displayed release smoke showed the real zoomed agent around an opaque
Search modal with a pasted `kind:timeline search` query, selected result,
repeated-source elision, visible cursor, and bounded footer; Escape and Ctrl+Q
closed it with exit 0 and no native process left. The final merge-gate result is
recorded in `docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers Generated Help

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and
paints the real `OverlayScene::Help` emitted by `FrontendHost` over a supported
Empty pane. The prepared plan retains the existing resolved area, live filter,
ordered heading/entry rows, configured key routes, selected index, and footer.
Displayed paint adds the existing block-cursor convention, clips base-pane
glyphs around the opaque Help rectangle, and uses the semantic overlay
background, palette border, selection, and overlay foreground roles without
changing the scene contract.

Context: the app already generates Help from the built-in command table, live
keymap, palette fast-path rules, pointer gestures, and glyph legends. It owns
filtering, selection, scrolling, footer overflow honesty, centered geometry,
keyboard editing, toggle/close behavior, and the distinction between headings,
labels, and key hints. The excluded renderer rejected that complete product
scene even though no app, command-table, or keymap access was required.

Rationale: retaining `HelpOverlay` in the headless paint plan keeps generated
content and live route truth in the app and scene layers. The real-host tracer
bullet filters to the App heading and Search session output entry, proving that
the configured Ctrl+Shift+F route crosses the renderer boundary instead of
being copied into the adapter. The same prepared data drives the displayed
surface, border, query cursor, grouped rows, selected-row highlight, key hints,
and pinned footer. Scalar-character fitting remains deliberate; grapheme,
wide-cell, and full style correctness remain Phase 4 work.

Consequences: no app, runtime, scene, command table, keymap, production
dependency, allowlist, installer, default command, or release surface changes.
Multiple panes, Welcome, full
input/theme/style parity, restore, Artifact Preview, and production GPU
admission remain separately gated. The next slice is the existing one-pane
first-run Welcome overlay only.

Verification: the real-host test recorded the initial
`UnsupportedScene::Overlay("help")` failure, then passed with the product Help
retained unchanged by the prepared plan. The isolated renderer test covers
geometry, query and cursor, grouped heading/entry indentation, key hints,
selection/window alignment, footer, the empty-items placeholder, and bounded
lines.
`./ci/gpu-spike.sh` passed 26 tests (two native-shell, ten real-host, and
fourteen isolated-renderer) plus the renderer dependency-boundary scan, and
`cargo test -p mandatum-app --lib` passed all 248 tests. A displayed release
smoke showed the real Empty pane around an opaque filtered Help modal with the
App heading, Search command, live Ctrl+Shift+F route, visible cursor, selection,
and bounded footer; Escape and Ctrl+Q closed it with exit 0 and no native or
attempted-shell process left. The final merge-gate result is recorded in
`docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers Generated Welcome

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one scene-only increment that accepts and
paints the real `OverlayScene::Welcome` emitted by `FrontendHost` over a
supported Empty pane. The prepared plan retains the existing resolved area,
introduction, ordered generated key routes and descriptions, and dismissal
text. Displayed paint clips base-pane glyphs around the opaque Welcome card and
uses the semantic overlay background, palette border, and overlay foreground
roles without changing the scene contract.

Context: the app already owns startup-restore policy, missing-workspace
detection, first-action dismissal, generated live-keymap routes, descriptions,
centered geometry, introduction, and dismissal text. Welcome is non-modal:
resize preserves it, while the first key, paste, click, or wheel action dismisses
it and still proceeds. The excluded renderer rejected that complete product
scene even though no app, persistence, restore implementation, command-table,
or keymap access was required.

Rationale: retaining `WelcomeOverlay` in the headless paint plan keeps first-run
policy and generated route truth in the app and scene layers. A writable
disposable project with no workspace file proves the real startup path rather
than synthesizing the overlay. The same prepared data drives the displayed
surface, border, aligned route rows, and dismissal. Scalar-character fitting
remains deliberate; grapheme, wide-cell, and full style correctness remain
Phase 4 work.

Consequences: no app, runtime, persistence, restore implementation, scene,
command table, keymap, production dependency, allowlist, installer, default
command, or release surface changes. Every current overlay variant now reaches
the excluded plan. Multiple panes, restore in the excluded native shell, full
input/theme/style parity, Artifact Preview, and production GPU admission remain
separately gated. The next slice is exactly two horizontally tiled Empty panes.

Verification: the real-host test recorded the initial
`UnsupportedScene::Overlay("welcome")` failure, then passed with the product
Welcome retained unchanged by the prepared plan. The isolated renderer test
covers geometry, title, introduction, blank separators, ordered and aligned
route/description rows, dismissal, and bounded lines. `./ci/gpu-spike.sh` passed
28 tests (two native-shell, eleven real-host, and fifteen isolated-renderer)
plus the renderer dependency-boundary scan, and `cargo test -p mandatum-app
--lib` passed all 248 tests. A displayed disposable harness compiled against
the exact local host, scene, and renderer showed the real Welcome over Empty
content without glyph leakage; Escape dismissed it, focused Ctrl+Q exited 0,
and no smoke or native-spike process remained. The final merge-gate result is
recorded in `docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers Two Horizontal Empty Panes

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one layout-only increment that accepts and
paints exactly two horizontally tiled `PaneContent::Empty` panes emitted by a
real `FrontendHost`. `PreparedScene` now owns an ordered collection of
per-pane paint records, while its existing single-pane accessors preserve the
covered one-pane test and adapter surface. Admission is deliberately limited
to two non-floating, non-stacked, non-zoomed Empty panes whose adjacent
rectangles fill the scene workspace and have equal vertical bounds.

Context: Ctrl+P then `v` already mutates the product layout and scene builder
into two resolved side-by-side panes. The excluded renderer previously rejected
that valid product frame solely because its preparation and glyph buffers
assumed one pane. No layout math, product command behavior, scene type, or
runtime behavior was missing.

Rationale: retaining each `PaneScene` unchanged keeps rectangles, durable
titles, focus, flags, and Empty detail app/scene-owned. The GPU adapter needs
only bounded per-pane title/body buffers and scene-order paint. Narrow
shape-based admission prevents the per-pane refactor from silently claiming
vertical, stacked, floating, dense, mixed-content, or three-plus-pane support.
The one-pane overlay path remains unchanged and two-pane overlays are not
admitted.

Consequences: no app, runtime, scene, layout, command table, keymap,
persistence, production dependency, allowlist, installer, default command, or
release surface changes. Every covered one-pane content and overlay path
remains green. The next slice is exactly two vertically tiled Empty panes;
stacked, floating, dense, mixed-content, and three-plus-pane layouts, restore,
broader input/theme/style parity, Artifact Preview, and production GPU
admission remain separately gated.

Verification: the real-host tracer proved the exact 80x24 product scene and
first failed with `UnsupportedScene::PaneCount(2)`. The focused GREEN retains
both 40x22 rectangles, titles, focus, flags, and Empty details in the prepared
plan. `./ci/gpu-spike.sh` passed 29 tests plus the renderer boundary scan, and
`cargo test -p mandatum-app --lib` passed all 248 tests. A displayed
missing-shell release smoke showed the real two-pane horizontal layout, titles,
focus styling, and Empty detail in the native window; no native process
remained afterward. The final merge-gate result is recorded in
`docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers Two Vertical Empty Panes

Status: accepted (2026-07-22)

Decision: continue Phase 3 with one layout-only increment that accepts and
paints exactly two vertically tiled `PaneContent::Empty` panes emitted by a
real `FrontendHost`. Admission is deliberately limited to two non-floating,
non-stacked, non-zoomed Empty panes whose adjacent rectangles fill the scene
workspace and have equal horizontal bounds. The completed one-pane and
two-horizontal-pane paths remain unchanged.

Context: Ctrl+P then `s` already mutates the product layout and scene builder
into two resolved top-to-bottom panes. The excluded renderer's scene-order
per-pane paint path already consumed individual rectangles, but its prepared
plan rejected the valid vertical product frame solely because admission
recognized only the horizontal tiled shape. No GPU layout math, product command
behavior, scene type, runtime behavior, or new paint representation was
required.

Rationale: retaining each `PaneScene` unchanged keeps rectangles, durable
titles, focus, flags, and Empty detail app/scene-owned. Shape-based admission
extends only the product-generated vertical sibling of the proven horizontal
path and continues to reject overlays, floating, stacked, dense,
mixed-content, and three-plus-pane scenes.

Consequences: no app, runtime, scene, layout, command table, keymap,
persistence, production dependency, allowlist, installer, default command, or
release surface changes. Every covered one-pane content/overlay path and the
two-horizontal-Empty-pane path remain green. Floating, stacked, dense,
mixed-content, and three-plus-pane layouts, restore, broader input/theme/style
parity, Artifact Preview, and production GPU admission remain separately
gated. The next implementation slice is the smallest still-rejected two-pane
floating Empty layout.

Verification: the real-host tracer proved the exact 80x24 product scene,
including `(0, 1, 80, 11)` and `(0, 12, 80, 11)` rectangles, titles, focus,
flags, and complete Empty details, then first failed with
`UnsupportedScene::Layout("only two horizontal tiled Empty panes")`. The
focused GREEN retains both panes unchanged in the prepared plan.
`./ci/gpu-spike.sh` passed 32 tests (two native-shell, fourteen real-host, and
sixteen isolated-renderer) plus the renderer dependency-boundary scan, and
`cargo test -p mandatum-app --lib` passed all 248 tests. A displayed
missing-shell release smoke showed the real two-pane vertical layout, complete
details, and lower-pane focus styling in the native window; Ctrl+Q exited and
no native or attempted-shell process remained. A fresh cold read found that a
real two-pane stack collapses to one visible `PaneScene` and had bypassed the
two-pane admission predicates; it now fails explicitly with `Layout("stacked
panes")`. The isolated negative matrix also proves overlays, every forbidden
layout flag on either pane, invalid adjacency/workspace geometry, and mixed
content fail closed. The final merge-gate result is recorded in
`docs/verification.md`.

## Accepted: The Excluded Native Render Plan Covers The Default Two-Pane Floating Empty Layout

Status: accepted (2026-07-23)

Decision: continue Phase 3 with one layout-only increment that accepts and
paints the smallest two-pane floating `PaneContent::Empty` layout emitted by a
real `FrontendHost`. Admission is limited to one tiled Empty pane filling the
workspace plus one default-position floating Empty pane at the scene-resolved
rectangle. The exact command route also admits the intermediate
two-horizontal-Empty plus Palette frame required to dispatch Float.

Context: Ctrl+P then `v`, followed by Ctrl+P then `f`, already mutates product
layout into the target scene. At 80x24 it resolves tiled `pane-1` to
`(0, 1, 80, 22)` and focused floating `pane-2` to `(8, 5, 72, 18)`. The
existing scene-order GPU paint consumed both records, but prepared-plan
admission rejected the floating shape. Displayed verification then exposed
that native redraw sees the real two-pane Palette frame between those commands.

Rationale: retaining each `PaneScene` unchanged keeps rectangles, durable
titles, focus, layout flags, and Empty detail app/scene-owned. Validating only
the default floating rectangle and the one Palette command frame avoids
claiming moved/resized floats or broader two-pane overlays while making the
required real native route executable. Because the GPU submits glyphs after
pane quads, the floating surface must also paint an opaque background and clip
lower-pane title/body glyph bounds around its scene-owned rectangle.

Consequences: no app, runtime, scene, layout, command table, keymap,
persistence, production dependency, allowlist, installer, default command, or
release surface changes. Every covered one-pane content/overlay path and both
tiled two-pane paths remain green. Stacked, broader floating, dense,
mixed-content, and three-plus-pane layouts, restore, broader input/theme/style
parity, Artifact Preview, and production GPU admission remain separately
gated. The next implementation slice is the smallest still-rejected two-pane
stacked Empty layout.

Verification: the required real-host tracer first failed with
`UnsupportedScene::Layout("only two horizontal or vertical tiled Empty
panes")`, then retained both exact pane records and complete Empty details.
The displayed route exposed a second RED for the intermediate Palette frame;
that exact frame now reaches the same prepared plan. A cold reviewer found
that lower-pane glyphs could otherwise render over the float after its quads;
the fix adds opaque float fill, clips lower title/body bounds, and covers a
long wrapped Empty cwd. The isolated negative matrix rejects overlays,
forbidden flags, altered tiled/floating geometry, and mixed content.
`./ci/gpu-spike.sh` passed 36 tests (two native-shell, sixteen real-host, and
eighteen isolated-renderer) plus the renderer boundary scan,
and `cargo test -p mandatum-app --lib` passed all 248 tests. A displayed
missing-shell release smoke repeated from the review-fixed binary with a long
wrapping project path: the tiled pane remained clipped behind focused floating
`terminal 2`, both panes showed complete Empty detail, Ctrl+Q exited 0, and no
native or attempted-shell process remained. The final merge-gate result is
recorded in `docs/verification.md`.

## Accepted: Default-Float Recognition And Palette Occlusion Stay Scene-Bound

Status: accepted (2026-07-23)

Decision: correct the excluded GPU adapter without admitting another scene
shape. `mandatum-scene::layout::default_floating_pane_rect` now resolves the
core `FloatingRect::default()` inside the scene workspace through the existing
floating-rect clamping calculation. The adapter recognizes its one supported
default float by consuming that result instead of duplicating the default
offsets, dimensions, and clamps. The exact admitted two-horizontal-Empty plus
Palette transition now treats the Palette as opaque and subtracts its
scene-owned area from every underlying pane title and body glyph region.

Context: the adapter had copied the core default values and the scene clamping
formula to recognize the supported float. A future core default or scene clamp
change could therefore make it reject the real product scene despite the
documented scene-owned layout boundary. Separately, the Palette background quad
was submitted before all glyph text, while pane-text occlusion covered Search,
Help, and Welcome only; long wrapped Empty detail could paint through the
Palette during the real Float command transition.

Rationale: one small public scene resolver keeps durable default intent and
resolved geometry behind the existing layout module without moving product
policy into the renderer. A headless visible-area plan makes the glyph
occlusion used by displayed paint directly testable against a real
`FrontendHost` frame rather than proving admission and geometry alone.

Consequences: the already-supported default floating path and its narrow
Palette transition are preserved. Stacked, moved/resized or additional
floating panes, broader two-pane overlays, dense, mixed-content, and
three-plus-pane scenes remain fail-closed. No app, runtime, command,
persistence, production dependency, release allowlist, installer, Artifact
Preview, or production-admission surface changes.

Verification: focused RED runs first failed because the shared scene resolver
and Palette-safe pane-text visibility plan did not exist. Focused GREEN proved
default resolution at 80x24, clamping geometry in a 6x3 viewport, and a real
two-horizontal-Empty Palette frame whose deliberately long project path wraps
through the overlay. That slice proved scene-cell body fragments stayed outside
the Palette, but the aggregate review later found that independently rounding
those fragments in pixels could reintroduce a one-pixel overlap. Cold review
also added one negative test proving that altering the Palette's scene-resolved
rectangle fails closed and one small-viewport regression proving pane-title
glyphs are removed from the opaque area. The pixel-space correction and the
fact that 6x3 is rejected by renderer admission are recorded in the decision
below and in `docs/verification.md`.

## Accepted: Pixel-First Occlusion And Usable Multi-Pane Interiors

Status: accepted (2026-07-23)

Decision: preserve the already-admitted horizontal, vertical, and
default-floating topologies while correcting their renderer boundary. Convert
complete pane title and body rectangles to final pixel `TextBounds` before
subtracting the outward-rounded bounds of later floats or any current opaque
overlay. Admit multi-pane scenes only when every pane rectangle is at least 3x3
cells, leaving one real interior cell after the one-cell border.

Context: the three-slice aggregate review found two defects. Pane bodies were
subtracted in scene cells and every visible fragment was then rounded outward
independently, allowing a one-pixel overlap at fractional cell widths. The
horizontal, vertical, and floating predicates also accepted sub-border pane
rectangles; the scene helper intentionally returns a clamped `(5, 1, 1, 1)`
default float at 6x3, whose derived body lies outside that pane and collides
with status chrome.

Rationale: pixel-first subtraction matches the already-correct pane-title path
and uses the same conservative outward rounding as the opaque surface. Requiring
actual pane rectangles to be 3x3 is the narrow fail-closed correction: it
handles arbitrary split ratios and default-float clamping without adding GPU
layout policy or broad clipping semantics. A scene resolver may describe
degenerate geometry; accepting that geometry for paint is a separate adapter
decision. Header and status text use the same final-pixel overlay subtraction
because their glyphs share the post-quad text pass.

Consequences: the default 50/50 horizontal path is supported from 6x5, the
default vertical path from 3x8, and the default float from 11x9. Width or height
immediately below those boundaries, and any larger frame whose split produces a
sub-3-cell pane, fail explicitly. Checked right/bottom endpoints also reject
malformed maximum-dimension rectangles whose true edge would overflow `u16`;
saturating geometry cannot masquerade as a frame-bound edge. The 6x3 scene
resolver test remains valid layout/clamping evidence but is not a successful
GPU render case. No additional topology, overlay, product dependency, build,
release, Artifact Preview, or production-admission surface is admitted.

Verification: focused RED failed because the final-pixel visibility API and
usable-interior predicates did not exist. Fractional-width isolated regressions
now prove final body `TextBounds` are disjoint from a later float and every
current opaque overlay. A 3x3 full-frame overlay regression proves header and
status glyphs are also removed. Real-host resize tests accept 6x5 horizontal,
3x8 vertical, and 11x9 default-floating scenes, then reject the immediately
smaller width or height. Maximum-width/height cases reject overflowing pane
edges. The long-path real-host Palette tracer checks the same final-pixel seam
at a fractional cell width. `./ci/gpu-spike.sh` passes 50 tests (two
native-shell, twenty real-host, twenty-eight isolated-renderer) plus the
renderer dependency-boundary scan; all 35 scene tests and all 248 app library
tests pass. Displayed smoke showed no leakage at the observed 800x632 scale,
Ctrl+Q exited cleanly, and no native or attempted-shell process remained. The
full merge-gate and review results are recorded in `docs/verification.md`.

## Accepted: Capability-Family Scene Compilation Replaces Topology Admission

Status: accepted (2026-07-23)

The capability-family delivery unit remains accepted. Its temporary per-pane
paint mechanism was superseded later the same day by
“One Neutral Cell Program Owns Frontend Presentation” below.

Decision: complete native/GPU parity by capability family rather than running a
full delivery lifecycle for every layout variant. The excluded adapter keeps
`prepare_scene(&WorkspaceScene, &Theme)` as its public seam and deepens the
prepared scene compiler behind it. Layout/composition is one family: compile
every ordered scene pane through generic structural validation, dynamically
size pane paint resources, and subtract every later opaque pane plus the
current opaque overlay in scene order.

Context: Phase 3 had accumulated a separate admission predicate, tracer,
displayed smoke, review, documentation update, gate, handoff, and commit for
each horizontal, vertical, and default-float topology. That repeated layout
knowledge in the adapter and made the cost of parity grow with the number of
variants even though `WorkspaceScene` already carries resolved rectangles,
flags, content, and draw order.

Rationale: the scene contract is already the authoritative layout program.
The renderer needs only local resource-safety checks: a usable bordered
interior, checked endpoints, workspace containment, and an aggregate pane
ceiling. It must not prove identity, tiled coverage, reject intentional
overlap, recompute default geometry, or infer draw order from flags. One deep
compiler therefore handles stacked, zoomed, three-plus-pane, mixed-content,
moved/custom-float, multiple-float, and overlay combinations without a new
special case.

Consequences:

- the older exact-topology admission decisions remain historical evidence but
  no longer describe the active renderer boundary;
- pane title/body buffers grow with the scene and retain high-water capacity;
- the excluded adapter rejects more than 256 visible panes, bounding that
  high-water mark without changing the product's layout model;
- lower pane text is clipped against every later pane, not only the first
  floating pane;
- focused RED/GREEN tracers remain useful, but aggregate review, displayed
  smoke, doc sync, full gate, handoff, and commit happen once per capability
  family;
- the next Phase 3 family is content/style parity, beginning with neutral cell
  semantics; input/lifecycle parity follows;
- Artifact Preview is a dedicated phase before hardening, measurement,
  admission, and rollout.

Verification: the stack tracer first failed with `Layout("stacked panes")`;
the three-pane tracer first failed with `PaneCount(3)`; and the dynamic buffer
test first failed to compile because no pane pool existed. Focused GREEN runs
then covered a real stack, three tiled panes, two real ordered floats, dynamic
high-water growth, and multi-float-plus-overlay pixel occlusion. The isolated
renderer and real-host suites pass as capability matrices. Aggregate review,
displayed smoke, and `./ci/gpu-spike.sh` are recorded in
`docs/verification.md`; `./ci/gate.sh` remains the final family completion
gate.

## Accepted: One Neutral Cell Program Owns Frontend Presentation

Status: accepted (2026-07-23)

Decision: `mandatum-scene` compiles every `WorkspaceScene` into one
renderer-neutral, final-topmost `CellProgram`. Each coordinate carries one
occupancy (`Glyph(char)` or `WideContinuation`), complete `SceneCellStyle`, an
optional selection kind, and a cursor mark. Terminal, task, agent, Empty,
header/status chrome, pane titles/borders, and every overlay use this compiler.
The shipped ratatui renderer and excluded GPU renderer are translation-only
adapters over the same program.

Context: layout/composition parity still left two presentation authorities.
The ratatui adapter owned pane/overlay widgets and the GPU adapter retained
content-specific strings, buffers, and overlay formatters. The GPU path also
honored only a subset of cell style, selection, and cursor semantics. Adding
more content-specific GPU branches would have multiplied drift with every
surface and theme role.

Rationale: presentation meaning belongs beside the renderer-neutral scene
contract. One cell compiler makes opacity, truncation/wrapping, semantic roles,
selection, cursor, and modifiers independently testable and gives every
frontend the same complete paint input. Keeping `SceneCell` unchanged avoids
claiming grapheme-width data the terminal engine does not yet expose; the
explicit continuation occupancy is the truthful Phase 5 seam.

Consequences:

- the old ratatui pane, surface, and overlay modules are deleted;
- GPU `PreparedScene` retains only `CellProgram`, not pane/content/overlay
  shadow plans;
- cell storage replaces earlier paint at the same coordinate while compiling,
  so retained memory is bounded by final frame coverage rather than summed
  overlapping pane area;
- the GPU adapter maps ANSI/indexed/RGB colors, bold, dim, italic, underline,
  inverse, hidden, strikethrough, terminal/item selection, and cursor;
- the excluded GPU boundary rejects more than 256 panes, more than 262,144
  frame cells, a conservative precompile estimate above 4,000,000 paint
  instructions, or more than 4,096 retained row buffers;
- these are adapter resource limits, not product layout meaning;
- `./ci/gpu-spike.sh` now includes warnings-denied all-target clippy;
- input/lifecycle parity was the next Phase 3 family and is completed by the
  following decision; wide/grapheme production and IME remain Phase 5, and
  production GPU admission remains blocked.

Verification: focused RED/GREEN tracers cover terminal style/selection/cursor,
mixed pane content, every overlay, final-cell opacity, narrow pane/overlay
containment, huge off-frame rectangles, many overlapping panes, reverse-video
modifier composition, and checked GPU resource limits. Real-host tests assert
representative final program content for Empty, task, agent, copy mode, and
every overlay. Aggregate review removed duplicate authorities and corrected
unbounded replacement storage, degenerate-border leakage, contradictory
selection state, and missing spike clippy coverage. The exact automated and
displayed evidence is recorded in `docs/verification.md`; `./ci/gate.sh`
remains the final completion gate.

## Accepted: Native Input And Lifecycle Stay Behind FrontendHost

Status: accepted (2026-07-23)

Decision: Phase 3 input/lifecycle parity uses the existing `FrontendHost` as
the only product boundary. The winit shell translates platform key, pointer,
focus, geometry, and clipboard events, owns pressed-button and surface/scale
state, and schedules paint. Configurable command resolution, terminal byte
encoding, selection/scrollback, runtime restore/reconciliation, and shutdown
remain in app/runtime layers.

Native Command+C and Command+V are exact platform fallbacks only after the
configured workspace keymap has first refusal. The shared terminal encoder
owns the baseline `xterm-256color` key/modifier/control families. Focus or
geometry transitions cancel workspace gestures and release a child mouse
capture before stale coordinates are discarded. A native frame that cannot be
presented clears shared hit targets and suppresses pointer input until a valid
frame presents. Renderer-neutral float layout preserves a 3x3 bordered area
whenever the workspace has room, including after restore or shrink.

Rationale: native conventions belong at the platform edge, but duplicating
command routing, terminal semantics, selection, recovery, or runtime ownership
would recreate the parallel-product failure Phase 2 removed. Interaction must
also resolve against what the user actually saw, never a rejected or
geometry-stale frame.

Consequences:

- exact native clipboard conventions remain configurable-chord-safe;
- unbound Super chords do not leak into child terminals;
- Alt-as-Meta and the baseline modified-key/control families are frontend
  neutral, while advanced IME/dead-key/grapheme behavior remains Phase 5;
- child any-event motion, button capture, scrollback, selection, focus
  cancellation, resize, scale, restore, and idempotent shutdown use the real
  host;
- the spike-only bounded scale tracer exercises the same transition as
  `ScaleFactorChanged` without changing system display settings;
- Artifact Preview is the exact next capability family; production GPU
  dependencies and release admission remain blocked.

Verification: focused tests, the aggregate multi-agent review, the 39-test GPU
matrix, the displayed macOS release matrix, the standing terminal latency and
idle-CPU procedure, and the post-documentation merge gate are recorded in
`docs/verification.md`.

## Accepted: Artifact Preview Keeps Intent Durable And Pixels Live

Status: accepted (2026-07-23)

Decision: Phase 4 introduces `ArtifactPaneIntent` as project-relative durable
core state containing source, title, useful alt text, and `Contain` fit only.
The app owns cheap source observation, secure descriptor-relative no-follow
opening on supported macOS/Linux hosts, PNG header validation, bounded decode,
reload, worker scheduling, and the live pixel cache. The scene carries typed
loading/ready/failed artifact content and one immutable RGBA8 sRGB
`RasterSurface`; it carries no decoder, file handle, or GPU resource.

The first slice accepts static PNG only. It rejects non-relative paths, every
symlink component, non-regular files, non-PNG extensions, animation, malformed
data, files above 16 MiB, dimensions above 4096×4096, more than 64 MiB decoded
RGBA across active/queued/cached previews, more than four concurrent decoders,
and more than 64 artifact panes. Unsupported platforms return a visible
failure instead of falling back to a racy open.

Rationale: the first native-only product value must cross the existing
core/app/scene/adapter seams without making pixels durable or giving a
renderer filesystem authority. Opening every path component relative to an
already-opened project root closes validation/use races. Counting queued and
active reservations prevents many individually valid files from bypassing the
aggregate ceiling. Preserving artifact completions across restore releases
stale reservations without persisting live state.

Consequences:

- "Open artifact preview" is fuzzy-palette discoverable and accepts a
  project-relative PNG path;
- "Restart pane" on an artifact forces reload without incrementing terminal
  restart generation;
- the terminal adapter always renders source/alt/state as a deterministic
  fallback;
- final-topmost `ProgramCell::raster_layer` markers let the excluded GPU
  adapter contain-fit and clip pixels behind later panes/overlays;
- the GPU cache evicts all stale live layers before allocating replacements,
  preserving the admitted 64 MiB high-water bound across redistribution;
- the native GPU spike remains outside the product workspace, release, and
  merge gate; this decision does not admit production GPU dependencies;
- advanced grapheme/IME correctness remains Phase 5.

Verification: focused tests cover durable-intent round trips, exact RGBA load
and revision reload, APNG/malformed/missing/oversize/traversal/symlink
failures, descriptor swap races, aggregate/fan-out/pane caps, stale restore
completion, terminal fallback, scene occlusion, contain-fit, cache replacement,
and the real host-to-GPU plan. Three independent reviewers plus a final cold
read drove the boundary fixes and ended clean. The displayed release matrix
proved landscape and portrait contain-fit, explicit reload, Help occlusion,
full-screen resize, visible missing-file failure, and clean Ctrl+Q exit. Exact
commands and counts are in `docs/verification.md`.

## Accepted: Grapheme Cells And IME Composition Stay Neutral

Status: accepted (2026-07-23)

Decision: Phase 5 replaces scalar cell occupancy with a bounded extended
grapheme string in the terminal snapshot and renderer-neutral cell program.
Width-two graphemes own a following `WideContinuation`; grid mutation repairs
that pair atomically. The scene compiler accepts exactly one nonempty grapheme
cluster of display width one or two and fails closed for malformed public scene
input. Copy, search, selection, cursor, wrapping, clipping, and both adapters
consume those same declared cell spans.

`InputEvent::Composition` is the only IME boundary: preedit carries text plus a
validated UTF-8 cursor range, commit inserts once into the locked active text
target, and cancel clears transient state. Composition is neither paste nor
durable workspace intent. Focus, modal, pointer, paste, ordinary key, and
shutdown transitions cancel it; one late commit from a canceled platform
sequence is ignored. The native shell enables platform IME only for a focused
eligible target and derives the candidate rectangle from scene cell geometry.
On macOS, left Option remains native dead-key/IME input and right Option is
terminal Meta.

Context: the earlier `Glyph(char)` cell program and direct logical-key path
could neither preserve combining/ZWJ sequences nor model platform preedit.
Letting each renderer infer width or composition would split cursor, selection,
overlay routing, and clipping authority across frontends. Treating composition
as paste would also bypass target locking and paste-specific policy.

Rationale: segmentation, display width, and transient composition are
renderer-neutral text semantics. The terminal engine must own grid invariants;
the app must own which product surface receives text; the native shell should
own only platform event translation, focus/enablement, caret placement, and
native font/scale settings.

Consequences:

- `mandatum-scene` may depend on pure Unicode segmentation and width crates in
  addition to `mandatum-core` and serde; it still has no terminal, GPU, or
  platform dependency;
- one grapheme is capped at 256 UTF-8 bytes, public scene input is normalized
  before compilation, and pathological GPU frames are rejected before buffer
  allocation;
- the GPU renderer creates one anchored buffer per visible grapheme, retains
  decorated spaces, and clips glyphs to shared fractional pixel boundaries;
- native font family, size, and runtime scale are validated shell settings and
  do not add inert terminal-frontend configuration;
- the one-display macOS matrix proves runtime scale/resize but does not claim
  cross-monitor or every installed locale/input source;
- Phase 6 hardening and symmetric measurement is next; production GPU
  admission, packaging, and rollout remain blocked.

Verification: focused terminal, scene, app, renderer, and native-shell tests
cover combining, CJK, emoji ZWJ, wide-edge repair, copy/search/selection/cursor,
preedit/commit/cancel, late commits, modal/focus routing, scale and glyph-span
geometry. Three independent review tracks drove boundary corrections and ended
clean. The displayed macOS matrix and exact gate/latency evidence are recorded
in `docs/verification.md`.

## Accepted: Phase 6 Completes The Excluded Hardening Refactor, Not Admission

Status: accepted (2026-07-24)

Decision: Phase 6 is complete when the excluded native adapter has deterministic
surface/device recovery, explicit failure outcomes, bounded event-loop work,
structured evidence, a complete resize/scale storm, and accepted symmetric
acquisition. The proposed 30-minute soak, multi-display matrix, and latency
thresholds are production-admission evidence owned by Phase 7. They are not
prerequisites for completing an excluded refactor.

Context: the Phase 6 implementation and repeated live runs exercised the
adapter beyond a normal spike bar. They found real event-loop starvation,
multi-second synchronous drain slices, watchdog ordering, and screen-lock
occlusion defects, all of which were fixed. Three paired 1,000-sample
acquisitions completed, but their accepted result already fails the later
admission bar: native p95 is above 20 ms, one pair improves by less than 25%,
the terminal path is not zero-miss across all trials, and this one-display Mac
cannot prove multi-display behavior. Further repeated 30-minute runs cannot
change that admission decision.

Rationale: engineering verification should be proportional to the decision it
can affect. Deterministic hardening tests and bounded live stress prove the
refactor; admission-grade endurance and platform evidence justify adding GPU
dependencies to the shipped product. Conflating those gates turns an excluded
spike into an open-ended qualification program without reducing current
product risk.

Consequences:

- Phase 6 code, tests, evidence, and documentation may land while winit/wgpu
  remain excluded from the product workspace, installer, release, and ordinary
  merge gate;
- the completed 1,000-change and 3×1,000 paired acquisitions remain valid
  Phase 6 evidence and do not need repetition;
- no clean 30-minute soak or multi-display qualification is claimed;
- Phase 7 must explicitly accept its long-soak, latency, platform, dependency,
  packaging, and rollout evidence before any production promotion;
- the terminal frontend remains the shipped default.

Verification: `./ci/gpu-spike.sh`, `cargo test -p mandatum-app`, and the full
`./ci/gate.sh` are green. Surface/device/OOM probes and the resize storm
completed with structured evidence. Three independent final reviews returned
no finding. Exact counts, methodology, measurements, and remaining boundaries
are recorded in `docs/verification.md` and
`spikes/frontend-wgpu/RESULTS.md`.

## Accepted: The Native wgpu Frontend Is The Product

Status: accepted (2026-07-24)

Decision: Mandatum is a personal GPU-native development environment. The native
wgpu frontend is the primary product surface and daily-driver target. The
terminal frontend is a maintained tool for SSH, headless use, recovery, and an
explicit escape hatch. At this decision point, public distribution was not in
scope and there was no Phase 7/8 admission ceremony. The later public-release
decision supersedes only that distribution boundary.

Context: the shared host, neutral input/effects, complete scene composition,
typed Artifact Preview, advanced text/IME, GPU recovery, bounded scheduling,
and measurement probes are already implemented. The former production-admission
framing made personal adoption wait on requirements that do not serve the
product's actual user or support matrix.

Rationale: daily use on the reference environment's known macOS hardware is the relevant quality
gate. Native polish and richer typed scene capabilities now create direct
product value. Existing probes remain useful regression evidence, but do not
grant permission to pursue the product direction.

Consequences:

- reorder startup so window, surface, adapter, and device succeed before
  `FrontendHost` creates application state or live runtimes;
- promote the native frontend into the workspace and ordinary native gate;
- keep GPU/window dependencies confined to the native frontend;
- compare glyphon/cosmic-text with Ghostty before visual-identity investment;
- add a bounded shaping cache, then profile before adding row damage tracking;
- make native the default after daily-driver readiness, with an explicit
  terminal escape hatch;
- retire the sub-20 ms admission bar, 25% comparison pairs, 30-minute soak and
  multi-display prerequisites, Linux-native qualification, accessibility/theme
  parity gates, and Phase 8 rollout ceremony;
- retain latency, idle, resize, recovery, and fault probes as regression tools;
- allow richer native presentation only through typed `mandatum-scene`
  extensions; `CellProgram` remains terminal parity.

Verification impact: `./ci/gate.sh` and the native gate must pass. Conformance
allows GPU dependencies only in the production native frontend and retains
negative checks elsewhere. Startup tests must prove forced no-adapter and
no-display failure before `AppState` exists.

This decision supersedes only the opt-in/default/admission posture in
**Native GPU Capability Branch Is Selected; Production Admission Remains
Gated** (2026-07-21) and **Phase 6 Completes The Excluded Hardening Refactor,
Not Admission** (2026-07-24). Their architecture choices and recorded evidence
remain historical fact.

## Accepted: Native GPU Preflight Precedes Product State

Status: accepted (2026-07-24)

Decision: the native application holds validated `AppConfig` and
`host: Option<FrontendHost>` while winit creates the window and `GpuText`
creates the surface, adapter, device, queue, and renderer. The sole host
construction seam runs only after that complete preflight succeeds.

Context: `App::new` previously constructed `FrontendHost` before winit entered
`resumed()`. Because `FrontendHost` owns `AppState`, restore, and all live
runtimes, a missing display or incompatible GPU could leave PTYs running even
though native presentation never became possible.

Rationale: configuration is inert and safe to retain during preflight; product
state and live runtimes are not. One ordered seam makes every window, surface,
adapter, device, or renderer failure return before host side effects without
creating a second state machine.

Consequences:

- native boot owns `host: None` until complete GPU renderer construction;
- failed preflight has no host to shut down and cleanup remains idempotent;
- successful restore and PTY startup begin only after native rendering can
  start;
- renderer recovery still preserves the already-running shared host;
- workspace promotion remains separate Work 2.

Verification: deterministic forced no-display, no-adapter, surface, and device
failure tests prove the GPU and host factories stop in order; a success test proves
window → GPU renderer → host construction. The native gate, real macOS
startup/clean exit, and authoritative workspace gate are recorded in
`docs/verification.md`.

## Accepted: Production Native Frontend Is A Workspace Component

Status: accepted (2026-07-24)

Decision: the native product frontend is split into two root-workspace
packages: `mandatum-native` owns the winit product shell and
`mandatum-native-renderer` owns scene-only wgpu/glyphon presentation.
Measurement, stress, synthetic fault injection, ScreenCaptureKit acquisition,
and the terminal latency probe remain in the excluded
`spikes/frontend-wgpu` lab.

Context: Work 1 made GPU preflight safe, but the working native implementation
and its maintenance gate still carried spike names and lived outside ordinary
workspace CI. The combined lab shell also mixed product lifecycle/input code
with measurement deadlines, evidence collection, stress schedules, and
injected failures.

Rationale: workspace membership makes native a maintained product component
without adding lab controls to the daily-driver executable. A separate
renderer crate keeps the strongest boundary executable: GPU paint consumes
`WorkspaceScene`, not app, PTY, parser, or terminal-renderer internals. The
excluded lab remains useful regression tooling, but it does not substitute for
product-package tests or a real native startup check.

Consequences:

- the stable development command is
  `cargo run -p mandatum-native --bin mandatum-native`;
- the product command accepts only bounded `--font-family` and `--font-size`
  options; lab-only flags are rejected;
- native input preserves Shift+Tab, exact Command copy/paste fallback,
  multi-grapheme composition, validated IME ranges, and Left Option native
  composition while Right Option remains terminal Meta;
- bounded runtime draining continues through event-loop wakes independently of
  whether the current surface can present;
- synthetic fault injection is a renderer feature enabled only by the lab;
- `ci/conformance.sh` allows GPU/window dependencies only in the two native
  packages, freezes both internal Mandatum dependency sets, and negative-tests
  modeled forbidden edges;
- `ci/native-frontend.sh` checks the product packages, the renderer with and
  without fault injection, and the separate lab/real-host regressions;
- `./ci/gate.sh` invokes the native gate and remains the single CI authority;
- the terminal command, installer, updater, release workflow, and archive
  allowlists are unchanged;
- Work 3 typography comparison is next; Work 4 shaping-cache work and
  default-launcher changes remain out of this slice.

Verification: the focused native gate passed 13 product-shell tests, 25
renderer tests with default features, 25 renderer tests with fault injection,
23 lab-shell tests, and 27 real-host tests, plus warnings-denied Clippy, build,
format, locked dependency, feature-closure, and renderer-boundary checks.
Conformance rejected modeled GPU edges in all nine non-native production
crates and a modeled native-shell PTY edge. The final synchronized
`./ci/gate.sh`, terminal smoke, real macOS native startup/clean exit, diff
hygiene, commit, and publication state are recorded in
`docs/verification.md` and the continuation handoff.

## Accepted: Typography Path Must Be Decided Before It Is Cached

Status: accepted (2026-07-24)

Decision: the current Work 3 evidence takes its focused-decision branch. Pause
broader visual-identity investment and the Work 4 shaping cache until the
native text path defines font provisioning, observable/fail-closed face
resolution, terminal palette ownership, and a shaping unit that can preserve
terminal cell semantics while shaping across appropriate grapheme/cell
boundaries. This does not authorize a Metal or Swift renderer rewrite;
glyphon/cosmic-text may remain if a focused row-run adapter proves the right
path.

Context: the reference environment's zero-config Ghostty 1.2.3 uses an embedded JetBrains Mono at
13 points, default background `#282c34`, default foreground `#ffffff`, and a
separate built-in ANSI palette on one external reference display at 3440×1440, scale 1.0,
85 Hz. Mandatum's production CLI accepts the same family name, but cosmic-text
sees only system fonts and the CLI does not verify resolution, so the nominal
actual-settings run silently used an unknown fallback. Native terminal
foreground, background, and ANSI colors are renderer constants rather than
configurable theme data. A displayed Menlo 13 control reduced but did not
eliminate face-resolution uncertainty and showed unjoined Arabic. Independent
code inspection established the cause: the current adapter creates and shapes
one buffer per grapheme, preventing shaping across grapheme/cell boundaries
and therefore preventing cross-cell ligatures regardless of cache performance.

Rationale: a cache makes the chosen shaping unit faster; it cannot make an
incorrect unit typographically complete. Likewise, judging a silent fallback
against Ghostty's embedded face would produce a false stack verdict. Resolve
font and palette truth plus run shaping first, then design the bounded cache
around the accepted presentation contract.

Consequences:

- explicit font requests must become observable and must not silently pass as
  matched evidence when the face is unavailable;
- the reference environment's chosen face needs a deliberate product provisioning path;
- terminal foreground, background, and ANSI colors need explicit ownership
  before exact reference comparisons;
- the next implementation decision must compare a glyphon/cosmic-text row-run
  adapter with any focused alternative while preserving cell clipping,
  cursor/selection placement, wide-cell occupancy, and terminal fallback;
- Work 4 cache implementation, broader visual identity, renderer
  modularization, row damage, and default-launcher work remain blocked.

Verification: `spikes/frontend-wgpu/scripts/typography-corpus.sh` was displayed
through Ghostty's real shell and Mandatum's production `FrontendHost`/PTY/scene
path. The actual-settings attempt was explicitly rejected as unmatched. The
labeled Menlo control exercised ASCII, symbols, fallback scripts, ligature
sequences, CJK, combining text, emoji, normal/bold/dim/italic/underline/inverse
styles, prompt cursor, native selection, and live resize to 1650×1280. The
displayed unjoined-script symptom and the renderer's one-buffer-per-grapheme
code path are separate evidence tiers. After the built-in Retina display became
active, production Mandatum and Ghostty both moved through backing scale
1.0→2.0→1.0 with the shared corpus visible. Mandatum recomputed 191×59,
89×46, then 127×48 scene sizes without observed stale frames or
scale-transition corruption. The focused native result is recorded in
`docs/verification.md`; after the evidence and active docs were synchronized,
`./ci/gate.sh` reported `GATE GREEN`.

## Accepted: Keep Glyphon/Cosmic-Text Behind A Verified Row-Run Contract

Status: accepted (2026-07-24)

Decision: retain glyphon 0.12 and cosmic-text 0.19. Implement native typography
through three owned seams before adding the Work 4 cache:

1. a renderer-owned font provisioner with bundled JetBrains Mono 13 as the
   default, strict installed-family overrides, and bounded face/fallback
   reporting;
2. a scene-owned `TerminalPalette` inside `Theme`, materialized by the native
   pixel renderer while the terminal escape hatch continues delegating reset
   and named ANSI colors to its host; and
3. a native-renderer row-run adapter that shapes adjacent same-style
   graphemes with `Shaping::Advanced` and
   `Buffer::set_monospace_width(Some(cell_width))`, clipped to the exact
   declared cell span.

Context: Work 3 proved that the current system-font-only initialization cannot
see Ghostty's embedded primary face, the CLI accepts an unavailable family
without detecting fallback, native owns an unconfigurable color table, and one
buffer per grapheme prevents Arabic joining and cross-cell ligatures. The
locked libraries already expose the missing primitives: `fontdb` accepts owned
font bytes and exact family queries, face metadata is inspectable,
`LayoutGlyph` exposes the selected `font_id`, rich text shapes across adjacent
spans, monospace-width layout rounds advances to cell-width multiples, and
glyphon `TextArea` bounds clip a complete run.

Rationale: the defect is the adapter contract, not the selected GPU text stack.
Replacing glyphon/cosmic-text would duplicate shaping, fallback, atlas, and
wgpu integration while weakening the scene-only renderer boundary. A bundled
open-licensed primary makes the reference environment's default reproducible; strict system
overrides keep proprietary or preferred installed faces possible without
allowing silent primary fallback. Palette data belongs beside the existing
semantic theme because it already follows config reload through `AppState` and
`FrontendHost`; putting it in `NativeTextSettings` would create a second
presentation configuration path. Cell-aligned backgrounds, cursor, selection,
wide occupancy, and clipping remain final `CellProgram` truth even when glyphs
shape across cells.

Consequences:

- vendor the four unmodified JetBrains Mono v2.304 static faces (Regular, Bold,
  Italic, Bold Italic), upstream OFL, source, version, and SHA-256 values; do not
  relicense the fonts under Mandatum's Apache license;
- the default primary is bundled and cannot be shadowed by an installed
  duplicate; same-family system faces are removed or partitioned before bundle
  insertion and primary selection is enforced by source identity;
- `--font-family` becomes a strict non-generic, monospaced installed-family
  override with exact `FaceInfo` weight/style matches for Regular, Bold,
  Italic, and Bold Italic; closest-match query results and variable-only
  families do not qualify in the first implementation, and failure exits before
  window, GPU, or host creation;
- `--font-info` resolves headlessly and prints stable JSON; normal startup
  names the selected primary, and bounded post-shape diagnostics name fallback
  faces or unresolved glyph samples without rejecting legitimate CJK/emoji
  fallback; the report resets per font-catalog generation and retains at most
  64 records / 64 KiB total with 256-byte string/sample caps;
- build the complete font database before `FontSystem`; renderer recreation
  reuses the resolved profile instead of rescanning;
- `Theme::terminal_palette` owns direct RGB foreground/background and exactly
  16 direct RGB ANSI slots, with partial `[theme.terminal]` overrides;
- native resolves `Default` and every `Ansi(0..=15)` through that palette,
  including semantic chrome; direct RGB and the indexed 16–255 cube/grayscale
  keep their existing meaning;
- the terminal adapter retains `Reset` and named ANSI output for SSH/recovery
  host-palette behavior; explicit RGB terminal output would be a separate
  decision;
- row runs break at row, gap, plain whitespace, raster-backed cell, hidden
  content, orphan continuation, cursor/selection transition, or any glyph-style
  boundary; the scene compiler also assigns a renderer-neutral paint-scope
  identity and clip so flattened cells from different panes, chrome, or
  overlays never join;
- a width-two grapheme and its continuation form one atomic standalone run;
  the continuation adds no text, and per-cell quads continue to own
  backgrounds, cursor, selection, inverse, and decorated-space geometry;
- each shaped byte cluster's unioned x/advance interval must match its complete
  declared-cell interval, and the total advance must match the run width, within
  0.5 physical pixel at the active scale;
- terminal grid order is authoritative: only monotonically increasing LTR
  cluster intervals are admitted. RTL/bidi reordering takes the bounded
  observable anchored fallback; correct bidi plus cell/caret mapping needs a
  later renderer-neutral contract and is not claimed here;
- the anchored grapheme adapter may remain only as a bounded, observable
  fail-safe after splitting a malformed or unrepresentable run;
- arbitrary font-file input, a terminal widget/parser import, a custom
  HarfBuzz/swash atlas, a second renderer, Work 4 caching, renderer
  modularization, row damage, default-launcher, installer, release, and rollout
  changes remain out of this decision slice.

Verification: three independent source-inspection lanes traced font
provisioning, palette ownership, and row shaping through the repository and
the locked dependency sources. They agreed that the existing boundaries and
APIs can express the contract without a stack replacement. This slice changes
architecture guidance only; no rendered-behavior or implementation claim is
made.

## Implement The Accepted Typography Foundation Before Caching

Status: accepted and completed (2026-07-24)

Decision: land font provisioning, terminal-palette ownership, and the clipped
row-run adapter as one capability family before introducing the Work 4 shaping
cache.

The native default is the four pinned JetBrains Mono v2.304 static faces.
`--font-family` accepts only an exact installed monospaced
Regular/Bold/Italic/BoldItalic set and rejects generic, missing, ambiguous, or
variable-only families before downstream application launch construction.
`--font-info` is the stable headless resolution surface. Device recreation
clones the resolved catalog generation and selected face identities. Shaped
fallback faces and missing glyphs emit deduplicated diagnostics retained under
the 64-record / 64-KiB ceiling.

`Theme::terminal_palette` owns direct foreground/background and ANSI 0–15.
Native materializes those colors, including semantic chrome, while the
maintained terminal adapter deliberately preserves host `Reset` and named ANSI
colors.

The scene compiler assigns renderer-neutral text paint scopes and exact clips.
Native builds same-row/same-style runs with checked UTF-8-to-cell maps, admits
only complete monotonic left-to-right clusters within the physical-pixel
tolerance, and otherwise splits or uses a bounded observable anchored fallback.
Final cell quads continue to own backgrounds, cursor, selection, inverse,
decorated spaces, and wide-cell geometry.

The displayed corpus exposed one adjacent lifecycle defect: on a backing-scale
change without a separate resize event, font metrics changed while the wgpu
surface retained the old physical size. The production scale-transition seam
now refreshes the surface from the live window size before scene reflow. This
keeps the full corpus, header, and pane clips coherent through
1.0→2.0→1.0.

Consequences:

- the one-buffer-per-grapheme path is retired except as the bounded ultimate
  fallback;
- legitimate script and emoji fallback remains allowed, named, and bounded;
- RTL/bidi visual reordering remains fallback behavior, not a support claim;
- the next slice may cache only admitted shaped runs, keyed and invalidated by
  the accepted generation/metrics contract;
- renderer modularization, row damage, default-launcher, installer, release,
  and rollout changes remain outside this capability family.

## Cache Only Normally Admitted Shaping Units

Status: accepted and completed (2026-07-24)

Decision: keep a bounded generation-aware shaping cache inside
`mandatum-native-renderer`. A key owns the exact UTF-8 text, resolved rich-style
ranges, declared run width and byte-to-cell topology, font-catalog generation,
font/line/cell metrics, scale generation, and shaping-policy generation.
Position, paint-scope identity, and clip remain per-occurrence `TextArea`
geometry and are not shaping identity.

Only a run that independently returns `RowRunAdmission::Accepted` may populate
the cache. A rejected parent never enters it. Split children must pass normal
admission under their own topology. Bidi and `AnchorAll` descendants bypass
both lookup and insertion, so an anchored fail-safe can never become a hit for
a normally accepted unit. Cached values share the immutable shaped `Buffer`
and retain the observed font IDs, missing-glyph facts, and bounded samples that
must still flow through `FallbackReport` on a hit.

Use amortized O(1) LRU eviction with three independent limits: 4,096 entries,
512 KiB conservative accounted bytes per entry, and 32 MiB conservative
aggregate accounted bytes. Cosmic-text 0.19 intentionally hides some internal
`Buffer` allocation capacities, so the byte metric is named and documented as
accounted rather than exact resident memory; the entry ceiling remains a hard
independent bound. Invalidation replaces the LRU index so its allocation is
released. Palette and real scale changes clear entries; metrics and scale
generation remain in the key so a 1→2→1 round trip cannot resurrect an old
layout. Device recreation keeps lifetime counters and high-water facts but
starts with no cached buffers.

Context: the accepted row-run path was correct but reshaped stable workstation
text every frame. A first scan-based cache draft passed small unit tests but
aggregate review found that high-churn refill could scan 4,096 entries per
miss, that opaque buffer retention was being described too precisely, that
cacheable misses retained both a pool buffer and its clone, and that device
recreation discarded pre-recovery cache evidence. The completed design uses
the locked `lru` dependency, transfers cacheable buffers out of the scratch
pool, preserves a direct pool-backed lab bypass for comparable uncached runs,
and carries render-stage timings even when a prepared frame is skipped or
reconfigured at surface acquisition.

Verification: 57 renderer unit tests covered identity, every generation and
metric bit, topology separation, true LRU behavior, count/entry/aggregate
ceilings, checked-add overflow, replacement, long churn, cold-reset statistics,
real JetBrains Mono ligature shaping, cached observation retention, and forced
anchor bypass. The excluded lab's 23 tests covered its version-2 evidence
schema and cache bypass. Three paired 400-input displayed runs used Apple M4
Pro/Metal at 60 Hz, backing scale 2.0, 1600×1200, scene 102×35, and bundled
JetBrains Mono 13. Median uncached/cached shaping p50 was 0.355/0.039 ms and
median p95 was 0.470/0.074 ms. Median whole-frame preparation p50/p95 was
3.436/4.393 ms uncached and 3.388/4.107 ms cached.

Consequences:

- repeat-heavy frames avoid shaping while retaining exact per-occurrence
  placement and clips;
- the lab-only `--disable-shaping-cache` option is absent from the production
  feature closure and exists only for paired measurement;
- cache lifecycle and frame-stage evidence are machine-readable alongside
  actual render geometry;
- row-level damage is not justified by this profile and remains deferred;
- renderer modularization, default-launcher, installer, release, and rollout
  work remain outside this capability family.

## Defer Public Surface And Visual Material Work Until Build Conclusion

Status: accepted (2026-07-24)

Decision: shelf installer, updater, release, rollout, public-GitHub
presentation, and visual-material work until the concluding build phase.
Interim slices remain functional and local-first. They must not expand into
packaging, archives, release workflows, public-repository presentation assets,
pane materials, spacing, transitions, or other visual polish merely because
those surfaces are adjacent.

Work 5 is therefore narrowed to making native the reference environment's default local launcher
while preserving an explicit terminal escape hatch. Its initial inventory
covers the development command and local `mandatum` / `mandatum-native`
executable seams only. If a local default cannot be selected without changing
tracked public-distribution surfaces, the slice stops at that boundary.

Context: installer, release, rollout, and visual presentation need to describe
the finished product as one coherent public surface. Changing them throughout
the functional refactor would create churn and prematurely expose intermediate
build decisions in the public GitHub repository.

Consequences:

- existing installer, updater, release, archive, and rollout behavior remains
  untouched during interim slices;
- public GitHub presentation and visual materials are not opportunistic
  cleanup targets;
- daily use may still surface functional reliability bugs, but it does not
  reopen the shelved visual-material roadmap;
- the concluding build phase incorporates the deferred public and visual work
  together.

## the primary user-Local Interactive Shell Selects Native Without Replacing The Terminal Tool

Status: accepted (2026-07-24)

Decision: the reference environment's interactive zsh defines `mandatum-native` as the existing
absolute-manifest native development command, routes the no-argument
`mandatum` name through it, and exposes `mandatum-terminal` as an explicit call
to the unchanged installed terminal release. The launcher does not change
directories, so the caller's current project remains the workspace context.
Non-interactive shells continue resolving `~/.local/bin/mandatum`.

Context: the production workspace already owned the stable
`cargo run -p mandatum-native --bin mandatum-native` seam, while the reference environment's PATH
contained only the installed `mandatum` terminal release. Replacing or renaming
that file would couple a personal daily-driver switch to the frozen installer,
updater, archive, and automation contract.

Rationale: an interactive-shell route is the narrow local-only boundary. It
makes the native product the reference environment's ordinary local launch without changing a
tracked executable, Cargo target, installer destination, updater lookup,
release archive, or non-interactive command used by automation.

Consequences:

- interactive `mandatum` and `mandatum-native` launch the native product;
- `mandatum-terminal` retains terminal help, version, update, SSH, headless,
  and recovery behavior;
- the installed terminal binary stays byte-identical and future terminal
  updates do not overwrite the interactive native routing;
- native help/version parity is not invented in this slice; terminal
  information and update operations remain explicit through
  `mandatum-terminal`;
- public/distribution and visual surfaces remain deferred.

Verification: a clean interactive zsh resolved all three names as functions,
`mandatum --font-info` returned the bundled JetBrains Mono profile from the
native package while preserving `/private/tmp` as the caller directory, and
`mandatum-terminal --version` returned `mandatum 0.2.0`. Focused native and
terminal release builds, 16 native shell tests, five terminal distribution
tests, and conformance passed. The final repository gate and displayed launch
checks are recorded in `docs/verification.md`.

## First-Run Escape Is A One-Shot Consumed Workspace Action

Status: accepted and completed (2026-07-24)

Decision: while the first-run note is visible, an exact bare Escape dismisses
the note, requests a redraw, and is consumed before terminal routing. Every
other dismissal action continues through its existing route, and Escape
returns to normal child or modal routing immediately after the note closes.
The shared `AppState` owns the exception so native and terminal frontends
retain identical product behavior.

Context: the first native daily-drive after Work 5 dismissed the note with
Escape. The same byte reached the reference environment's vi-mode zsh, switched it out of insert
mode behind the overlay, and caused the following `pwd` characters to edit and
execute a prior history command. A second fresh-workspace run proved the path:
after Escape, a leading `i` was consumed as zsh's insert-mode command while the
following `printf` executed.

Rationale: the visible orientation note makes dismissal an explicit,
short-lived workspace interaction. Consuming its conventional Escape action
prevents hidden child-state mutation without turning the note modal or
weakening the terminal-soul rule for ordinary input.

Consequences:

- first-run Escape cannot alter shell editing mode or another child state
  behind the note;
- ordinary typing, paste, composition, pointer actions, and configured
  workspace chords still dismiss and continue;
- opening any modal clears the note, so the visible modal owns its Escape
  route and the welcome note cannot reappear behind it;
- a second Escape follows the normal child or active-modal route;
- renderer implementation, launcher, persistence, packaging, release, and the
  visual-material roadmap remain unchanged; the displayed dismissal guidance
  now states the functional exception.

Verification: a focused RED test first observed Escape reach the no-PTY child
route. The implemented regression proves the first Escape is consumed, the
next Escape resumes child routing, Ctrl+P still opens the palette and its
following Escape closes it, and an ordinary first key still follows the child
route. The final displayed native check and repository gate are recorded in
`docs/verification.md`.

## Native In-App Visual Polish Is The Next Product Phase

Status: accepted; implementation in progress (2026-07-24)

Decision: begin production-grade visual polish as the next native product
phase. In-app typography, materials, hierarchy, density, focus, overlays,
workflow surfaces, motion, accessibility, and visual verification are now
active product work. Installer, updater, release, rollout, public-GitHub
presentation, and marketing material remain shelved and are not prerequisites
for improving the reference environment's daily-driver experience.

The phase follows `docs/visual-polish-plan.md`. Its visual direction is a
"quiet instrument": a dense graphite workbench in which terminal content is
the visual center, chrome is restrained and structural, and color communicates
navigation or changed state rather than decoration.

Context: the native frontend has crossed the reference-environment daily-driver bar and
the first functional hardening defect is fixed. A displayed audit of the
current Welcome, Palette, Help, and multi-pane states found one typographic
voice, touching box-glyph borders, weak material separation, and inverse-video
selection. The accompanying source audit found task, agent, and artifact detail
flattened into labeled strings before native rendering. Treating production
polish as a late marketing concern would leave a cornerstone of the daily-use
product deferred for the wrong reason.

Rationale: in-app polish and public presentation solve different problems.
The former improves comprehension, confidence, state recognition, and repeated
daily use; the latter packages and markets a finished product. Mandatum's
architecture already permits richer native presentation through typed
`mandatum-scene` extensions while preserving `CellProgram` as the maintained
terminal fallback.

Consequences:

- `docs/visual-polish-plan.md` is the ordered authority for the phase;
- the first capability family freezes canonical visual scenarios, records
  candidate tokens and current acceptance evidence, and defines the
  scene-owned dual-geometry contract before changing rendered pixels;
- native presentation consumes typed scene meaning and must not parse
  `detail_lines()` or reconstruct product semantics in `gpu.rs`;
- `CellProgram`, the terminal frontend, L1-L5, and one shared state machine
  remain unchanged architectural constraints;
- dark is the flagship theme, but light and high-contrast must become coherent
  complete themes before the phase exits;
- motion explains state changes, direct manipulation remains immediate, and
  reduced motion reaches the stable end state without animation redraws;
- fixed-reference macOS visual baselines supplement rather than replace
  portable semantic, geometry, contrast, clipping, and hit-target gates;
- decorative gradients, glow, vibrancy, custom titlebar work, marketing
  screenshots, and a theme marketplace remain optional later work.

Verification impact: documentation alone does not establish a polished result.
Each capability family must run its focused semantic and native-renderer tests,
the representative displayed scenario matrix when pixels change, and the full
`./ci/gate.sh` after final documentation synchronization. Baseline images may
change only through explicit human acceptance; CI must never auto-regenerate
them.

## Fixed-Reference Visual Evidence Fails Closed

Status: accepted (2026-07-24)

Decision: the `macbook-pro-metal-scale2` profile means a genuinely scale-2
native client and ScreenCaptureKit frame. The capture tool must refuse a
scale-1 window or display rather than upscale it and relabel the result.
Canonical `narrow` is narrow pane geometry inside the same fixed
1600 x 1200 physical, 800 x 600 logical, 102 x 35 scene; smaller-window cases
belong to the later pairwise matrix.

Context: Phase 1 implemented the real-host catalog and fixed capture/diff
interfaces while the initial capture attempt saw only `external reference display` at
backing scale 1.0. ScreenCaptureKit can resample output dimensions, but those
pixels do not prove the accepted scale-2 font rasterization, rounding, or
compositor path. Treating that file as the fixed baseline would make exact
metadata dishonest. The plan also named `narrow` while requiring one fixed
surface for every canonical baseline, so the scenario needed one unambiguous
interpretation.

Rationale: reference artifacts are valuable only when their environmental
identity is true. A fail-closed preflight turns a missing display condition
into an explicit operational prerequisite instead of a silent weakening of
the evidence. Keeping every canonical image on one fixed surface makes
metadata, comparison, and review coherent while still exercising narrow pane
behavior.

Consequences:

- capture requires an active 2.0-backing-scale display with at least
  800 x 600 logical pixels;
- the scenario window and ScreenCaptureKit frame are validated independently;
- candidates remain ignored local evidence until explicit acceptance;
- `compare` writes no files;
- `accept` requires a nonblank rationale, rejects a dirty source record, and
  never replaces the optional mask implicitly;
- no scale-1 baseline is accepted for the fixed profile; and
- Phase 1 closed only after all 11 fixed-reference baselines were captured and
  explicitly accepted from clean source commit `ebd7ee4`.

Verification: the portable catalog and visual-diff tests passed, the Swift
capture script typechecked, and the fixed-profile capture command first
refused the active scale-1 display before writing a candidate. After the LG
display entered its genuine 1720 x 720 logical / scale-2 mode, all 11
1600 x 1200 client-surface images recorded the fixed metadata contract from
clean source commit `ebd7ee4`; explicit acceptance succeeded and every strict
comparison returned SSIM 1.0 with zero changed or masked pixels. Visual review
also caught a missing context-menu capture before acceptance; the lab now
defers scenario driving until initial native geometry settles.

## Native Presentation Meaning And UI Tokens Stay Typed

Status: accepted; Phase 2 complete (2026-07-24)

Decision: native visual presentation is one typed capability family spanning
`Theme.ui`, coherent viewport input, scene-owned logical geometry and semantic
nodes, and a pure bounded renderer plan. Terminal ANSI identity remains owned
by `TerminalPalette` and `CellProgram`; native chrome colors are direct UI
tokens. The product state machine continues to build both native meaning and
the honest terminal projection in the same `WorkspaceScene`.

Context: Phase 1 froze the required dual-geometry and semantic identity
contract, but production scenes still exposed only cell geometry to the
renderer. Implementing materials directly in `gpu.rs`, deriving item identity
from labels or positions, or reusing ANSI slots for chrome would create a
second presentation authority and make resize, scale, transition, pointer, and
future accessibility behavior disagree.

Rationale: one coherent `ViewportMetrics` value gives scene layout a single
rounding boundary. Fixed-point logical rectangles and opaque structural ids
make identity deterministic across scale and geometry changes. A pure
headless plan lets exact ordering, clipping, transitions, text scopes, token
selection, and resource ceilings fail before GPU allocation. Keeping
`CellProgram` unchanged preserves the maintained terminal frontend while
allowing native presentation to deepen in later phases.

Consequences:

- UI palette, typography, spacing, radius, elevation, opacity, selection, and
  motion values are typed and independently configurable from terminal color;
- built-in themes must pass app-owned contrast checks and explicit low-contrast
  user overrides warn rather than silently changing the user's value;
- app-built scenes carry stable semantic nodes, terminal projections, logical
  hit targets, exact PTY viewport mappings, transition targets, and
  dependency-free accessibility nodes/actions;
- native presentation preparation enforces exact parent/clip containment and
  aggregate command, node, text-scope, and transition ceilings;
- multi-metric text is limited to the four provisioned static faces and its
  cache identity includes metric generation and slot;
- Phase 2 defines accessibility events but keeps them inert until Phase 7
  supplies the native projection and same-frame action map; and
- isolated visual fixtures normalize random root identity only in the
  review-facing scene so strict baseline comparison is repeatable.

Verification: built-in contrast, config compatibility, 1.0/2.0 geometry,
stable presentation identity, unchanged `CellProgram`, exact native-plan
ordering/clipping/resource limits, multi-metric baseline/cache identity, and
scenario determinism have focused tests. The displayed native token sampler
showed every direct UI color role, `./ci/native-frontend.sh` passed, and the
source-and-documentation repository gate reported `GATE GREEN`. Fixed-reference
capture from clean source commit `6979318` replaced random fixture identity and
corrected the seven Phase 1 images affected by an external compositor
color-state transition. After explicit reviewed acceptance, all 11 strict
comparisons returned SSIM 1.0 with zero changed or masked pixels. The final
gate and push evidence are recorded in `docs/verification.md`.

## Workspace Chrome Uses Typed Materials And Dual-Coordinate Input

Status: accepted and displayed (2026-07-24)

Decision: visual Phase 3 implements the everyday workspace as one typed
capability family. `WorkspaceScene` owns header/status rails, typed attention
and pane badges, compact focus, density, separator state, and floating state.
The pure native plan resolves those semantics into bounded materials and text
scopes. Native pointer events preserve both logical position for workspace
chrome and cell coordinates for terminal children. The native window starts at
1200 x 800 logical pixels, enforces a 720 x 480 minimum, and titles itself from
the active project.

Context: the first implementation pass exposed three boundary defects during
aggregate review. Density was plumbed but produced identical output; the native
shell discarded the six-logical-pixel separator target by collapsing pointer
input to cells; and GPU compatibility paint could cover new rails/chips or
composite floating shadows over later panes. A renderer-side glyph-content
filter also tried to identify legacy border decoration by parsing characters.

Rationale: Phase 2 deliberately established typed dual geometry and a pure
presentation plan so Phase 3 would not need renderer inference or a second
layout/input authority. Native polish is trustworthy only if the product route
actually consumes those contracts. Terminal parity still requires an honest
cell projection and cell-coordinate child input.

Consequences:

- compact/comfortable density changes native title-rail breathing room without
  consuming a PTY row or changing `CellProgram`;
- a one-logical-pixel separator has a six-logical-pixel native hit target, and
  redraws occur on hover-identity changes rather than every pointer move;
- typed `PaneDecoration` scope replaces glyph parsing, while compiled
  `NativeTextScope` projections own app-chrome text color;
- modern app-owned chrome/default pane backgrounds do not repaint semantic
  materials, but terminal cursor, selection, raster, and explicit backgrounds
  remain authoritative;
- tiled panes and separators precede raised floating panes; later floating
  surfaces occlude earlier bounded shadow fragments; and
- the active project name is a typed header field distinct from session and
  composed header text.

Verification: focused RED/GREEN and aggregate-review corrections are green
across core, scene/`CellProgram`, app, both renderers, native shell,
frontend-parity, and the exact canonical-scenario plan test. The real native
Metal route was reviewed at backing scale 2 through one, split, stacked,
floating, zoomed, 720 x 480 minimum, and restored states. All 11 canonical
candidates from clean source commit `7221937` were individually reviewed and
explicitly accepted. A fresh repeated performance series was explicitly scoped
out when it stopped adding useful confidence; existing reference preparation
evidence and bounded plan/resource tests remain the regression guard. The
display was restored and verified at 3440 x 1440 / scale 1 / 60 Hz. The
synchronized final gate and completion commit are recorded in
`docs/verification.md`.

## Overlay Family Uses One Typed Material Stack And Three Interaction Grammars

Status: accepted and displayed (2026-07-24)

Decision: Palette, Timeline, Session Map, Prompt, Search, Help, Welcome, and
Context Menu share one scene-owned overlay family. Modal surfaces use the
shared scrim and raised shell; Welcome keeps its non-modal dismissal behavior;
Context Menu remains anchored without a viewport scrim. Stable semantic item
keys, constrained geometry, inset bands, soft selection, a leading indicator,
and right-aligned hints cross the typed presentation boundary.

Rationale: implementing one overlay at a time would preserve visible drift and
invite renderer-side inference. The Phase 2 presentation contract already
provides stable identity, logical geometry, ordered materials, and exact hit
targets, so the whole family can deepen without changing product state or
weakening the terminal fallback.

Consequences:

- the GPU orders workspace paint, modal scrim, overlay depth/materials,
  overlay cursor, and text explicitly;
- `OverlayDecoration` suppresses terminal box glyphs on native without parsing
  glyph content;
- item labels/details/hints retain their compiled cell styling while native
  selection material replaces inverse-video row backgrounds;
- Context Menu click handling uses the exact constrained presented shell; and
- the MacBook Pro built-in Retina display is the fixed reference for this and
  subsequent visual work. One-cell vertical overlay rhythm remains documented
  spacing debt.

Verification: aggregate review found and the implementation corrected cursor
layering, item recoloring, rounded-band overlap, constrained Context Menu hit
geometry, degenerate shells, stable Help keys, and clipped key hints. The
representative native matrix was reviewed and explicitly accepted from clean
source commit `1988b0b`; the full repository gate ended `GATE GREEN`.

## Workflow Presentation Is Typed, Bounded, And Scene-Owned

Status: accepted and displayed (2026-07-24)

Decision: task, agent, approval, and artifact panes expose one scene-owned
typed workflow projection. `WorkflowRow` and stable `WorkflowNodePart`
identities carry role, tone, and bounded fallback text; the native plan maps
those roles to compact badges, contained callouts, console/inspector material,
and artifact canvas material without parsing labels. `detail_lines()` formats
the same rows for the terminal frontend.

Rationale: styling `detail_lines()` by prefixes would create renderer-owned
product meaning and let native and terminal geometry drift. Dropping offscreen
nodes during resize would also make animation and accessibility identity
unstable. Artifact pixels need a fail-closed semantic canvas contract rather
than a best-effort lookup that can silently discard a ready surface.

Consequences:

- task status is typed separately from its label, including detached, waiting,
  running, succeeded, diagnostic, and failed states;
- approval and failure emphasis is limited to their callouts and compact
  badges rather than the entire pane;
- agent output lines are bounded before entering live runtime state and the
  scene exposes only a bounded tail with an honest overflow marker;
- `$`-prefixed raw output is never treated as a command channel;
- hidden semantic nodes preserve identity across resize without painting; and
- ready artifact preparation rejects a missing, wrongly typed, hidden, or
  geometrically mismatched canvas when visible geometry is required.

Verification: focused scene, cell-program, renderer, native-plan, app, and
real-host tests cover typed mapping, compact badge geometry, resize identity,
long content, terminal fallback, failure/approval attention, and artifact
geometry. `./ci/native-frontend.sh` passed and the full repository gate ended
`GATE GREEN`. The representative `dense-workspace`, `attention`, and
`artifacts` references were visually reviewed and explicitly accepted from
clean source commit `d17bdd2` on the MacBook Pro built-in Retina display;
strict comparisons returned SSIM 1.0 with zero changed or masked pixels.

## Motion Intent Is Typed And Runtime Polling Does Not Drive Paint

Status: accepted architecture and displayed evidence (2026-07-25)

Decision: Phase 6 adds motion as scene-owned typed eligibility plus
renderer-local deterministic presentation progress. `SceneMotionPolicy`
expresses reduced-motion and direct-geometry frames. `TransitionTarget` binds
stable semantic nodes to Focus, Selection, Overlay, PaneGeometry, or
ApprovalArrival and to the only permitted properties: geometry, opacity, and
scale. Approval arrival also carries a monotonic sequence so distinct requests
on one pane cannot collapse between paints. Overlay opacity covers its
descendant material and cell-owned text family; scale and pane geometry apply
only to native material-backed commands. Cell-owned glyph placement, child
output, and artifact raster placement stay direct. Overlay close snaps because
the new scene no longer owns its glyph rows and the adapter must not retain an
empty shell.

The native renderer samples an injected monotonic instant and retains no
durable product truth. Equal plans do not restart motion. Interruption and
reversal begin at the current sampled presentation and converge on the newest
scene. A new approval receives one brief inward emphasis, then its typed amber
callout remains statically high-salience. Reduced motion and direct
pointer/resize geometry snap immediately and schedule no transition frames.

Context: the prior waiting-approval treatment changed scene output from a
wall-clock pulse, and the native loop coupled its periodic heartbeat with
redraw opportunities. That made product scene equality time-dependent and
could repaint a static workspace for child-exit polling. Independently moving
native material also creates input risk whenever rendered geometry lies
between scene-owned hit-target endpoints.

Rationale: the scene must name why a stable semantic surface may move, while
the adapter alone owns transient pixels. An injected monotonic clock makes
start, midpoint, completion, interruption, reversal, and convergence exact in
tests without putting time in product state. A separate visible-state
generation lets the platform distinguish real scene change from snapshot
production or heartbeat cadence. Suspending pointer admission during
hit-bearing interpolation preserves the exact-painted-frame interaction
contract.

Consequences:

- `FrameSnapshot` carries both an always-advancing production revision and a
  scene generation that advances only for visible state change;
- `FrontendHost::heartbeat` reports whether child-exit work changed that
  generation rather than implicitly requiring repaint;
- the native shell schedules the earlier of the renderer animation deadline
  and the 250 ms heartbeat and redraws only when either active motion or real
  scene change requires it;
- typing, output, pointer drag, live resize, and terminal child interaction
  remain authoritative and uninterpolated;
- pane and overlay motion cannot admit pointer input against geometry between
  stable scene endpoints;
- reduced motion retains static semantic cues while scheduling no transition
  frames; and
- animation progress, deadlines, scene generation, and pointer suspension are
  live presentation/runtime state and are never serialized.

Verification: focused deterministic source tests cover typed target
construction, stable families, direct and reduced policy, start/mid/end,
interruption, reversal, convergence, approval arrival, scheduling, redraw, and
idle heartbeat behavior. Displayed motion evidence, final measured values,
explicit reference acceptance, and the complete repository gate are recorded
in `docs/verification.md`. The real Metal approval-arrival matrix was captured
from clean source commit `4732ba8` on the MacBook Pro built-in Retina display;
three-run medians met the motion and idle budgets.

## Native Visual Polish Closes With Physical Improvements And Risk-Driven Evidence

Status: accepted (2026-07-25)

Decision: close the native visual-polish program with one finishing slice that
improves the product rather than creating a Phase 7/8 qualification ceremony.
The slice completes coherent light and high-contrast terminal palettes, applies
typed interface metrics through native shaping, gives overlay controls a
scene-owned two-cell rhythm and matching hit targets, and validates a realistic
18-point usage envelope. Dark remains the flagship theme.

Native macOS accessibility projection and VoiceOver support are explicitly
deferred. Mandatum retains keyboard operation, non-color state cues, focus
semantics, and dependency-free typed accessibility nodes/actions, but those
contracts are not described as platform accessibility until a same-frame
AppKit projection and action route exist. Pane title rails also remain one cell
high because increasing them without a new layout contract would consume or
overlap PTY geometry; the finishing slice does not hide that boundary behind a
nominal spacing target.

Context: the prior plan still described complete theme/accessibility parity and
a separate integration phase even though the accepted native-first decision
had retired those admission gates. Meanwhile, app-owned text still shared the
terminal metric in the GPU path, overlay controls inherited one compressed
terminal row, and the non-dark terminal palettes were incomplete. Those are
visible daily-driver defects. Repeating motion, idle, recovery, 1,000-resize,
or full-catalog screenshot programs would not test their owning seams.

Rationale: verification earns its cost only when it can change a decision.
Portable tests should exercise changed contracts, displayed captures should
cover representative changed pixels, and specialized probes should run only
for the subsystem they measure. A freshly accepted baseline does not gain
confidence from an immediate identity comparison against itself. Native
VoiceOver work is a real platform feature, not a checkbox that can be satisfied
by typed scene data alone.

Consequences:

- light and high-contrast are coherent built-in themes with app-owned contrast
  coverage, while arbitrary child-terminal ANSI combinations remain outside
  Mandatum's contrast guarantee;
- native app chrome can use its declared role-specific metrics without changing
  terminal output metrics or widening the font-family boundary;
- overlay item, input, and footer controls use full two-cell logical targets,
  while pane title-rail height remains named debt;
- the six displayed checks are limited to dense workspace and Palette across
  dark, light, and high-contrast, with only changed dark references accepted;
- no post-accept SSIM identity rerun, 29-cell cross-product, repeated Phase 6
  motion/idle/recovery evidence, or unrelated 1,000-resize stress is required;
- a focused frame-preparation sanity check may trigger deeper profiling but is
  not relabeled as three-run fixed-reference authority;
- Phase 8 remains retired, and the next product family is named task and
  dev-server recipes rather than another visual-polish ceremony; and
- installer, updater, release, rollout, and public presentation were outside
  this visual slice; the later public-distribution decision supersedes that
  separate deferral.

Verification: three independent cold reviews found and drove corrections for
an overlay empty-state overwrite, semantic-emphasis erasure, line-box
clipping/overlap, and an invisible light ANSI 15 value. Six serialized native
captures and the final 18-point Metal smoke were visually reviewed; one
contaminated concurrent capture and one overlapping candidate were rejected
and rerun after correction. Focused source suites and a current-surface
frame-preparation sanity check passed. Exact capture diffs, diagnostics,
measurements, and the intentionally scoped-out probes are recorded in
`docs/verification.md`. The authoritative synchronized `./ci/gate.sh` ended
`GATE GREEN`.

## Public Distribution Uses Split Signed Archives And A Verified Update Boundary

Status: accepted, release pending (2026-07-25)

Decision: present Mandatum as a public pre-release product with a concise
capability-first README and a truthful limitations section. Version `0.3.0`
prepares the first native macOS distribution without breaking the published
`0.2.0` updater contract: each architecture keeps the common
`mandatum`/approval-bridge archive, while macOS receives a separate
`mandatum-native` archive. The installer verifies exact members and SHA-256 for
every archive. When native assets exist, it also verifies every executable's
Developer ID signature against the pinned Apple Team ID before replacing
anything. Partial replacement restores the prior binary set, and an equal
published version is a no-op rather than a possible same-version downgrade.

Release automation pins every GitHub Action to an immutable commit, requires a
tag that exactly matches the root semantic version, signs all macOS executables
with the hardened runtime, submits them to Apple notarization, and fails
publication when any required credential or verification is absent. Until the
first native release exists, the public installer detects the missing native
asset and safely installs only the already-published terminal frontend.

The same public-readiness boundary removes raw real-agent transcripts and
personal machine identifiers from the current tree, neutralizes visual
baseline profile names, enables private vulnerability reporting and secret
scanning, and replaces private build-diary language at the repository entrance
with current product and roadmap documentation.

Context: the repository was already public, but its front page said there was
no public audience, release archives omitted the native product, test fixtures
contained private workstation metadata, and GitHub private vulnerability
reporting was disabled despite the security policy linking to it. A public
release also makes the approval socket and update supply chain advertised
security boundaries. Review found that the fallback approval directory trusted
a shared `/tmp` path and that checksums downloaded beside release archives did
not independently exercise the configured Developer ID identity.

Rationale: public claims must follow shipped artifacts and fail-closed
boundaries. A separate native archive preserves the old updater's allowlist,
while signature/team verification makes Developer ID signing meaningful at the
client. Approval settings and sockets live below a per-user `0700` root under
the sticky system temporary directory, so the gated protocol never traverses a
shared or writable project directory. Gated launch also resolves and validates
the bridge executable before Claude starts. Synthetic parser fixtures preserve
behavioral coverage without publishing irrelevant personal state.

Consequences:

- the first native release cannot publish until valid Developer ID and Apple
  notarization credentials are configured in GitHub Actions;
- an equal-version install is a no-op only when every selected platform binary
  is already present, so adding the native archive cannot strand an earlier
  terminal-only installation;
- raw command archives do not claim a stapled or offline notarization ticket;
- the default agent policy is documented precisely: Bash gates by default,
  while reads and writes auto-allow;
- repository history still contains the superseded raw fixtures; removing that
  historical privacy residue would require a disruptive history rewrite and is
  not silently folded into this release-preparation commit; and
- direct-to-main remains the documented solo-maintainer policy, with the full
  repository gate required before each push and the tag workflow rerunning it
  before release publication.

Verification: focused parser, bridge-resolution, and private-runtime tests; the
distribution installer/rollback/retained-recovery/current-version smoke;
distribution/update tests; locked native and terminal builds; exact Developer
ID and notarization-status workflow checks; shell/workflow syntax validation;
conformance; and documentation trace checks passed during implementation. The
synchronized authoritative `./ci/gate.sh` ended `GATE GREEN`. A real
signed/notarized release remains the final remote authority and is explicitly
pending valid Apple credentials.

## User Input Overtakes Runtime Backlogs

Status: accepted (2026-07-25)

Decision: keep `AppEventSender` as the sole frontend/runtime ingress, but give
neutral `InputEvent` values a priority lane. Each input also places an internal
wake marker on the runtime lane. The receiver checks input first, then runtime
events; a blocking receiver still waits on only the runtime lane and the marker
wakes it for input. Shared queue accounting claims a marker when its input is
consumed early and discards that marker later without losing or duplicating an
event.

Context: the public-preparation push ran the authoritative gate successfully on
macOS, then the Linux GitHub runner twice failed the live `yes` flood regression
because a queued quit chord did not arrive within two seconds. Tightening the
per-pane PTY credit cap from 256 KiB to 64 KiB was a useful memory and parse
backlog improvement, but the second runner failure proved that FIFO scheduling
still could not defend input latency under variable parser and host load.

Rationale: extending the wall-clock assertion would hide a real product defect,
and reducing the PTY cap to one chunk would trade throughput for an indirect
latency hope. A priority input lane states the actual invariant directly: user
input must not wait for runtime output. The runtime marker preserves the
single-blocking-wait architecture and existing platform-neutral wake callback.

Consequences:

- key, paste, pointer, focus, and resize input preserve FIFO order with each
  other while overtaking queued PTY, agent, and artifact events;
- runtime events retain their own FIFO order, flow credits, generation tokens,
  and bounded drain behavior;
- the 64 KiB per-pane PTY cap remains because it independently bounds memory
  and parse work; and
- `AppEventReceiver` is app-private and consumed only by the runtime engine or
  focused tests, so platform and renderer boundaries do not change.

Verification: the event tests prove input overtakes a queued runtime event,
input bursts preserve order and wake coalescing, concurrent wake transitions
do not strand events, and PTY/agent producers retain the same app-owned sender.
The live flood test proves quit and shutdown remain responsive while flow
credits stay within the cap. The final synchronized `./ci/gate.sh` ended
`GATE GREEN`.

## Distribution Ships One App Bundle And One Launcher

Context: the previous public contract shipped three raw command archives per
platform, asked users to manage `mandatum`, `mandatum-native`, and
`mandatum-approval-bridge` on `PATH`, and required Developer ID signing plus
Apple notarization credentials that were never configured, so no native
release could ever publish. The README led with Cargo, certificates, and
bridge internals. That is a developer-toolchain contract wrapped around what
is actually a desktop application.

Decision: releases publish exactly one macOS artifact: a universal
`Mandatum.app.zip` with a SHA-256 sidecar, assembled by
`packaging/package-app.sh` from per-architecture CI builds joined with
`lipo`. The bundle carries the approval bridge as a sibling of the app
executable in `Contents/MacOS` (the connector already resolves siblings
first) and a POSIX-sh launcher at `Contents/Resources/mandatum`. `install.sh`
verifies the checksum, installs the app to `/Applications` (or
`~/Applications`, or `MANDATUM_APP_DIR`), and copies the launcher to
`~/.local/bin/mandatum` last, so a failed app swap leaves the previous
updater usable. `mandatum` opens the app in the current directory;
`mandatum update` re-runs the hosted installer, which refuses downgrades and
restores the prior app if a swap fails. Binaries are ad-hoc signed; the
project claims checksums and CI provenance, not notarization. The terminal
frontend (`mandatum-app`'s `mandatum` binary) remains a development surface
and is no longer distributed.

Rationale: the user contract is download, open, update. Every removed
concept (Cargo, toolchains, certificate pinning, bridge placement) was
machinery the product should carry, not the user. Ad-hoc signing is honest
about what exists today: the pinned-team verification path could never
succeed without credentials, which made the old installer's strictness
theater. Conformance now enforces the new surface: allowlisted release
build targets, allowlisted bundle executables, checksum-before-extract, no
developer tokens in the installer, and launcher-installed-last.

Consequences:

- pre-app installations cannot self-update across the format change (their
  embedded updater looks for retired tar.gz assets); the migration path is
  rerunning the one-line installer;
- browser downloads hit a Gatekeeper warning because nothing is notarized;
  the README documents the System Settings allowance and the installer path
  that avoids quarantine entirely;
- `ci/distribution-smoke.sh` now exercises the real packaging script,
  fixture app bundles, launcher update, downgrade refusal, and swap
  rollback; and
- the release workflow needs no repository secrets.

Verification: the distribution smoke passes locally (install, idempotent
reinstall, launcher-driven update, downgrade refusal, swap rollback with
restore), conformance passes against the new surfaces, and the packaged
bundle launches with `--font-info` intact from inside `Mandatum.app`. The
release pipeline's own smoke installs the published asset on both Mac
architectures and exercises `mandatum update` end to end.

## Braille Ships As A Generated Metric-Matched Fallback Face

Context: CLI spinners emit Braille block characters (U+2800-U+28FF).
Neither the bundled JetBrains Mono nor any monospace font on stock macOS
covers the block, so cosmic-text's whole-database fallback scan picked
Apple Braille, whose 0.692 em advance can never match the 0.6 em cell.
Every spinner frame therefore failed admission and took the anchored
decomposition path. The previously proposed lever, preferring
`Family::Monospace` during fallback, was verified to be a no-op: glyphon
pulls cosmic-text without the `monospace_fallback` feature, no fontdb
monospace face covers Braille, and the database's monospace family is
already the primary.

Decision: bundle a fifth face, `Mandatum Braille`, generated from scratch
by `packaging/make-braille-font.py` (fontTools): 256 glyphs drawing the
dot pattern of `codepoint - 0x2800` on the standard 2x4 grid, with
JetBrains Mono Regular's metrics (1000 UPM, 600 advance, 1020/-300 typo
ascender/descender) and `post.isFixedPitch` set. The generated TTF is
committed. `resolve_in_database` loads it into every resolved catalog
after removing any same-named system face, and `create_font_system`
installs a custom `Fallback` that answers `Script::Braille` with the
bundled family and delegates everything else to `PlatformFallback`.
Script fallback runs before the common list and the database scan, so the
override is deterministic.

Rationale: generating the face avoids third-party outlines entirely (no
OFL reserved-name renames, no license text to carry) and is the only
option that fixes the metrics as well as the look: subsetting an existing
Braille font would inherit a foreign advance. Primary-font semantics are
untouched, because only the Braille script's fallback list changes.

Consequences: the app bundle grows by ~32 KB; the fallback report now
names `MandatumBraille-Regular` where it previously named Apple Braille;
regenerating the face requires fontTools, but only when the design
changes, since the artifact is committed.

Verification: `braille_spinner_glyphs_shape_admitted_from_the_bundled_fallback`
pins that a spinner run shapes as one admitted grid-aligned run whose
observations all resolve to the bundled face, and fails if the fallback
override is disabled. The full native-renderer suite passes.

## Launch Without A Working Directory Falls Back To Home

Context: a Finder- or Dock-launched app inherits `/` as its working
directory. `/` is a real directory, so the spawn-cwd guard accepts it,
but no project lives there and `/.mandatum/workspace.json` is not
writable, so every task in such a session failed. Refusing directories
without a VCS marker was rejected because non-VCS project directories are
legitimate; a project picker is the eventual product answer and remains
open.

Decision: `AppConfig::from_current_dir` routes through
`resolve_project_path`, which treats a cwd of exactly `/` as "no
directory chosen": it redirects the project path to `$HOME` and surfaces
a status-line warning naming the substitution, matching Terminal.app's
launch behavior. Any other cwd passes through untouched, and if `$HOME`
is unset the original cwd is kept with a warning rather than guessing.

Consequences: intentionally launching Mandatum with `/` as the working
directory is no longer possible; that session now opens in `$HOME`. Given
that tasks could never run from `/`, nothing functional is lost.

Verification: three unit tests pin the pass-through, the redirect with
its warning, and the no-`$HOME` case.

## In-App Appearance Overlay And The First Config Writer

Context: adjusting the theme, background color, or font required hand-
editing `config.toml` and, for fonts, a restart. The config boundary was
deliberately read-only: no production code path wrote the file. An in-app
control surface needed both a live-apply path and a persistence story
that would not trample a hand-maintained config.

Decision: a new `adjust-appearance` command (palette-searchable; the
reserved `i` letter stays free) opens a modal Appearance overlay defined
in `mandatum-scene` and painted in the shared cell program, so both
frontends render it (L1). Rows follow the index-selected list idiom:
the base theme cycles the built-ins as a complete snapshot, matching
`[theme] name` semantics, and the terminal background adjusts by HSL
channel over cell-rendered gradient bars whose stops preview the exact
resulting colors. Font rows appear only when a frontend declares its
font facts (resolved family, size, and cycle candidates); the terminal
frontend never does, since it inherits the host terminal's font. Font
changes flow to the native frontend as a `FrontendEffect::ApplyFont`:
size reuses the cheap metric path (shaping-cache flush, no atlas
rebuild), family resolves a fresh profile and swaps it through the
device-recreation path, and failure degrades to a status warning with
the previous font kept — after every attempt the frontend re-declares
the resolved truth, so the overlay's rows cannot drift from the screen.

Every adjustment persists to the user config file through the codebase's
first production config writer (`config_write.rs`): toml_edit edits only
the managed keys (`[theme] name`, `[theme.terminal] background`,
`[font] family`, `[font] size`), preserving the user's comments and
formatting, and replaces the file atomically by temp-then-rename. A
missing file is created with a short header; a file that is not valid
TOML is never overwritten — the write fails into a status warning while
the live session keeps the change. Project-level config still wins at
the next launch through the unchanged overlay order.

Consequences: `config.toml` is no longer strictly read-only at the
boundary, but the writer's surface is exactly the four managed keys.
Reload Config remains the live-apply path for hand edits to colors;
hand edits to `[font]` still need a restart, since reload does not
re-resolve fonts — the overlay is the live font path.

Verification: config_write round-trip tests (creation, comment
preservation, invalid-file refusal), appearance adjustment and overlay
scene tests, app-state interaction tests (input ownership, mutual
exclusion with other overlays, persistence wiring), a cell-program
painting test pinning truthful bar stop colors and marker contrast, and
a candidate-family enumeration test. Full local ci/gate.sh run.

## Encoded-Space Alpha Blending For Ghostty-Parity Text Weight

Context: the native surface preferred an sRGB texture format, so all
alpha blending — including glyph antialiasing — happened in linear
space. Linear coverage blending visibly thickens light-on-dark text
relative to macOS-native rendering; side-by-side with Ghostty (whose
macOS default is `alpha-blending = native`, i.e. encoded-space
blending, with font smoothing off), the same JetBrains Mono at the same
size read heavier and less refined in Mandatum.

Decision: the surface format preference flips to non-sRGB (sRGB remains
the fallback), so blending happens in encoded space — the space glyph
antialiasing is designed for and the platform-native default. No new
code path: every color seam already branched on `format.is_srgb()`
(quad/material byte decode, glyphon color mode, scrim compositing, the
artifact raster texture format), so the change selects the existing
encoded path rather than adding one. Ghostty's `linear-corrected` mode
(per-pixel alpha correction against the destination luminance) was
considered and rejected: it needs per-glyph background knowledge inside
the text shader, which glyphon does not expose, and encoded-space
blending is what stock Ghostty on macOS ships anyway.

Consequences: text weight matches the macOS-native/Ghostty look; the
theoretical cost is linear blending's freedom from hue-darkening
artifacts on antialiased edges between saturated complements, an edge
case the platform default also accepts. Existing fixed-reference visual
baselines predate the current theme and remain stale; new captures are
the comparison truth.

Verification: full local ci/gate.sh run; fresh context-menu, palette,
and typography scenario captures reviewed against the prior render and
Ghostty for weight, plus a pixel probe of captured surfaces confirming
theme bytes survive un-double-encoded (±1 compositor color-management
rounding only).
