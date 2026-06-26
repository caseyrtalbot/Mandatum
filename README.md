# Mandatum

Mandatum is a greenfield terminal workspace for developers: closer to tmux and zellij than to a general IDE, built entirely from terminal/Codex workflows.

The product thesis is simple: developers live in terminals, panes, sessions, builds, tests, logs, and agent threads. This project should make that environment fast, aesthetic, recoverable, programmable, and deeply usable without trying to replace an editor or requiring an Apple-native GUI stack.

## North Star

Build Mandatum as a terminal-native workspace for coding sessions:

- fast terminal panes
- persistent project workspaces
- split, stack, floating, tabbed, and zoomed layouts
- command palette and keymap-driven control
- build/test/task surfaces
- agent/thread orchestration
- terminal visual polish
- renderer-neutral core architecture
- terminal/Codex build discipline

## Non-Goal

This is not an IDE clone. Early milestones should not include a built-in source editor, language-server platform, debugger platform, extensions marketplace, or large file explorer.

Editing can happen in Neovim, Helix, Zed, Cursor, VS Code, or another editor. This product coordinates the coding environment.

This is also not an Xcode or Apple-native app project. Do not use Xcode.app, `.xcodeproj`, SwiftUI, AppKit, Metal, or other Apple GUI frameworks as the implementation path.

## Current Status

This repo has completed Milestone 3 and the first Milestone 4 real-terminal
runtime slice:

- Cargo workspace with `core`, `commands`, `workflows`, `pty`, `terminal-vt`, `renderer`, and `app` crates.
- Renderer-neutral `core` domain for workspace, project, session, pane, layout, focus, actions, and JSON session persistence.
- Minimal `commands` crate that maps command ids to core actions without owning layout mutation logic.
- Minimal `workflows` crate for durable task/agent pane intent helpers only.
- `terminal-vt` has the first fake parser adapter seam plus a `TerminalParser` owner that the app can keep one-per-pane and feed from PTY byte streams.
- `pty` has the native OS PTY seam plus split reader/writer/controller parts so the app can read output on a background thread while writing input and resizing from the event loop.
- `libghostty-vt` has been evaluated as a future optional `terminal-vt` backend; no binding or dependency has been added.
- `renderer` renders workspace layout state, pane chrome, focus, zoom, floating panes, status, command-palette overlay, and supplied terminal grid snapshots.
- `app` launches from root `cargo run`, enters/restores the terminal, spawns PTY-backed shells for visible terminal panes, feeds PTY output into `terminal-vt`, sends normal key input and paste text back to the focused PTY, resizes PTYs from pane geometry, and dispatches workspace commands through `mandatum-commands`.

Remaining limitations:

- The terminal grid is still backed by the fake/basic parser, so shell escape
  sequences can render visibly until a real VT parser backend is added.
- Copy/selection, scrollback, restart registry behavior, task/agent workflow
  panes, and `libghostty-vt` binding remain deferred.

Current runtime controls:

- Normal keys go to the focused PTY-backed shell.
- `Ctrl-Q`: quit Mandatum and restore the terminal.
- `Ctrl-P`: open/close command palette mode.
- In command palette mode: `v` split right, `s` split down, `Tab`/`l` focus
  next, `Shift-Tab`/`h` focus previous, `x` close focused pane, `z` zoom
  focused pane, `n` new floating terminal intent, `f` float focused pane, `t`
  stack focused pane, `r` restart focused pane intent, `Esc` close palette.

Start with:

- `AGENTS.md`
- `docs/product-principles.md`
- `docs/architecture.md`
- `docs/interaction-model.md`
- `docs/rendering-strategy.md`
- `docs/ghostty-libghostty-evaluation.md`
- `docs/technology-direction.md`
- `docs/milestones.md`
- `docs/codex-goal.md`

## Historical Codex Planning Prompt

This was the bootstrapping `/plan` prompt used before Milestone 1 implementation began:

```text
Use the repository docs as source of truth. Plan Milestone 0 and Milestone 1 for this greenfield terminal-native workspace. Do not write code yet unless the plan explicitly calls for a scaffold. Identify decisions that are blocked, propose the smallest viable implementation path, and produce an execution plan with verification gates.
```

The accepted plan has been promoted into implementation; keep this prompt only as planning provenance.

## Verification

Current code verification:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run
```

Between phases, also run the doc hygiene scan in `docs/verification.md` and clear or label outdated status language before writing a handoff.
