# Decision Log

Use this file for durable architectural decisions once `/plan` begins.

Each entry should use this shape:

```text
## YYYY-MM-DD: Decision Title

Status: Proposed | Accepted | Rejected | Superseded

Decision:

Context:

Options Considered:

Rationale:

Consequences:

Verification:
```

## 2026-06-25: Greenfield Product Boundary

Status: Accepted

Decision:

Mandatum is a greenfield terminal-native workspace. It should not reuse an existing Aetherspace code path or become an IDE-first product.

Context:

The product is meant to transfer the idea of a developer command workspace onto a native, high-quality terminal layer, closer to tmux/zellij/Ghostty than VS Code.

Options Considered:

- Continue from an existing TUI implementation.
- Fork Ghostty and build product features inside it.
- Start greenfield with terminal substrate evaluation behind an adapter.

Rationale:

The durable product idea is the workspace model, not the prior runtime implementation. Forking a terminal emulator too early would shift effort toward terminal maintenance instead of developer-workspace design.

Consequences:

- Early work is docs and architecture first.
- Core state must stay renderer-neutral.
- Terminal parser choice is deferred behind `terminal-vt`.
- No existing Aetherspace code should be copied into the repo.

Verification:

- `AGENTS.md` states the greenfield rule.
- Architecture docs define separate core, PTY, terminal-vt, renderer, app, commands, and workflows layers.

## 2026-06-25: Terminal/Codex Build Constraint

Status: Accepted

Decision:

This repo must be buildable, testable, and runnable from terminal commands under Codex. Xcode.app, `.xcodeproj`, SwiftUI, AppKit, Metal, MetalKit, CoreText renderer work, and Apple-native GUI app surfaces are out of scope.

Context:

The product is intended to be developed through terminal/Codex workflows rather than Apple IDE or GUI-app tooling. MacBook-only remains acceptable as an operating environment, but not as a reason to adopt Apple-native app frameworks.

Options Considered:

- Swift/AppKit/Metal native Mac app.
- Zig-first systems app.
- Rust-first Mandatum workspace.

Rationale:

Rust gives the best balance for command-line verification, PTY/event-loop work, terminal UI ecosystem, and Codex-friendly incremental development. Zig remains useful only if a later terminal parser or libghostty adapter spike justifies it.

Consequences:

- Use Rust as the default implementation stack.
- Treat terminal rendering as the first product surface.
- Do not create Apple project files or native GUI surfaces.
- Keep libghostty-vt behind a terminal adapter and defer it until after core and fake parser seams exist.

Verification:

- `docs/technology-direction.md` states the prohibited stack and Rust-first recommendation.
- `PLAN.md` and `docs/codex-goal.md` instruct Codex to avoid Apple-native GUI tooling.

## 2026-06-25: Rust Core-First Milestone 1

Status: Accepted

Decision:

Use a Cargo workspace for Milestone 1. Implement only the renderer-neutral domain in `crates/core`, minimal command metadata/dispatch in `crates/commands`, durable task/agent intent helpers in `crates/workflows`, and non-runtime boundary marker crates for `crates/pty`, `crates/terminal-vt`, `crates/renderer`, and `crates/app`.

Context:

The accepted plan calls for the smallest useful implementation foundation: deterministic workspace/session/layout/pane/action state and persistence before any PTY, parser, renderer, or app runtime work.

Options Considered:

- Build a runnable terminal app shell immediately.
- Start with PTY/parser integration.
- Start with renderer and command palette UI.
- Start with pure core state and command dispatch.

Rationale:

Core state can be tested without terminal UI, avoids early coupling to parser or renderer choices, and provides the durable contract that later runtime crates must respect.

Consequences:

- `core` owns workspace, project, session, panes, layout tree, focus, zoom, split, stack, floating panes, restart/rename intent, action results, and session persistence.
- `commands` maps command ids to core actions but does not mutate layout state directly.
- `workflows` does not launch tasks or agents in Milestone 1.
- `pty`, `terminal-vt`, `renderer`, and `app` compile but contain no runtime implementation yet.

