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

This repo has completed Milestone 3, Milestone 4 (real terminal pane),
Milestone 5A (workspace open/restore and layout persistence), and the first
Milestone 5B task-runtime slice:

- Cargo workspace with `core`, `commands`, `workflows`, `pty`, `terminal-vt`, `renderer`, and `app` crates.
- Renderer-neutral `core` domain for workspace, project, session, pane, layout, focus, actions, and JSON session persistence.
- Minimal `commands` crate that maps command ids to core actions (and routes app-runtime commands such as copy mode and run-task), without owning layout mutation logic.
- Minimal `workflows` crate for durable task/agent pane intent helpers only.
- `terminal-vt` provides a hardened default VT parser behind `TerminalAdapter`, built on the pure-Rust `vte` tokenizer, with SGR styling, cursor addressing, erase/insert/delete, scroll region, alternate screen, and bounded scrollback. The original fake adapter is retained for fixtures.
- `pty` has the native OS PTY seam plus split reader/writer/controller parts so the app can read output on a background thread while writing input and resizing from the event loop.
- `libghostty-vt` has been evaluated as a future optional `terminal-vt` backend; no binding or dependency has been added.
- `renderer` renders workspace layout state, pane chrome, focus, zoom, floating panes, status, command-palette overlay, styled terminal grid snapshots with a scrollback/selection-aware viewport, and task-pane runtime status/output views.
- `app` launches from root `cargo run`, enters/restores the terminal, restores
  durable workspace JSON from `.mandatum/workspace.json` when present, spawns
  PTY-backed shells for visible terminal panes (`TERM=xterm-256color`), feeds
  PTY output into the hardened parser, sends normal key input and paste text
  back to the focused PTY, resizes PTYs from pane geometry, offers a keyboard
  copy/scrollback mode, restarts a pane's PTY in place, saves/restores durable
  layout intent on command, launches one configured shell task in an app-owned
  task pane via `Ctrl-P` then `b`, supports focused task rerun/stop through
  app-owned runtime semantics, and dispatches workspace commands through
  `mandatum-commands`.

Remaining limitations:

- `libghostty-vt` binding remains deferred; the local `vte`-based backend is the default.
- Copy mode is a keyboard-first baseline (stream selection + OSC 52 clipboard). Native OS mouse selection, semantic selection, and rich clipboard history are out of scope.
- Command history, named task recipe configuration, automatic restored-task
  relaunch, and agent workflow panes remain Milestone 5B+/6 work.

Current runtime controls:

- Normal keys go to the focused PTY-backed shell.
- `Ctrl-Q`: quit Mandatum and restore the terminal.
- `Ctrl-P`: open/close command palette mode.
- In command palette mode: `v` split right, `s` split down, `Tab`/`l` focus
  next, `Shift-Tab`/`h` focus previous, `x` close focused pane, `z` zoom
  focused pane, `n` new floating terminal intent, `f` float focused pane, `t`
  stack focused pane, `r` restart focused terminal pane or rerun focused task
  pane, `c` stop focused task pane, `b` run the configured task, `w` save
  workspace, `o` restore workspace, `[` enter copy mode, `Esc` close palette.
- In copy mode (scrollback + selection): `h`/`j`/`k`/`l` or arrows move, `PageUp`/`PageDown`
  scroll a page, `g`/`G` jump to top/bottom, `0`/`$` line start/end, `v` or
  `Space` start selection, `c` clear selection, `y`/`Enter` copy the selection
  (via OSC 52) and exit, `q`/`Esc` exit. The host terminal must support OSC 52
  for the copy to reach the system clipboard.

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
