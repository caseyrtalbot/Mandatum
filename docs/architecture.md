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

Owns live state:

- terminal pane runtime registry
- task runtime registry
- agent runtime registry
- process event routing
- reader-thread lifecycle
- runtime tokens and replaced-runtime event rejection
- live status strings
- launch failures
- stop/rerun/restart behavior
- restore reconciliation

The current app implementation isolates these responsibilities in
`app_shell`, `events`, `frontend`, `input`, `persistence`, `process_events`,
`terminal_runtime`, `task_runtime`, and `agent_runtime` modules under
`crates/app` (full module map: docs/repo-structure.md). The run loop is
event-driven: one unified channel (`AppEvent::Input | Pty | Agent`), a
250 ms heartbeat, and an 8 ms redraw cap; PTY readers are bounded by
flow-credit backpressure (256 KiB in flight per pane).

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
- GPU-backed frontend (proven as the `spikes/frontend-wgpu` adapter, held
  warm; see docs/frontend-platform.md)
- native window / platform-specific frontends (options, not built)

Frontend adapters should draw a scene and emit input/hit-test events. They do
not own product behavior.

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