Verification:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- Boundary check: `core` imports no PTY, terminal parser, renderer, app runtime, or terminal UI crates.

## 2026-06-25: JSON Session Persistence

Status: Accepted

Decision:

Use JSON for the first durable session persistence format, wrapped in a versioned schema field.

Context:

Milestone 1 needs persistence that is transparent, easy to inspect in tests, and sufficient for durable workspace intent without migration machinery.

Options Considered:

- JSON
- TOML
- SQLite
- Custom binary schema

Rationale:

JSON keeps the first schema simple and verifiable. The versioned wrapper gives later milestones a migration point without pulling in database or config-format decisions too early.

Consequences:

- Persist workspace/project/session/pane/layout/focus/task/agent intent.
- Do not persist PTY handles, parser state, process ids, renderer state, thread handles, or unbounded scrollback.
- Return structured errors for corrupt JSON, unsupported schema versions, and invalid session state.

Verification:

- Unit tests cover serialize/deserialize, unsupported schema, corrupt JSON, invalid session data, and runtime-handle exclusion strings.

## 2026-06-25: Fake Terminal Parser Seam First

Status: Accepted

Decision:

Start Milestone 2 by adding a fake terminal parser adapter seam in `crates/terminal-vt` before PTY runtime work or `libghostty-vt` integration.

Context:

The project needs renderer-independent terminal-state tests and a swappable adapter boundary before choosing a real parser backend.

Options Considered:

- Bind `libghostty-vt` immediately.
- Start with PTY process lifecycle.
- Start with a fake parser/grid adapter seam.

Rationale:

The fake adapter proves the public shape for feeding bytes, resizing, reading grid/cursor/cell state, and fixture testing without pulling parser, PTY, renderer, or app runtime dependencies into `core`.

Consequences:

- `crates/terminal-vt` now owns plain terminal grid, cursor, cell, capability, update, adapter, and fake-adapter types.
- Runtime process orchestration, real renderer, app shell, and
  `libghostty-vt` binding remain deferred. Later PTY decisions supersede the
  deferred backpressure, child-exit, and headless spawning parts of this
  parser-seam entry.
- Phase handoffs must include hygiene checks so placeholder or historical docs do not masquerade as current state.

Verification:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- Boundary check: `core` imports no PTY, terminal parser, renderer, app runtime, or terminal UI crates.

## 2026-06-25: Pure PTY Abstraction Before OS Spawning

Status: Accepted

Decision:

Start the PTY side of Milestone 2 with pure value types and bounded-buffer
behavior before launching real processes.

Context:

The app runtime will later orchestrate PTY output into the terminal parser
adapter. The PTY crate needs a stable byte/event contract first, without
depending on parser, renderer, app, or core crates.

Options Considered:

- Bind OS PTY spawning immediately.
- Couple PTY output directly to `terminal-vt`.
- Define process/session intent, output events, child-exit state, restart
  intent, and bounded backpressure first.

Rationale:

The pure seam makes PTY output testable and keeps parser and runtime choices
outside the PTY crate. It also lets the app layer decide how PTY bytes are fed
to `TerminalAdapter`.

Consequences:

- `crates/pty` owns PTY session/process identifiers, spawn/resize/restart
  intent, byte-stream events, child exit, and bounded byte buffering.
- This decision was the precursor to the later headless native OS PTY session
  wrapper.
- `crates/pty` does not depend on `terminal-vt`, renderer, app, or core.
- App-level PTY orchestration and visible terminal panes remain later slices.

Verification:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- Boundary check: `crates/pty` has no dependencies on parser, renderer, app,
  core, or terminal UI crates.

## 2026-06-25: Headless Native PTY Spawning Behind mandatum-pty

Status: Accepted

Decision:

Add native OS PTY spawning inside `crates/pty` only, using `portable-pty` behind
`mandatum-pty` value/event types.

Context:

The fake parser seam, pure PTY value seam, and `libghostty-vt` feasibility spike
were already in place. The next Milestone 2 gap was proving that `SpawnIntent`
can create a real PTY-backed process while still returning raw byte and child
exit data without parser, renderer, app, or core coupling.

