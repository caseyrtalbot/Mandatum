# PLAN.md

## Objective

Prepare and execute Mandatum, a greenfield terminal-native workspace for developers, closer to tmux/zellij than an IDE, and buildable entirely from terminal/Codex workflows.

This repo began as a planning scaffold. Milestone 0 decisions are accepted,
Milestone 1 has a Cargo workspace plus renderer-neutral core implementation,
Milestone 2 has the terminal parser, native PTY, and `libghostty-vt`
feasibility seams, Milestone 3 has the runnable terminal shell, and Milestone 4
is complete: real PTY-backed terminal panes with a hardened VT parser,
scrollback, a keyboard copy/selection baseline, and in-place PTY restart.
Milestone 5A is complete: the app can save and restore durable workspace/layout
intent from disk and reconciles restored terminal panes with fresh PTYs. The
first Milestone 5B slices are complete: the app can launch one configured
shell command in a task pane and explicitly rerun or stop the focused task
while keeping process handles, parser state, thread handles, output buffers,
and status runtime outside durable core state.

## Current State

Created:

- `AGENTS.md`
- `README.md`
- product principles
- architecture boundaries
- interaction model
- rendering strategy
- Ghostty/libghostty evaluation plan
- milestones
- verification plan
- workflow descriptions
- decision log
- repo-scoped Codex skills
- Cargo workspace
- `crates/core`
- `crates/commands`
- `crates/workflows`
- first fake parser adapter seam in `crates/terminal-vt`
- first pure PTY abstraction seam in `crates/pty`
- headless native OS PTY spawning, raw input/output, resize, child-exit, and kill wrapper in `crates/pty`
- `libghostty-vt` feasibility spike documented in `docs/libghostty-vt-feasibility-spike.md`
- runnable Crossterm/Ratatui app shell in `crates/app`
- Ratatui workspace and terminal-grid renderer in `crates/renderer`
- split PTY reader/writer/controller runtime parts in `crates/pty`
- `TerminalParser` owner in `crates/terminal-vt`
- PTY-backed shell spawning, output feeding, key/paste input, pane resize, and
  process-exit status in `crates/app`
- hardened default VT parser backend behind `TerminalAdapter`, built on the
  `vte` tokenizer in `crates/terminal-vt`
- bounded, runtime-owned scrollback with a scrollback/selection viewport in the
  renderer
- keyboard copy mode with stream selection and OSC 52 clipboard copy in
  `crates/app`
- in-place PTY restart registry keyed by `PaneId` in `crates/app`
- disk-backed workspace save/restore in `crates/app` at
  `.mandatum/workspace.json`, using existing core JSON persistence and fresh
  PTY reconciliation after restore
- durable task pane command intent in `crates/core`/`crates/workflows`, `Run Task`,
  `Rerun Task`, and `Stop Task` command metadata in `crates/commands`,
  task-pane output/status presentation in `crates/renderer`, and app-owned task
  process runtime launch/rerun/stop semantics in `crates/app`

Not yet created:

- `libghostty-vt` binding (still a deferred optional backend)
- native OS mouse selection, semantic selection, rich clipboard history
- command history, named task recipe configuration, automatic restored-task
  relaunch, and agent workflow pane runtime

The terminal pane now runs a normal interactive shell: common VT output (shell
prompts, command output, line redraws, clears, and ANSI styling) renders without
exposing raw escape sequences.

## Product Summary

Build Mandatum as a terminal-native workspace for coding sessions:

- persistent project workspaces
- terminal panes
- split/stack/floating/zoom layouts
- command palette
- keymap-driven control
- build/test task panes
- agent/thread panes
- terminal visual polish
- renderer-neutral core
- no Xcode or Apple-native GUI dependency

Do not build a full IDE in the first milestones.

## Architecture Summary

Target layers:

```text
core          workspace/session/layout/action domain model
pty           process lifecycle and terminal I/O
terminal-vt   terminal parser adapter boundary
renderer      terminal grid, pane chrome, overlays, and frame pacing
app           terminal runtime and lifecycle
commands      command palette, keymaps, action registry
workflows     builds, tests, tasks, agents, logs
```

The first high-value implementation target was `core`; current work should
preserve that renderer-neutral boundary while evolving the PTY, parser,
renderer, app, commands, and workflows crates.

## Accepted First `/plan`

The first plan accepted these defaults:

- Rust workspace.
- JSON session persistence with a versioned schema field.
- Core-first Milestone 1 implementation.
- Fake parser boundary before evaluating `libghostty-vt`.
- No app runtime yet.
- No Xcode, SwiftUI, AppKit, Metal, MetalKit, CoreText renderer work, or Apple-native GUI stack.

Original planning prompt:

Prompt:

```text
Use this repository's docs as source of truth. Plan Milestone 0 and Milestone 1 for this greenfield terminal-native workspace.

The product should be closer to tmux and zellij than to a general IDE. It should provide persistent project workspaces, terminal panes, split/stack/floating-style layouts inside the terminal, command palette control, build/test task surfaces, and agent/thread orchestration.

Do not inspect or reuse any existing Aetherspace code. Treat this as greenfield.

Do not use Xcode.app, `.xcodeproj`, SwiftUI, AppKit, Metal, MetalKit, CoreText renderer work, or any Apple-native GUI app stack. The implementation must be buildable, testable, and runnable from terminal commands under Codex.

Do not write implementation code yet unless the plan explicitly calls for a minimal scaffold. Identify decisions that are blocked, propose the smallest viable implementation path, and produce an execution plan with verification gates.

Your plan must cover:
1. product principles and non-goals
2. repo structure
3. module boundaries
4. technology choices with tradeoffs
5. milestone plan
6. verification plan
7. open questions that truly block implementation
```

## Milestone 1 Implementation Goal

Milestone 1 implements the accepted core-first goal:

```text
Implement Milestone 1: renderer-neutral core domain for Mandatum.

Create the selected build system and core module structure. Implement workspace, project, pane, layout, focus, command action, and session persistence models. Keep core independent from renderer, terminal app runtime, PTY, and terminal parser types. Add tests for layout, focus, action dispatch, and serialization.

Done when formatting, tests, and build/typecheck pass, and docs reflect the implemented boundaries.
```

## Post-Milestone 5B Task Runtime Deferred Decisions

1. Command history, named task recipe configuration UI, and automatic
   restored-task relaunch policy.
2. `libghostty-vt` binding as a future optional `terminal-vt` backend.
3. Native OS mouse selection, semantic selection, and rich clipboard history.
4. Terminal capability model beyond the current local `vte` backend.
5. Agent workflow pane runtime and broader keymap configuration.

## Suggested Decision Biases

- Choose Rust-first if the goal is terminal/Codex-friendly incremental progress.
- Preserve the pure `core` boundary before adding runtime or workflow features.
- Keep parser implementations behind `TerminalAdapter`; the local `vte` backend
  is the current default, and the fake adapter is fixture-only.
- Avoid a Ghostty fork until the workspace product proves itself.
- Avoid Swift/AppKit/SwiftUI/Metal entirely for this repo.
- Avoid a web UI unless terminal-native output is explicitly deprioritized.

## Verification Gate

Current verification commands:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- boundary check that `core` imports no PTY, terminal parser, renderer, app-runtime, or terminal UI crates
- persistence check that serialized session state contains durable intent only
