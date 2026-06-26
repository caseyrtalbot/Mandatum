# Repo Structure

This is the implemented Milestone 1 shape plus the first Milestone 2
`terminal-vt` fake parser seam and `pty` abstraction/native OS PTY seam.

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
- `crates/pty`: PTY identifiers, spawn/resize/restart intent, output/exit events, bounded byte-buffer backpressure state, and headless native OS PTY session wrapper.
- `crates/terminal-vt`: fake parser adapter seam with grid/cursor/cell/capability/update types and fixture-driven tests; `libghostty-vt` is evaluated but not bound.
- `crates/renderer`: compile-only placeholder.
- `crates/app`: compile-only placeholder, no runnable app shell yet.

## Workspace Commands

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Rules

- Keep `crates/core` renderer-neutral. Do not add PTY, parser, renderer, app-runtime, or terminal UI dependencies to `crates/core`.
- Keep phase language clear: docs may mention historical milestones, but current status sections must say which crates are implemented, placeholders, or deferred.
