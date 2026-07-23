# Architecture

## Goal

Mandatum separates durable workstation intent, live runtime state, terminal
state, scene composition, frontend rendering, workflow orchestration, and agent
state.

The architecture should let one engine support multiple frontends while keeping
product behavior testable from shell commands.

## Layer Map

```text
frontend adapters
  draw scenes, collect input, report hit targets and platform events

scene layer
  panes, bounds, terminal surfaces, overlays, selections, animations, status

runtime engine
  PTYs, tasks, agents, process events, reader threads, live status, recovery

terminal engine
  parser adapters, grid snapshots, scrollback, styles, cursor, capabilities

workflow layer
  task recipes, server recipes, agent launch intent, command history

command layer
  palette entries, key routing, action metadata, context-aware command targets

workspace engine
  projects, sessions, panes, layout, focus, durable intent, persistence schema
```

## Module Responsibilities

### `core`

Owns durable workstation state:

- workspace identity
- project identity
- session identity
- pane identity and kind
- layout tree, stacks, floating panes, zoom, focus
- durable task and agent intent
- core actions
- persistence schema and validation

`core` must not own live process handles, parser instances, threads, render
resources, frontend framework types, or platform event types.

### `commands`

Owns command vocabulary:

- command ids and labels
- command categories
- palette routing
- context-aware key resolution
- durable core action targets
- runtime command targets

Commands describe what can be invoked. They do not perform process I/O, draw UI,
or mutate layout except through `core`.

### `pty`

Owns PTY process mechanics:

- spawn intent
- process launch
- byte input/output
- resize
- child exit
- termination
- reader/writer/controller split
- backpressure signals

PTY events are raw runtime facts. They do not know about panes, rendering, or
product workflow beyond session identity.

### `terminal-vt`

Owns terminal state:

- parser adapter interface
- default parser backend
- terminal grid
- cell style
- cursor state
- scrollback
- resize
- terminal capabilities
- parser errors

The app and scene layers should consume snapshots and value types without
depending on a concrete parser backend.

### Runtime Engine

`crates/app/src/runtime_engine.rs` is the deep app-local Module for live
runtime state. Its Interface owns product-shaped operations and observations;
the terminal, task, and agent registries are low-level Implementations that do
not escape for production mutation.

It owns:

- terminal pane runtime registry
- task runtime registry
- agent runtime registry
- the unified input / PTY / agent event channel
- process event routing
- reader-thread lifecycle
- runtime tokens and replaced-runtime event rejection
- live status strings
- launch failures
- stop/rerun/restart behavior
- approval decisions against live agent controls
- active-session reconciliation and retirement
- transactional restore staging and activation
- typed lifecycle facts for fresh, deferred, detached, and not-replayed runtime
  outcomes; restore staging failures return a typed error and commit no facts

`AppState` owns the durable workspace fold, timeline, status copy, and
presentation coordination. It asks `RuntimeEngine` to perform live operations
and maps returned typed effects into those durable and visible concerns. This
keeps registry ordering, runtime replacement, and identity policy local without
moving process handles into `core` or introducing one generic registry trait
over three materially different runtime kinds.

Supporting Implementations remain in `events`, `process_events`,
`terminal_runtime`, `task_runtime`, and `agent_runtime`; `app_shell`,
`frontend`, `input`, and `persistence` remain adjacent orchestration Modules
(full module map: docs/repo-structure.md). The run loop is event-driven: one
unified channel (`AppEvent::Input | Pty | Agent`) behind app-owned
`AppEventSender`, a 250 ms heartbeat, and an 8 ms redraw cap. The sender can
invoke one frontend-neutral callback per non-empty queue interval; shared
queue accounting makes the last receive and next enqueue one race-safe state
transition. PTY readers remain bounded by flow-credit backpressure (256 KiB in
flight per pane).

The excluded native shell binds that neutral callback to
`EventLoopProxy<UserEvent>`. The proxy is a disposable platform notification;
the unified channel remains event truth, and the native shell drains it through
`FrontendHost` rather than owning a parallel runtime path.

Live runtime state is never serialized as durable truth.

### Scene Layer (`mandatum-scene`)

Owns renderer-neutral presentation:

- pane bounds and all pane-rect layout math (`scene::layout`)
- tiled, stacked, floating, and zoomed surfaces
- terminal grid surfaces (`TerminalSurface`: windowed styled cells, cursor,
  scrollback viewport, selection)
- task and agent summaries (`PaneContent`)
- command palette view model
- status strips
- overlays
- hit targets
- neutral input event types (`scene::input`: keys, pointer, paste, resize,
  focus; the app consumes these exclusively; the terminal frontend
  translates crossterm events into them in `crates/app/src/frontend.rs`)

The scene layer is the interface between product state and frontend adapters.
It is an engine-side crate (deps: `mandatum-core` + serde only) and never
depends on the terminal engine; the app's `scene_builder` converts engine
grids into scene surfaces.

### Frontend Adapters

Own rendering and platform input:

- terminal frontend (`mandatum-renderer`: the ratatui adapter over
  `mandatum-scene`; computes no layout, no direct terminal-engine
  dependency; shipped, v1)
- excluded native/GPU frontend (`spikes/frontend-wgpu`): a working winit shell
  over the real `FrontendHost`, with a scene-only GPU renderer; not shipped
- production native or platform-specific frontends (not admitted)

