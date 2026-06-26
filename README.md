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

This repo has completed the first scaffold step and started Milestone 2:

- Cargo workspace with `core`, `commands`, `workflows`, `pty`, `terminal-vt`, `renderer`, and `app` crates.
- Renderer-neutral `core` domain for workspace, project, session, pane, layout, focus, actions, and JSON session persistence.
- Minimal `commands` crate that maps command ids to core actions without owning layout mutation logic.
- Minimal `workflows` crate for durable task/agent pane intent helpers only.
- `terminal-vt` has the first fake parser adapter seam: plain grid, cursor, cell, capability, update, and adapter types with fixture-driven tests.
- `pty` has the first native OS PTY seam: session/process identifiers, spawn/resize/restart intent, raw byte output, input writes, resize, child exit, kill, and bounded byte-buffer backpressure tests.
- `libghostty-vt` has been evaluated as a future optional `terminal-vt` backend; no binding or dependency has been added.
- `renderer` and `app` remain compile-only placeholders. They do not implement runtime behavior yet.

There is still no runnable app shell, visible terminal pane, renderer integration, or `libghostty-vt` binding.

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
```

Between phases, also run the doc hygiene scan in `docs/verification.md` and clear or label outdated status language before writing a handoff.
