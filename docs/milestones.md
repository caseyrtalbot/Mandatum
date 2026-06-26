# Milestones

## Milestone 0: Product And Architecture Packet

Goal: finalize the planning foundation before implementation.

Deliverables:

- product principles
- non-goals
- user workflows
- interaction model
- architecture boundaries
- technology recommendation
- rendering strategy
- terminal substrate evaluation plan
- verification plan

Validation:

- docs are internally consistent
- open implementation questions are explicit
- chosen first milestone is small enough for Codex to execute
- no existing codebase is treated as source material

## Milestone 1: Core Domain

Goal: implement renderer-neutral workspace state and actions.

Status: Implemented as the first Cargo workspace scaffold. Runtime crates are
no longer placeholders; `pty`, `terminal-vt`, `renderer`, and `app` now have
Milestone 4 implementations documented below.

Deliverables:

- workspace model
- project model
- pane specs
- layout tree
- split/stack/floating/zoom/focus actions
- command action model
- session persistence schema
- migration/error handling shape
- unit tests

Validation:

- core tests run without GUI
- no PTY, parser, renderer, or app-runtime types leak into core
- layout/focus behavior is deterministic
- session state serializes durable intent only

## Milestone 2: PTY And Terminal Adapter Spike

Goal: prove process and terminal-state seams.

Status: Implemented as the parser/PTY seam milestone. It added the fake parser
adapter seam in `crates/terminal-vt`, pure PTY abstraction plus headless native
OS PTY support in `crates/pty`, and the `libghostty-vt` feasibility spike.
`libghostty-vt` binding remains deferred; app runtime, renderer integration,
and visible PTY-backed panes were delivered in later milestones.

Deliverables:

- PTY abstraction
- headless native OS PTY spawning
- fake terminal parser adapter
- terminal adapter trait/interface
- fixture-based stream tests
- terminal capability model
- libghostty-vt evaluation spike

Validation:

- fake parser can drive renderer-independent tests
- PTY output can be consumed with bounded backpressure
- native PTY sessions can spawn, read raw output, write input, resize, report
  child exit, and kill without parser/UI coupling
- child exit and restart are represented cleanly
- adapter can be swapped without core changes

## Milestone 3: Terminal Runtime Prototype

Goal: create the first runnable terminal application shell.

Status: Implemented as the first runnable terminal UI shell. `cargo run`
launches Mandatum, restores the terminal on quit, renders core workspace layout
state, handles resize events, dispatches existing command ids, and shows a
command palette overlay. Real PTY-backed pane rendering was delivered in
Milestone 4.

Deliverables:

- terminal initialization/restoration
- workspace scene inside the terminal
- panes from core state
- focus actions
- split/resize actions
- command palette overlay
- basic settings/config path

Validation:

- app launches from `cargo run`
- core actions update visible layout
- no core dependency on terminal UI types
- terminal resize updates scene correctly

## Milestone 4: Real Terminal Pane

Goal: connect one real process to one visible terminal pane.

Status: Complete. `cargo run` spawns shells for visible terminal panes, reads
PTY output on background reader threads, feeds bytes into a hardened VT parser
behind `TerminalAdapter`, renders terminal grid plus scrollback snapshots, sends
normal key input and paste text back to the focused PTY, resizes PTYs from pane
geometry, reports process exits, supports a keyboard copy/scrollback mode, and
restarts a pane's PTY in place for the same pane identity. The default parser
backend is now a local VT state machine built on the pure-Rust `vte` tokenizer;
the older fake/basic adapter is retained for fixtures only. `libghostty-vt`
remains a deferred optional backend.

Deliverables:

- spawn shell: implemented for visible terminal panes
- terminal parser hardening: implemented behind `TerminalAdapter` with a
  pure-Rust `vte` tokenizer backend, SGR styling, cursor addressing,
  erase/insert/delete, scroll region, alternate screen, and save/restore cursor
- render terminal grid: implemented, with colored/styled cells and a
  scrollback-aware viewport
- scrollback: implemented as bounded, runtime-owned terminal history (not in
  durable core state)
- send key input: implemented for normal keys when command palette is closed
- paste: implemented through Crossterm paste events (suppressed in copy mode)
- resize PTY with pane: implemented from renderer pane content geometry
- copy/selection baseline: implemented as a keyboard copy mode (scrollback
  navigation, stream selection, OSC 52 clipboard copy)
