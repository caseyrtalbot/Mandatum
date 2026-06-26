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

Status: Implemented as the first Cargo workspace scaffold. Runtime crates remain placeholders.

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

Status: Started with the fake parser adapter seam in `crates/terminal-vt`, the
pure PTY abstraction and headless native OS PTY seams in `crates/pty`, and a
`libghostty-vt` feasibility spike. Real `libghostty-vt` binding, renderer
integration, visible terminal panes, and app runtime remain deferred.

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

Deliverables:

- terminal initialization/restoration
- placeholder workspace scene inside the terminal
- panes from core state
- focus actions
- split/resize actions
- command palette placeholder
- basic settings/config path

Validation:

- app launches from `cargo run`
- core actions update visible layout
- no core dependency on terminal UI types
- terminal resize updates scene correctly

## Milestone 4: Real Terminal Pane

Goal: connect one real process to one visible terminal pane.

Deliverables:

- spawn shell
- render terminal grid
- send key input
- paste
- resize PTY with pane
- copy/selection baseline
- process exit handling

Validation:

- common shell interaction works
- terminal pane survives resize
- output does not freeze UI under moderate load
- failed child process is visible and restartable

## Milestone 5: Multi-Pane Coding Workflow

Goal: support useful coding sessions.

Deliverables:

- multiple terminal panes
- project workspace open/restore
- build/test task recipes
- task status surfaces
- rerun/stop commands
- command history
- layout persistence

Validation:

- user can run editor, shell, tests, and server together
- workspace can close and reopen with useful intent restored
- task failure is visible and actionable

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
