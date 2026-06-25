# Repo Structure

This is the implemented Milestone 1 shape.

```text
native-terminal-workspace/
  AGENTS.md
  README.md
  docs/
    architecture.md
    codex-goal.md
    ghostty-libghostty-evaluation.md
    interaction-model.md
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

## Milestone 1 Crate Status

- `crates/core`: implemented pure domain and JSON persistence.
- `crates/commands`: implemented command ids, labels, categories, and core-action dispatch.
- `crates/workflows`: implemented durable task/agent pane intent helpers only.
- `crates/pty`: compile-only placeholder.
- `crates/terminal-vt`: compile-only placeholder.
- `crates/renderer`: compile-only placeholder.
- `crates/app`: compile-only placeholder, no runnable app shell yet.

## Workspace Commands

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Rule

Keep Milestone 1 core renderer-neutral. Do not add PTY, parser, renderer, app-runtime, or terminal UI dependencies to `crates/core`.