- restart registry: implemented; a restart relaunches a fresh PTY for the same
  `PaneId` without serializing runtime handles or mutating core layout
- process exit handling: implemented as visible status

Validation:

- common shell interaction works without raw escape/control sequence leakage
- terminal pane survives resize
- output does not freeze UI under moderate load
- failed child process is visible
- restart creates a fresh PTY for the same pane and leaves core layout intact
- scrollback is bounded and lives in runtime/presentation state
- copy/selection baseline is implemented and documented

Deferred to later milestones:

- `libghostty-vt` backend binding
- native OS mouse selection, rich clipboard history, semantic selection

## Milestone 5A: Workspace Open/Restore And Layout Persistence

Goal: persist and restore durable workspace/session layout intent from disk.

Status: Complete. Mandatum saves workspace JSON to `.mandatum/workspace.json`
under the project path, restores that file on startup when present, exposes
explicit save/restore commands through the command path, validates restored
state and stages fresh PTYs before swapping workspaces, preserves the current
workspace on restore failure, and activates fresh PTYs for restored visible
terminal panes.

Deliverables:

- app-level session path decision
- startup restore path for a saved workspace
- explicit save/restore command handling in `app`
- fresh PTY runtime reconciliation for restored terminal panes
- visible restore/save status and failure handling
- tests proving restored layout intent does not include runtime handles,
  process ids, parser state, scrollback, renderer state, or thread handles

Validation:

- saved workspace JSON contains durable core intent only
- restore recreates panes/layout/focus from disk and launches fresh live PTYs
  for visible terminal panes
- restore failure is visible and does not corrupt the current workspace
- `core` remains renderer-neutral and runtime-handle-free
- docs and handoff identify task/agent workflow runtime as later work

## Milestone 5B: Multi-Pane Coding Workflow

Goal: support useful coding sessions.

Status: Started. The first task-runtime slices are complete: `Run Task` creates
a durable task pane intent and the app launches one configured shell command in
that task pane; focused task panes can now be explicitly rerun or stopped.
Live task process handles, parser state, reader threads, output buffers, exit
state, runtime tokens, and status strings are owned by `crates/app`; durable
core state stores only task command intent (`recipe_id`, `command`, and `cwd`).
Renderer task status/output views are read-only runtime inputs. Tasks launched
or rerun while hidden by zoom are tracked as pending app runtime launches;
failed launches and stopped tasks surface app-owned status without serializing
failure or stopped state.

Deliverables:

- multiple terminal panes beyond the current split/stack/floating baseline:
  implemented before 5B
- build/test task recipes: first configured shell command slice implemented
- task status surfaces: first running/succeeded/failed status and output surface
  implemented
- rerun/stop commands: implemented for the focused task pane through app-owned
  runtime commands
- command history: deferred
- layout persistence hardening on top of the Milestone 5A disk-backed baseline:
  task intent persists; live task runtime does not auto-relaunch on restore

Validation:

- user can launch one configured shell task from the command palette
- user can rerun or stop the focused task without mutating durable core state
- workspace can close and reopen with useful intent restored
- task failure is visible and actionable
- `RestartPane` remains shell-only for task panes; task rerun uses explicit
  runtime task command semantics
- saved workspace JSON excludes task process handles, process IDs, parser state,
  reader threads, runtime tokens, output buffers, runtime status, stopped state,
  and scrollback

## Milestone 6: Agent Surface

Goal: make agents first-class without making the product chat-first.

Deliverables:

- agent/thread pane model
- active objective display
- status/progress display
- pending approval surface
- changed files summary
- test/check result display
- open external Codex/thread action

Validation:

- agent state is useful at a glance
- agent pane does not replace terminal workflow
- user can distinguish running, blocked, failed, and complete agent work

## Milestone 7: Polish And Hardening

Goal: refine the product under real development load.

Deliverables:

- keyboard config
- theme system
- accessibility pass
- performance benchmarks
- crash/recovery rules
- config validation
- onboarding-minimal first-run

Validation:

- low idle CPU
- smooth resize and scroll
- reliable session recovery
- readable under high output
- no regressions in core tests
