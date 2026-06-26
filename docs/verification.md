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

## Milestone 3 Terminal Runtime Prototype Verification

Historical gate: this slice added the first runnable placeholder terminal app
shell before Milestone 4 connected a real PTY-backed pane. Run from the repo
root when checking a Milestone 3-only state:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run
git diff --check
```

For the runtime and renderer, verify:

- `cargo run` enters the alternate screen, renders the Mandatum workspace scene,
  and restores the terminal on quit.
- Temporary controls dispatch existing command ids through `mandatum-commands`:
  `v` split right, `s` split down, `Tab` or `l` focus next, `Shift-Tab` or `h`
  focus previous, `x` close focused pane, `z` zoom focused pane, `n` new
  floating terminal intent, `f` float focused pane, `t` stack focused pane, `r`
  restart focused pane intent, `p` or `Ctrl-P` command palette, and `q` or
  `Ctrl-C` quit.
- The visible scene reflects `mandatum-core` layout state, focused pane state,
  zoom state, floating pane state, and command metadata.
- Resize events update app runtime size state and trigger redraw. If the local
  verification tool cannot physically resize its PTY, record that limitation and
  rely on the runtime resize unit test plus manual smoke coverage for redraw.
- `crates/core` has no terminal UI, PTY, parser, renderer, app runtime,
  platform UI, `crossterm`, or `ratatui` dependencies.
- `crates/pty` still has no parser, renderer, app, core, or terminal UI
  dependencies beyond its existing `portable-pty` boundary.
- At the Milestone 3 gate only, the app remained a placeholder shell and did
  not connect `NativePtySession` output to `mandatum-terminal-vt` or render a
  real terminal grid.

## Milestone 4 Real Terminal Pane Verification

Milestone 4 is complete. Visible terminal panes run real PTY-backed shells whose
output is parsed by the hardened `VteTerminalAdapter` (default backend behind
`TerminalAdapter`), rendered with styling and a scrollback/selection viewport,
and driven through a keyboard copy mode and an in-place PTY restart. Run from the
repo root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -p mandatum-pty
cargo run
git diff --check
```

For the runtime and renderer, verify:

- `cargo run` enters the alternate screen, spawns a shell for a visible terminal
  pane (`TERM=xterm-256color`), renders shell output, and restores the terminal
  on `Ctrl-Q`.
- Normal keys go to the focused PTY-backed shell while the command palette is
  closed.
- `Ctrl-P` opens command palette mode. In palette mode, `v` split right, `s`
  split down, `Tab`/`l` focus next, `Shift-Tab`/`h` focus previous, `x` close,
  `z` zoom, `n` new terminal intent, `f` float, `t` stack, `r` restart, `[`
  enter copy mode, and `Esc` closes the palette.
- Paste events write pasted text to the focused PTY (suppressed in copy mode).
- PTY output is read on a background reader thread and fed into the per-pane
  `TerminalParser`.
- Renderer receives borrowed terminal grid snapshots only; it does not own
  process handles, parser mutation, or product dispatch.
- PTYs are resized from renderer pane content geometry. Hidden panes from zoom
  or stack presentation must not be treated as stale runtime panes.
- Process exit or PTY failures are visible in status.

Parser hardening, scrollback, copy/selection, and restart are covered by tests:

- `crates/terminal-vt`: VT-backend unit tests (SGR color/truecolor, cursor
  addressing, erase display/line, carriage-return redraw, alternate screen,
  bounded scrollback) plus the retained fake-adapter fixtures.
- `crates/renderer`: scrollback viewport + selection highlight rendering.
- `crates/commands`: copy-mode command target routing.
- `crates/app` unit tests: copy-mode navigation/selection/extraction, OSC 52
  base64 payload, and a real-PTY restart that replaces the live runtime for the
  same `PaneId` while leaving core layout intact.
- `crates/app/tests/terminal_smoke.rs`: real `/bin/sh` end-to-end checks that
  SGR color and cursor addressing render without raw escape leakage, `echo`
  round-trips, and `seq 1 200` is captured into bounded scrollback without
  hanging. These stand in for the interactive `cargo run` smoke where an
  automated terminal is unavailable.

Deferred (not part of the Milestone 4 gate):

- `libghostty-vt` backend binding remains unbound.
- Native OS mouse selection, semantic selection, and rich clipboard history.
- Task/agent workflow panes (Milestone 5+).

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

Milestone 3 historical concrete checks:

- `crates/core` still does not import `mandatum-pty`, `mandatum-terminal-vt`,
  `mandatum-renderer`, `mandatum-app`, `crossterm`, `ratatui`, terminal UI
  runtime crates, or platform UI crates.
- At the Milestone 3 gate, `crates/renderer` depended on `mandatum-core` and
  `ratatui`, but not `mandatum-app`, `mandatum-pty`, or
  `mandatum-terminal-vt`.
- `crates/app` may depend on `mandatum-core`, `mandatum-commands`,
  `mandatum-renderer`, `crossterm`, and `ratatui`, but it must not dispatch
  layout mutations except through `mandatum-commands`.
- `crates/pty` remains independent of core, parser, renderer, app, and terminal
  UI dependencies.

Milestone 4 concrete checks:

- `crates/core` still does not import `mandatum-pty`, `mandatum-terminal-vt`,
  `mandatum-renderer`, `mandatum-app`, `crossterm`, `ratatui`, terminal UI
  runtime crates, or platform UI crates.
- `crates/pty` still does not import `mandatum-terminal-vt`,
  `mandatum-renderer`, `mandatum-app`, `mandatum-core`, `crossterm`,
  `ratatui`, parser-specific crates, or terminal UI runtime crates.
- `crates/terminal-vt` still has no `mandatum-pty`, `mandatum-renderer`,
  `mandatum-app`, `mandatum-core`, `portable-pty`, `crossterm`, `ratatui`,
  Ghostty, Zig, CMake, bindgen, or FFI dependency.
- `crates/renderer` may depend on `mandatum-core`, `mandatum-terminal-vt`, and
  `ratatui`, but not `mandatum-app`, `mandatum-pty`, `crossterm`, or
  process/runtime handles.
- `crates/app` may depend on `mandatum-core`, `mandatum-commands`,
  `mandatum-renderer`, `mandatum-terminal-vt`, `mandatum-pty`, `crossterm`, and
  `ratatui`; live PTY handles and parser mutation stay in app.
