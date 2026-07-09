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
`app_shell`, `input`, `persistence`, `process_events`, `terminal_runtime`, and
`task_runtime` modules under `crates/app`.

Live runtime state is never serialized as durable truth.

### Scene Layer

Owns renderer-neutral presentation:

- pane bounds
- tiled, stacked, floating, and zoomed surfaces
- terminal grid surfaces
- task and agent summaries
- command palette view model
- status strips
- overlays
- hit targets
- scrollback viewport
- selection state
- animation intent

The scene layer is the interface between product state and frontend adapters.

### Frontend Adapters

Own rendering and platform input:

- terminal frontend
- native window frontend
- GPU-backed frontend
- platform-specific frontend

Frontend adapters should draw a scene and emit input/hit-test events. They do
not own product behavior.

### `workflows`

Owns developer-workflow definitions:

- task recipes
- build/test/dev-server recipes
- task history metadata
- agent launch intent
- agent result summaries
- failure classification
- command history

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
