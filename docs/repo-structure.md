# Repository Structure

## Root

```text
README.md       product entrypoint
AGENTS.md      agent operating contract
PLAN.md        active product plan
Cargo.toml     Rust workspace manifest
Cargo.lock     locked Rust dependencies
docs/          product and architecture specs
crates/        implementation modules
.agents/       repo-local agent skills
```

## Docs

```text
docs/product-principles.md  product thesis and quality bar
docs/architecture.md        engine, runtime, scene, and frontend responsibilities
docs/frontend-platform.md   terminal/native/GPU frontend strategy
docs/rendering-strategy.md  scene and visual performance strategy
docs/terminal-engine.md     terminal parser/grid/backend strategy
docs/agent-runtime.md       agent actor model and runtime surface
docs/interaction-model.md   commands, panes, session map, timeline, input
docs/workflows.md           end-to-end developer workflows
docs/roadmap.md             active execution gates
docs/verification.md        proof commands, scans, and quality gates
docs/repo-structure.md      current file layout
docs/decisions.md           active decisions
```

## Crates

### `crates/core`

Durable workstation model:

- workspaces
- projects
- sessions
- panes
- layouts
- focus
- actions
- persistence

### `crates/commands`

Command vocabulary and routing:

- command ids
- labels
- categories
- palette key resolution
- core/runtime command targets

### `crates/pty`

PTY process mechanics:

- spawn intent
- native PTY session
- reader/writer/controller split
- resize
- input/output
- child exit
- termination

### `crates/terminal-vt`

Terminal engine:

- parser adapter
- terminal grid
- cursor
- cell styles
- scrollback
- terminal snapshots

### `crates/renderer`

Current terminal frontend rendering adapter.

This crate should move toward consuming a renderer-neutral scene contract rather
than owning workstation behavior.

### `crates/app`

Current terminal runtime shell:

- `app_shell.rs`: terminal lifecycle, event loop, renderer handoff
- `app_state.rs`: command dispatch and runtime reconciliation orchestration
- `terminal_runtime.rs`: terminal pane runtime registry
- `task_runtime.rs`: task runtime registry and pending task launch state
- `process_events.rs`: reader-thread process event routing
- `persistence.rs`: workspace file persistence coordinator
- `input.rs`: keyboard and palette input routing
- `copy_mode.rs`: terminal copy/selection state
- `clipboard.rs`: OSC 52 clipboard payloads

### `crates/workflows`

Workflow intent:

- task recipes
- agent thread specs
- future task history
- future agent launch metadata

## Repo-Local Skills

```text
.agents/skills/product-architect/
.agents/skills/interaction-reviewer/
.agents/skills/rendering-spike/
.agents/skills/terminal-conformance/
```

These skills should point to the current spec set and avoid hidden product
constraints.
