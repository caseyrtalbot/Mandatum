# Roadmap

## Gate 1: Documentation Source Of Truth

Outcome: future agents can read the docs and build toward the workstation
vision without contradictory instructions.

Deliverables:

- current product principles
- current architecture
- frontend platform strategy
- terminal engine strategy
- rendering strategy
- agent runtime spec
- interaction/workflow specs
- verification plan
- repo structure
- decision log

Validation:

- no references to missing docs
- active docs describe only the current target state
- no contradictory frontend constraints
- doc trace scan passes

## Gate 2: Runtime Decomposition

Status: implemented as the initial `crates/app` runtime module split.

Outcome: live runtime responsibilities are isolated behind clear modules.

Deliverables:

- terminal runtime registry
- task runtime registry
- process event router
- persistence coordinator
- input router
- app shell orchestrator

Validation:

- existing behavior preserved
- tests still cover task launch/rerun/stop, restore, replaced-runtime event rejection, and
  terminal pane restart
- `core` remains free of runtime/frontend types

## Gate 3: Scene Contract

Outcome: frontends consume a renderer-neutral scene.

Deliverables:

- scene types
- pane bounds and hit targets
- terminal surface view
- task surface view
- agent surface view
- command palette view
- status strip view
- scene tests

Validation:

- current terminal frontend renders through scene types
- no product action dispatch inside frontend drawing code
- scene supports terminal, task, and agent surfaces

## Gate 4: Frontend Platform Spike

Outcome: decide whether a native/GPU frontend materially improves the product.

Deliverables:

- one live PTY rendered through candidate frontend
- text, cursor, selection, scrollback, resize, paste
- task/agent status strip
- latency and frame pacing notes
- smoke verification path

Validation:

- measurable quality gain or clear rejection
- product logic remains in shared engine
- build and run instructions are explicit

## Gate 5: Workstation Visibility Slice

Outcome: the product can supervise a real development session.

Deliverables:

- multiple terminal panes
- task recipe with history
- dev-server recipe
- agent status pane
- session map
- execution timeline
- global attention strip

Validation:

- user can identify running, failed, blocked, and waiting work at a glance
- user can jump to every attention item
- restore preserves useful intent

## Gate 6: Brilliance Pass

Outcome: the experience feels exceptional under real load.

Deliverables:

- smooth resize and scroll
- crisp text and color
- precise pointer selection
- semantic output search
- polished command palette
- calm failure states
- accessible keyboard and native frontend hooks

Validation:

- stress output remains responsive
- UI remains readable with many actors
- user can recover from failed panes, failed tasks, blocked agents, and restore
  errors without ambiguity
