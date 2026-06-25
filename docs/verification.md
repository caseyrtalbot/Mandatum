# Verification Plan

## Current Scaffold Verification

Until code exists, verify:

- expected docs exist
- repo has Git initialized
- `AGENTS.md` contains architecture and verification rules
- docs consistently state greenfield and non-IDE constraints

## Future Code Verification

Every implementation milestone should define:

- formatting command
- unit test command
- build/typecheck command
- integration/smoke command if applicable
- architecture boundary check

## Milestone 1 Code Verification

Run from the repo root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo run` is intentionally not part of Milestone 1 verification because `crates/app` is a compile-only boundary and no runnable app shell exists yet.

## Minimum Quality Gate

Before a milestone is marked complete:

1. relevant commands pass
2. failures are documented
3. docs match implementation
4. architecture boundaries are checked
5. remaining work is explicit

## Boundary Checks

For early architecture work, verify:

- `core` imports no UI/app/renderer/PTY/parser modules
- durable session structs do not include runtime handles
- renderer does not dispatch product-specific mutations directly
- workflows call core actions rather than mutating layout state
- terminal parser adapter is replaceable

Milestone 1 concrete checks:

- `crates/core` does not import `ntw-pty`, `ntw-terminal-vt`, `ntw-renderer`, `ntw-app`, `crossterm`, `ratatui`, or terminal UI runtime crates.
- Session JSON includes durable workspace, project, pane, layout, focus, task, and agent intent.
- Session JSON excludes PTY handles, parser objects, renderer resources, process ids, thread handles, and unbounded scrollback.
