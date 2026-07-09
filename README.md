# Mandatum

Mandatum is a development workstation for builders who live in shells, editors,
builds, tests, logs, agents, diffs, and long-running project sessions.

The product goal is a brilliant modular terminal experience with IDE-grade
visibility: every pane, process, task, agent, approval, failure, and recovery
path should be visible without turning the workspace into a chat app,
dashboard, or conventional editor clone.

## Product Direction

Mandatum is built around a reusable workstation engine and one or more product
frontends.

- The engine owns durable workspace intent, process runtime, terminal state,
  task execution, agent state, persistence, and command routing.
- The scene layer describes panes, terminal grids, overlays, hit targets,
  selections, status surfaces, and animation intent without committing to a
  specific renderer.
- Frontends can be terminal, native, GPU-backed, or platform-specific as long as
  product behavior stays behind stable engine and scene interfaces.

The current codebase already contains useful substrate: workspace state, pane
layout, command routing, PTY process handling, terminal parsing, task-pane
runtime, persistence, and a terminal frontend adapter.

## North Star

The first-class experience should make these visible at a glance:

- which project and session are active
- which terminals, tasks, servers, and agents are running
- what failed and what command produced it
- which files changed
- what needs approval
- what can be restarted, rerun, stopped, copied, searched, or restored
- how to resume a session without rebuilding the mental map

## Current Repository Shape

```text
crates/core         durable workspace, sessions, panes, layouts, actions
crates/commands     command metadata, palette routing, core/runtime targets
crates/pty          PTY process lifecycle, I/O, resize, child exit
crates/terminal-vt  terminal parser adapter and grid/snapshot model
crates/renderer     current terminal frontend rendering adapter
crates/app          terminal app shell plus runtime coordination modules
crates/workflows    task and agent intent helpers
```

## Spec Set

Read in this order:

1. `AGENTS.md`
2. `PLAN.md`
3. `docs/product-principles.md`
4. `docs/architecture.md`
5. `docs/frontend-platform.md`
6. `docs/rendering-strategy.md`
7. `docs/terminal-engine.md`
8. `docs/agent-runtime.md`
9. `docs/interaction-model.md`
10. `docs/workflows.md`
11. `docs/roadmap.md`
12. `docs/verification.md`
13. `docs/repo-structure.md`
14. `docs/decisions.md`

## Development Commands

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run
git diff --check
```

`cargo run` launches the current terminal frontend. It is a useful runtime and
verification surface, not the only valid product frontend.
