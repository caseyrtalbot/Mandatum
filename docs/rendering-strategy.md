# Rendering Strategy

## Goal

Mandatum should feel smooth, crisp, and stable while rendering dense development
output across multiple panes, tasks, and agents.

Rendering must communicate structure without stealing ownership of product
behavior.

## Rendering Stack

```text
terminal/runtime data
  parser grids, task status, agent state, workflow history

scene model (mandatum-scene)
  pane bounds, surfaces, overlays, selections, hit targets, animation intent

frontend adapter (mandatum-renderer is the terminal adapter)
  terminal drawing, native drawing, GPU drawing, platform input
```

The scene contract is implemented: `mandatum-scene` owns the neutral scene
types (`WorkspaceScene`, `PaneScene`, `TerminalSurface`, overlays, hit
targets), all pane-rect layout math (`scene::layout`), and the neutral input
event types (`scene::input`, now fully wired: the app consumes them
exclusively, and the terminal frontend translates crossterm events into
them in `crates/app/src/frontend.rs`). The app builds a `WorkspaceScene`
each frame (`scene_builder` converts terminal-engine grids into scene
surfaces app-side), and `mandatum-renderer` is one adapter: it draws a
scene with ratatui and computes no layout. A test-only plain-text frontend
renders the same scenes to prove the contract is renderer-neutral
(`crates/app/tests/frontend_parity.rs`), and the GPU spike renders from the
same contract (`spikes/frontend-wgpu`, see docs/frontend-platform.md).

## Scene Requirements

The scene model must describe:

- root workspace bounds
- tiled pane surfaces
- stacked pane surfaces
- floating pane surfaces
- zoomed pane surfaces
- terminal grid surfaces
- task output/status surfaces
- agent status surfaces
- command palette
- session map
- execution timeline
- status strips
- overlays
- selection rectangles
- cursor state
- hit targets
- animation intent

No scene type should require a specific frontend framework.

## Visual Principles

- Dense output must remain readable.
- Pane chrome should be thin and useful.
- Attention states should be clear without shouting.
- Failures should be visible near the thing that failed.
- Agent and task state should be glanceable.
- Empty space should serve scanning, not decoration.
- Motion should clarify state changes, not entertain.

## Text And Terminal Quality

The renderer should support:

- crisp monospace text
- bold, dim, italic, underline, inverse, hidden, and strikethrough styles
- ANSI indexed color
- true color
- stable cursor rendering
- scrollback rendering
- selection rendering
- wrapped-line fidelity
- alternate-screen behavior
- copy/search affordances

## Performance Targets

Met and held by standing checks:

- typing latency: key-to-bytes-out p50 13.30 ms on the event-driven loop,
  regression bar p50 < 25 ms (procedure and numbers:
  docs/verification.md, "Input Latency Regression Check")
- bounded memory and responsiveness under a PTY flood: flow-credit
  backpressure caps in-flight bytes at 256 KiB per pane; the quit chord
  works during a `yes` flood (test
  `pty_flood_stays_bounded_responsive_and_quittable`)
- bounded scrollback memory (2000-row grid limit)
- low idle CPU: ~0.1% over 30 s idle (docs/verification.md)

Ongoing targets without a standing check: no visible freeze during pane
resize, recoverable parser or render failures.

Advanced targets (GPU-frontend territory, not yet product goals):

- smooth scrollback
- frame pacing suitable for native display refresh
- low idle CPU
- high-DPI correctness
- efficient glyph caching
- minimized redraw regions
- large-output stress stability

## Frontend Adapter Expectations

Every frontend adapter must:

- render from scene data
- emit input and hit-test events
- avoid mutating product state directly
- expose errors as runtime-visible status
- support automated smoke tests where possible

## Quality Gates

Rendering work is not complete until it has been checked under:

- empty workspace
- dense multi-pane output
- rapid terminal output
- task failure output
- agent waiting-for-approval state
- resize
- scrollback
- selection
- restored workspace

## Resize And Rewrap

Lines wrapped at a narrow width stay wrapped after the terminal grows
(classic xterm behavior). This is deliberate for now: rewrap-on-resize is a
terminal-engine concern and would belong in `mandatum-terminal-vt`'s grid,
never in the scene or a frontend. Revisit only with adapter-conformance
coverage for both backends.