Options Considered:

- Keep PTY as value types only until the app runtime exists.
- Hand-roll platform PTY calls.
- Add a narrow native session wrapper around `portable-pty`.
- Begin visible terminal pane work.

Rationale:

`portable-pty` provides the platform-specific open/spawn/read/write/resize/kill
surface needed for the spike. Wrapping it in `NativePtySession` keeps the
external crate contained inside `crates/pty` and preserves the local contract:
raw byte output, input bytes, resize intent, child exit, and `PtyEvent` wrappers.

Consequences:

- `crates/pty` now depends on `portable-pty`.
- `NativePtySession` is an opaque runtime handle, not durable session state.
- Native PTY output remains raw bytes; no parser or UTF-8 assumption is added.
- The wrapper can spawn, read output, write input, close input, resize, read the
  current size, try-wait, wait, wait as a `PtyEvent`, and kill.
- `core`, `terminal-vt`, `renderer`, and `app` do not depend on `portable-pty`.
- App-level orchestration, terminal parser feeding, renderer integration,
  visible terminal panes, and restart registries remain later work.

Verification:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo test -p mandatum-pty`
- Boundary check: `crates/pty` depends on `portable-pty` only, with no
  `mandatum-terminal-vt`, `mandatum-renderer`, `mandatum-app`, `mandatum-core`, parser-specific, or
  terminal UI runtime dependency.
- Boundary check: `crates/core` still has no PTY/parser/renderer/app/runtime
  dependency.

## 2026-06-25: Defer libghostty-vt Binding Behind Optional Backend Gate

Status: Accepted

Decision:

Treat `libghostty-vt` as feasible for a future optional `terminal-vt` backend,
but do not bind it in the repo yet.

Context:

The fake adapter seam and pure PTY byte/event seam are now in place, so this was
the right time to check whether Ghostty's VT library can sit behind
`TerminalAdapter` without forcing Ghostty's app architecture into this product.

Options Considered:

- Add a direct Rust FFI binding now.
- Vendor or submodule Ghostty now.
- Keep the fake adapter only and defer the research.
- Record an evidence-backed feasibility result and future binding gate.

Rationale:

The upstream C API has the required capability shape: terminal allocation, raw
byte feeding, resize, cursor/metadata reads, grid/cell/style access, render-state
snapshots, and input encoding helpers. However, upstream explicitly marks the
API as work in progress, says signatures are still in flux, and this machine
does not currently have `zig` or `cmake` on `PATH`, so a real binding cannot be
verified in this phase.

Consequences:

- `libghostty-vt` remains a promising optional backend, not the default backend.
- The fake adapter remains the only compiled `terminal-vt` backend.
- No Cargo dependency, build script, vendored source, bindgen output, or
  generated headers were added.
- A future binding must pin upstream, provide an explicit Zig/CMake/prebuilt or
  third-party binding story, avoid network fetches during normal Cargo builds,
  keep all FFI inside `crates/terminal-vt`, and preserve normal `cargo test`
  without Ghostty installed.
- `core`, `pty`, `renderer`, and `app` must not depend on Ghostty directly.

Verification:

- `docs/libghostty-vt-feasibility-spike.md` records the upstream source
  evidence, local toolchain check, adapter mapping, risks, and next binding gate.
- Boundary check: `crates/terminal-vt/Cargo.toml` has no Ghostty, Zig, CMake,
  bindgen, or FFI dependency.
- Boundary check: `core` and `pty` remain free of Ghostty/parser/renderer/app
  dependencies.

## 2026-06-26: Runnable Placeholder Terminal Shell

Status: Accepted

Decision:

Implement Milestone 3 as a terminal-native placeholder shell using Crossterm for
terminal lifecycle/events and Ratatui for drawing. Keep the runtime in
`crates/app`, the drawing code in `crates/renderer`, and all product mutations
behind `mandatum-commands` dispatch into `mandatum-core`.

Context:

Milestones 1 and 2 created durable core state, command metadata, workflow
intent helpers, terminal parser seams, and PTY seams. The next validation gate
was a root `cargo run` app that visibly reflects core layout state without
connecting a real PTY-backed terminal pane.

Options Considered:

- Continue with compile-only app and renderer placeholders.
- Build a real PTY-backed terminal pane immediately.
- Add an Apple-native GUI surface.
- Add a narrow terminal runtime shell with placeholder rendering.

Rationale:

The placeholder shell proves terminal initialization/restoration, redraw,
resize event handling, input mapping, command dispatch, and renderer/core
composition before taking on terminal-grid snapshots. Crossterm and Ratatui
are narrow terminal dependencies and keep the project buildable and verifiable
from terminal commands.

Consequences:

- Root `cargo run` launches `mandatum-app`.
- `crates/app` owns terminal lifecycle, event polling, key-to-command mapping,
  command-palette state, and resize event handling.
- `crates/renderer` owns placeholder scene construction and Ratatui drawing for
  pane chrome, focus, zoom, floating panes, status, and command metadata.
- `crates/core` remains free of terminal UI, PTY, parser, renderer, and app
  runtime dependencies.
- Real terminal process rendering, PTY byte feeding into `terminal-vt`, input
  byte encoding, PTY resize by pane size, restart registry behavior, and
  `libghostty-vt` binding remain deferred to later milestones.

Verification:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo run`
- Boundary scans for `core` and `pty` dependency leakage.

