# Repo Structure

This is the implemented Milestone 3 shape plus the first Milestone 4
real-terminal runtime slice: core domain, command/workflow intent seams, PTY and
terminal parser seams, placeholder workspace shell, and PTY-backed terminal
grid rendering through the current fake/basic parser.

```text
Mandatum/
  AGENTS.md
  README.md
  docs/
    architecture.md
    codex-goal.md
    ghostty-libghostty-evaluation.md
    interaction-model.md
    libghostty-vt-feasibility-spike.md
    milestones.md
    product-principles.md
    rendering-strategy.md
    repo-structure.md
    verification.md
    workflows.md
  Cargo.toml
  Cargo.lock
  crates/
    core/
    pty/
    terminal-vt/
    renderer/
    app/
    commands/
    workflows/
  .agents/
    skills/
      product-architect/
      interaction-reviewer/
      rendering-spike/
      terminal-conformance/
```

## Current Crate Status

- `crates/core`: implemented pure domain and JSON persistence.
- `crates/commands`: implemented command ids, labels, categories, and core-action dispatch.
- `crates/workflows`: implemented durable task/agent pane intent helpers only.
- `crates/pty`: PTY identifiers, spawn/resize/restart intent, output/exit events, bounded byte-buffer backpressure state, headless native OS PTY session wrapper, and split reader/writer/controller runtime parts.
- `crates/terminal-vt`: fake parser adapter seam with grid/cursor/cell/capability/update types, `TerminalParser` ownership wrapper, and fixture-driven tests; `libghostty-vt` is evaluated but not bound.
- `crates/renderer`: Ratatui renderer for core layout state, pane chrome, focus, zoom, floating panes, status, command palette overlay, and supplied terminal grid snapshots.
- `crates/app`: Crossterm/Ratatui terminal runtime with lifecycle restoration, root binary, event loop, resize handling, PTY-backed shell spawning, PTY output reader threads, parser feeding, key/paste input routing, and command-palette state.

## Workspace Commands

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run
```

## Rules

- Keep `crates/core` renderer-neutral. Do not add PTY, parser, renderer, app-runtime, or terminal UI dependencies to `crates/core`.
- Keep phase language clear: docs may mention historical milestones, but current status sections must say which crates are implemented, placeholders, or deferred.
