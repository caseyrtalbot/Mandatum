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

## Milestone 2 Parser Seam Verification

The first Milestone 2 slice adds only the fake terminal parser seam in
`crates/terminal-vt`. Run from the repo root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo run` is still intentionally out of scope because Milestone 2 has not
created a runnable app shell.

For the fake parser seam, verify:

- `mandatum-terminal-vt` exposes plain adapter/grid/cursor/cell/capability types.
- fixture-driven tests cover row population, carriage return overwrite,
  scrolling, wrapping, and controls.
- integration/unit tests cover invalid UTF-8, resize behavior, and split UTF-8
  input.
- `crates/core` still has no PTY, parser, renderer, app runtime, or terminal UI
  imports/dependencies.

## Milestone 2 PTY Abstraction Verification

The next Milestone 2 slice adds only the pure PTY abstraction seam in
`crates/pty`. Run from the repo root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For the PTY abstraction seam, verify:

- `mandatum-pty` exposes session/process identifiers, spawn intent, resize intent,
  restart intent, byte-stream output events, child-exit representation, and
  bounded byte-buffer/backpressure state.
- tests cover output events, resize intent, child exit, restart intent, and
  bounded buffer/backpressure behavior.
- `crates/pty` does not depend on `mandatum-terminal-vt`, `mandatum-renderer`, `mandatum-app`,
  `mandatum-core`, parser-specific types, or terminal UI runtime crates.
- This section describes the pure abstraction slice. The native OS PTY slice is
  covered by the next verification section.

## Milestone 2 Native PTY Verification

This slice adds headless OS PTY spawning in `crates/pty` without creating a
runnable app shell or visible terminal pane. Run from the repo root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -p mandatum-pty
```

For the native PTY seam, verify:

- `mandatum-pty` exposes an opaque `NativePtySession` runtime handle.
- native tests cover spawn success, spawn failure, raw byte output including
  invalid UTF-8, input writes, resize propagation, child exit, kill, closed
  input handling, and `PtyEvent` wrappers.
- `crates/pty` may depend on `portable-pty`, but it must not depend on
  `mandatum-terminal-vt`, `mandatum-renderer`, `mandatum-app`, `mandatum-core`, parser-specific
  types, or terminal UI runtime crates.
- `crates/core` still has no PTY, parser, renderer, app runtime, terminal UI,
  platform UI, or `portable-pty` dependency.
- Runnable app orchestration, parser feeding, renderer integration, visible
  terminal panes, and restart registries remain later work.

## Milestone 2 libghostty-vt Feasibility Verification

This slice evaluates `libghostty-vt` without adding a compiled binding. Run from
the repo root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Also verify:

- `docs/libghostty-vt-feasibility-spike.md` records upstream source evidence,
  local toolchain availability, adapter mapping, risks, and the next binding
  gate.
- `crates/terminal-vt/Cargo.toml` still has no Ghostty, Zig, CMake, bindgen,
  or FFI dependency.
- `crates/core` and `crates/pty` still have no Ghostty, parser, renderer, app,
  terminal UI, or platform UI dependencies.
- docs state that `libghostty-vt` is feasible as a future optional backend, but
  real binding remains deferred.

## Minimum Quality Gate

Before a milestone is marked complete:

1. relevant commands pass
2. failures are documented
3. docs match implementation
4. architecture boundaries are checked
5. remaining work is explicit

## Phase Hygiene Check

Run this before starting a new phase and before writing a handoff:

```sh
git status --short
rg -n "placeholder terminal parser|deferred to Milestone 2|fake parser.*deferred|terminal-vt -> renderer|not yet created:.*terminal parser|deprecated|stale|TODO|FIXME" AGENTS.md README.md PLAN.md docs crates -g '*.md' -g '*.rs' -g '!docs/verification.md'
```

For each match, either:

- update the stale statement to current state
- label it as historical planning provenance
- keep it only when the surrounding section clearly says the work remains deferred

Handoffs must name the current crate status, stale/deprecated doc findings, verification commands, boundary checks, and the next exact action.

## Boundary Checks

For early architecture work, verify:

- `core` imports no UI/app/renderer/PTY/parser modules
- durable session structs do not include runtime handles
- renderer does not dispatch product-specific mutations directly
- workflows call core actions rather than mutating layout state
- terminal parser adapter is replaceable

Milestone 1 concrete checks:

- `crates/core` does not import `mandatum-pty`, `mandatum-terminal-vt`, `mandatum-renderer`, `mandatum-app`, `crossterm`, `ratatui`, or terminal UI runtime crates.
- Session JSON includes durable workspace, project, pane, layout, focus, task, and agent intent.
- Session JSON excludes PTY handles, parser objects, renderer resources, process ids, thread handles, and unbounded scrollback.