## 2026-06-26: PTY-Backed Terminal Runtime Slice

Status: Accepted

Decision:

Start Milestone 4 by wiring real PTY-backed shell processes into the terminal
app while continuing to use the current fake/basic `terminal-vt` parser. Keep
live runtime handles in `crates/app`, expose split PTY reader/writer/controller
parts from `crates/pty`, wrap the current parser behind a `TerminalParser`
owner in `crates/terminal-vt`, and let `crates/renderer` draw borrowed terminal
grid snapshots.

Context:

Milestone 3 proved terminal lifecycle, command dispatch, resize events, and
placeholder rendering. The next needed proof was end-to-end process I/O:
spawning a shell, reading PTY output without blocking input writes, feeding the
parser, drawing grid content, sending keyboard/paste input back, resizing PTYs
from pane geometry, and surfacing child exit.

Options Considered:

- Bind `libghostty-vt` before runtime wiring.
- Keep PTY reads synchronous inside the main event loop.
- Add split PTY runtime parts and keep the fake parser for the first
  end-to-end shell.

Rationale:

The current `NativePtySession` read path blocks, so app input would stall if the
same object owned reads and writes in a single thread. Splitting reader,
writer, and controller parts preserves `mandatum-pty` as a parser/app-neutral
boundary while letting the app read on a background thread and write/resize from
the event loop. Deferring `libghostty-vt` keeps this slice focused on runtime
plumbing and avoids compounding it with FFI/toolchain risk.

Consequences:

- `crates/app` now depends on `mandatum-pty` and `mandatum-terminal-vt`.
- `crates/renderer` now depends on `mandatum-terminal-vt` for grid snapshot
  value types, but still does not own runtime handles or dispatch actions.
- `crates/pty` remains independent of parser, renderer, app, core, and terminal
  UI crates.
- `crates/terminal-vt` remains independent of PTY, renderer, app, core, and
  terminal UI crates.
- Normal keys now go to the focused shell; workspace controls move behind
  `Ctrl-P` command palette mode, with `Ctrl-Q` as the app quit shortcut.
- The fake/basic parser can show shell escape sequences visibly. A real VT
  parser backend remains a later gate.
- Copy/selection, scrollback, restart registry behavior, and `libghostty-vt`
  binding remain deferred.

Verification:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo test -p mandatum-pty`
- `cargo run` smoke: shell prompt renders, `echo M4_OK` roundtrips, command
  palette split/focus/zoom works, hidden panes are not killed by zoom, and
  `Ctrl-Q` restores the terminal.
- Boundary scans for `core`, `pty`, and `terminal-vt` dependency leakage.
