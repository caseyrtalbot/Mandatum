# Verification

## Standard Commands

Run from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
git diff --check
```

Run `cargo run` when verifying the current terminal frontend manually.

## Documentation Verification

After documentation changes, search the active docs for:

- references to files that no longer exist
- prompt text or background-story sections
- frontend bans that contradict `docs/frontend-platform.md`
- implementation status claims that disagree with `docs/repo-structure.md`
- read-order lists that omit a source-of-truth document

Any match must be intentionally current. Missing-doc references and
contradictory frontend constraints should not remain in active documentation.

## Architecture Boundary Checks

Verify:

- `core` has no PTY, parser, renderer, frontend, platform UI, or runtime-handle
  dependencies
- durable workspace JSON excludes process handles, runtime tokens, parser state,
  thread handles, render resources, and scrollback
- frontend drawing code does not dispatch product mutations directly
- runtime modules own live process and agent state
- parser backends stay behind the terminal engine interface

Useful scans:

```sh
rg -n "mandatum-pty|mandatum-terminal-vt|mandatum-renderer|mandatum-app|crossterm|ratatui|portable-pty|winit|wgpu|swift|appkit|metal" crates/core Cargo.toml crates/core/Cargo.toml
rg -n "process_id|runtime_token|reader_thread|JoinHandle|NativePty|parser|scrollback" crates/core
```

## Runtime Checks

For terminal and task runtime work, prove:

- a shell launches in a pane
- typed input reaches the focused pane
- output reaches terminal parser state
- resize updates PTY and parser size
- process exit becomes visible status
- pane restart replaces the runtime for the same pane identity
- task launch/rerun/stop works
- events from replaced runtimes are ignored
- restore preserves durable intent without live handles

## Terminal Engine Checks

Cover:

- plain text
- invalid UTF-8 bytes
- CR/LF behavior
- wrapping
- SGR styles
- true color
- cursor addressing
- erase display and line
- alternate screen
- scroll regions
- bounded scrollback
- output stress

## Scene And Frontend Checks

For scene/frontend work, prove:

- scene renders terminal panes
- scene renders task panes
- scene renders agent panes
- command palette renders from scene data
- hit targets match pane bounds
- resize preserves layout semantics
- drawing code does not own product behavior
- terminal frontend and any native frontend consume the same scene contract

## Agent Runtime Checks

For agent work, prove:

- agent pane can be created from durable intent
- running, blocked, failed, complete, stopped, and waiting states render
- pending approvals become global attention items
- changed-file summaries are visible
- verification results attach to the agent actor
- restore keeps agent intent without inventing live runtime state

## Completion Rule

Do not claim a task is complete until:

- relevant files are updated
- source-of-truth docs agree
- required commands pass or are explicitly scoped out
- remaining risks are named
