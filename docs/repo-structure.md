# Repo Structure

This is the implemented Milestone 5B task-runtime shape: core domain, command/workflow
intent seams, the PTY seam, a hardened VT parser behind `TerminalAdapter`, a
workspace shell, PTY-backed terminal panes with styled grid plus scrollback
rendering, keyboard copy mode, in-place PTY restart, and disk-backed durable
workspace layout persistence, plus app-owned configured task launch, rerun, and
stop runtime semantics.

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

- `crates/core`: implemented pure domain and JSON persistence, including durable task pane command intent without live status or process handles.
- `crates/commands`: implemented command ids, labels, categories, core-action dispatch, and runtime command metadata for copy mode plus configured task run/rerun/stop commands.
- `crates/workflows`: implemented durable task/agent pane intent helpers only; it still does not launch processes.
- `crates/pty`: PTY identifiers, spawn/resize/restart intent, output/exit events, bounded byte-buffer backpressure state, headless native OS PTY session wrapper, and split reader/writer/controller runtime parts.
- `crates/terminal-vt`: grid/cursor/cell/style/capability/update types with a bounded scrollback grid (`grid.rs`), a hardened default parser backend on the `vte` tokenizer (`vte_backend.rs`), the retained `FakeTerminalAdapter` for fixtures (`fake.rs`), and the `TerminalParser` ownership wrapper. `libghostty-vt` is evaluated but not bound; the only external dependency is the pure-Rust `vte` crate.
- `crates/renderer`: Ratatui renderer for core layout state, pane chrome, focus, zoom, floating panes, status, command palette overlay, styled terminal grid snapshots with a scrollback/selection-aware `TerminalViewport`, and read-only task runtime status/output views.
- `crates/app`: Crossterm/Ratatui terminal runtime with lifecycle restoration, root binary, event loop, resize handling, disk-backed workspace save/restore at `.mandatum/workspace.json`, PTY-backed shell spawning, PTY output reader threads, parser feeding, key/paste input routing, command-palette state, a keyboard copy mode (`copy_mode.rs`), OSC 52 clipboard output (`clipboard.rs`), an in-place PTY restart registry, and app-owned configured task process launch/rerun/stop runtime.

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
