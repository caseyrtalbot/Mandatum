# Repo Structure

This is the implemented Milestone 4 shape: core domain, command/workflow intent
seams, the PTY seam, a hardened VT parser behind `TerminalAdapter`, a workspace
shell, and PTY-backed terminal panes with styled grid plus scrollback rendering,
keyboard copy mode, and in-place PTY restart.

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
- `crates/terminal-vt`: grid/cursor/cell/style/capability/update types with a bounded scrollback grid (`grid.rs`), a hardened default parser backend on the `vte` tokenizer (`vte_backend.rs`), the retained `FakeTerminalAdapter` for fixtures (`fake.rs`), and the `TerminalParser` ownership wrapper. `libghostty-vt` is evaluated but not bound; the only external dependency is the pure-Rust `vte` crate.
- `crates/renderer`: Ratatui renderer for core layout state, pane chrome, focus, zoom, floating panes, status, command palette overlay, and styled terminal grid snapshots with a scrollback/selection-aware `TerminalViewport`.
- `crates/app`: Crossterm/Ratatui terminal runtime with lifecycle restoration, root binary, event loop, resize handling, PTY-backed shell spawning, PTY output reader threads, parser feeding, key/paste input routing, command-palette state, a keyboard copy mode (`copy_mode.rs`), OSC 52 clipboard output (`clipboard.rs`), and an in-place PTY restart registry.

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