Frontend adapters should draw a scene and emit input/hit-test events. They do
not own product behavior.

The shipped terminal shell drives `FrontendHost` for workstation behavior. It
retains crossterm ownership, the terminal guard, input-reader lifecycle,
heartbeat and redraw scheduling, ratatui rendering, and OSC 52 encoding.

### Shared Frontend Host

`crates/app/src/frontend_host.rs` owns exactly one private `AppState` and its
`RuntimeEngine`. It accepts neutral input, exposes one blocking unified-event
wait plus a bounded nonblocking drain, performs child-exit heartbeat work when
the shell schedules it, and returns owned `FrameSnapshot` values containing
`WorkspaceScene`, `Theme`, and a monotonic snapshot-order revision. It also
drains typed effects in FIFO order, exposes quit state, and makes shutdown
behaviorally idempotent. It exposes no concrete runtime registry.

Snapshot revisions identify frame production order rather than semantic dirty
state: every frame call advances the revision. `frame()` retains the returned
scene's hit targets in `AppState`, and the terminal requests and renders that
same snapshot inside its draw callback, so pointer input resolves against the
most recently painted frame.

`FrontendHost::new_with_wake_callback` optionally installs a renderer-neutral
notification callback. Terminal input, PTY readers, restore-preserved input,
and agent forwarders all use clones of one crate-private `AppEventSender`; no
raw sender escapes. The callback coalesces while events remain queued and the
channel stays authoritative. No platform waker type exists in the app layer.

The excluded winit shell is the second exercised consumer of this boundary. It
binds the callback to `EventLoopProxy<UserEvent>`, translates platform events to
neutral `InputEvent` values, and paints the host's real scene header, terminal
pane, task pane with optional live output, agent pane, status strip, and command
palette. It also paints the product's Empty fallback from its scene-composed
cwd, restart-generation, and no-live-grid detail lines, plus the existing
context-menu area, rows, chord hints, and selection. The execution timeline is
also scene-bound: its resolved area, filter query, windowed durable-event rows,
selected index, and footer pass unchanged through the prepared GPU plan. The
session map follows the same boundary: its resolved area, ordered tree rows,
depth, glyph, label, live state, focus marker, badges, selection, and footer
remain app/scene-owned. The objective prompt is scene-bound too: its resolved
area, focused pane title, configured input, cursor location, and footer paint
without renderer access to app or runtime state. Session-output Search follows
the same rule: the prepared plan retains the scene's resolved area, live query,
grouped source labels, matched output text and char indices, selection,
overflow, footer, and row targets. The GPU adapter clips underlying pane glyphs
around that opaque modal while leaving the surrounding one-pane scene intact.
Generated Help follows the same boundary: the scene owns its resolved area,
live filter, ordered heading/entry rows, live key routes, selection, and footer.
The adapter paints those values with the existing semantic overlay roles and
clips base-pane glyphs around the opaque Help modal without consulting the
command table or keymap.
Its former `TerminalSession`, direct parser/input path, and `scene_bridge` are
removed; its window, platform-input translation, GPU, and paint-scheduling
state remain frontend-local.

A native shell may own a window, platform wake handle, DPI/IME state,
clipboard integration, GPU surface/device resources, glyph caches, and paint
scheduling. It may not own a second PTY/parser path, command router, approval
model, persistence model, or recovery policy. The full contingent sequence and
its stop/go gate are in
[native-gpu-implementation-plan.md](native-gpu-implementation-plan.md).
Phase 3 is underway. Terminal, task, agent, and Empty one-pane content plus the
  palette, context-menu, timeline, session-map, objective-prompt,
  session-output Search, and Help overlays are now covered. Restore handling,
  multi-pane layouts, remaining overlays, and broader scene/input parity remain.
Artifact Preview and production GPU admission remain later, separately gated
decisions.

### `workflows`

Owns developer-workflow definitions and cross-actor handoff policy. Built
today: `TaskRecipe`, `AgentThreadSpec`, and `TaskFailureHandoff`, which shape
durable pane intent for `mandatum-core` and turn bounded, explicitly untrusted
task-failure evidence into an agent mandate. Evidence is JSON-escaped and each
physical line is prefixed inside an unforgeable frame. It launches no runtime.
Not yet built: build/test/dev-server recipe catalogs, task history metadata,
agent result summaries, richer failure classification, command history (see
docs/workflows.md).

Workflow modules request core/runtime actions instead of mutating layout or
process state directly.

## Event Model

Use typed events across the runtime:

- key input
- pointer input
- paste input
- command invocation
- PTY output
- process exit
- task status update
- agent status update
- approval request
- file-change summary
- parser update
- frontend resize
- render tick
- persistence request
- restore result

Events should carry enough identity to reject output from replaced
runtimes.

## Durable State

Persist intent:

- workspaces
- projects
- sessions
- panes
- layout
- focus
- task command intent
- agent objective and thread identity
- user preferences
- keymap/theme names
- last known working directory

Do not persist:

- process handles
- process ids as durable truth
- PTY handles
- parser objects
- thread handles
- frontend window handles
- GPU resources
- live task status
- live agent output streams
- unbounded scrollback

## Failure Model

Every runtime failure should become visible state:

- process spawn failure
- process exit
- parser error
- reader failure
- task failure
- agent blocked
- approval required
- persistence failure
- restore mismatch
- frontend rendering failure

Failures should leave enough information for the user to inspect, rerun,
restart, stop, or recover.
