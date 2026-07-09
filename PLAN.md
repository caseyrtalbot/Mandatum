# Mandatum Product Plan

## Objective

Create the most functional, dynamic, and beautiful development workstation for
terminal-centered builders: a modular workspace that gives full visibility into
shells, editors, tasks, servers, agents, diffs, approvals, failures, and
recovery.

Mandatum should feel like a terminal environment expanded into a complete
session operating system.

## Design Premise

The product is not defined by one frontend. It is defined by a workstation
engine, a terminal engine, a scene model, and frontend adapters.

The current Rust workspace provides the initial substrate:

- durable workspace/session/pane/layout/action state
- command metadata and routing
- PTY-backed process runtime
- terminal parser and grid snapshots
- task pane launch/rerun/stop runtime
- JSON workspace persistence
- terminal frontend adapter

Future work should deepen these pieces into a product architecture that can
support both a terminal frontend and a high-polish native or GPU-backed
frontend.

## User Experience Target

The user opens Mandatum and sees the real work surface immediately:

- primary terminal/editor pane
- supporting shell/log panes
- build/test/dev-server task pane
- agent pane showing objective, state, approvals, changed files, and checks
- status strip showing project health
- command palette for every meaningful action
- session map and execution history available without visual clutter

The workspace should stay readable during high output, failed tests, multiple
agents, build logs, server restarts, and long-running shell sessions.

## Architecture Target

```text
workspace engine
  durable state, layout, pane identity, commands, persistence intent

runtime engine
  live PTYs, tasks, agents, reader events, process lifecycle, recovery

terminal engine
  parser adapters, grid snapshots, scrollback, terminal capabilities

scene layer
  renderer-neutral panes, terminal surfaces, overlays, hit targets, selections

frontend adapters
  terminal frontend, native/GPU frontend, platform-specific frontend options

workflow layer
  task recipes, build/test/dev-server surfaces, agent orchestration
```

## Workstreams

### 1. Documentation Source Of Truth

Create a coherent spec series that future agents can read without contradictory
constraints.

Deliverables:

- product principles
- architecture spec
- frontend platform strategy
- rendering strategy
- terminal engine spec
- agent runtime spec
- interaction model
- workflow spec
- roadmap
- verification plan
- current repo structure
- current decision log

### 2. Runtime Decomposition

Move live runtime responsibilities behind smaller modules.

Status: initial app runtime decomposition is implemented in `crates/app`.

Targets:

- terminal pane runtime registry
- task runtime registry
- process event router
- persistence coordinator
- input router
- app shell orchestrator

### 3. Scene Contract

Define a renderer-neutral scene interface.

The scene must include:

- pane tree and floating surfaces
- terminal grid surfaces
- task and agent status surfaces
- command palette model
- hit targets
- selection state
- scrollback viewport state
- animation intent
- diagnostic overlays

### 4. Frontend Platform Spike

Evaluate product frontend options against measurable criteria:

- startup time
- input latency
- frame pacing
- text shaping and glyph quality
- scrollback smoothness
- resize smoothness
- pointer precision
- accessibility hooks
- platform integration
- build/test automation

Candidate adapters:

- current terminal frontend for continuity and remote operation
- Rust native/GPU frontend using `winit`, `wgpu`, and text-rendering libraries
- macOS-native frontend when platform fit is decisive

### 5. Workstation Slice

Build one end-to-end session with:

- multiple live terminal panes
- one task recipe and task history
- one agent status pane
- command palette search
- pane focus/split/stack/float/zoom
- save/restore
- failure visibility
- copy/search/scrollback baseline

### 6. Brilliance Pass

Raise the experience from functional to exceptional:

- buttery pane resize and scroll
- crisp text rendering
- precise selection
- semantic output search
- timeline of commands, tasks, and agent actions
- visible approval queue
- visual state that stays calm under load
- recovery that explains what was restored and what needs action

## Immediate Implementation Priority

The next implementation target is the scene contract. Runtime decomposition now
gives the terminal frontend clearer app-shell, input, persistence, process
event, terminal-runtime, and task-runtime boundaries to build from.
