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
types (`WorkspaceScene`, `PaneScene`, `TerminalSurface`, `RasterSurface`,
artifact loading/ready/failed content, overlays, hit
targets), all pane-rect layout math (`scene::layout`), and the neutral input
event types (`scene::input`, now fully wired: the app consumes them
exclusively, and the terminal frontend translates crossterm events into
them in `crates/app/src/frontend.rs`). The app builds a `WorkspaceScene`
each frame (`scene_builder` converts terminal-engine grids into scene
surfaces app-side), and `mandatum-renderer` is one adapter: it draws a
scene with ratatui and computes no layout. A test-only plain-text frontend
renders the same scenes to prove the contract is renderer-neutral
(`crates/app/tests/frontend_parity.rs`), and the current native GPU renderer
uses the same contract (its source remains at `spikes/frontend-wgpu` until
workspace promotion; see docs/frontend-platform.md).

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
- bounded artifact surfaces with deterministic text fallback
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

Current bars and boundedness contracts:

- typing latency: key-to-bytes-out p50 < 25 ms (the dated measurements and
  procedure live in
  [verification.md](verification.md#input-latency-regression-check))
- bounded memory and responsiveness under a PTY flood: flow-credit
  backpressure caps in-flight bytes at 256 KiB per pane; the quit chord
  works during a `yes` flood (test
  `pty_flood_stays_bounded_responsive_and_quittable`)
- bounded scrollback memory (2000-row grid limit)
- no busy-spin idle loop (measure using the standing verification procedure)

Ongoing targets without a standing check: no visible freeze during pane
resize, recoverable parser or render failures.

Native product priorities:

- smooth scrollback
- frame pacing suitable for native display refresh
- low idle CPU
- high-DPI correctness
- efficient glyph caching
- profiling-guided redraw reduction after the shaping cache, only if justified
- large-output stress stability

The ordered path for delivering these priorities is in
[native-gpu-implementation-plan.md](native-gpu-implementation-plan.md).

## Frontend Adapter Expectations

Every frontend adapter must:

- render from scene data
- emit input and hit-test events
- avoid mutating product state directly
- expose errors as runtime-visible status
- support automated smoke tests where possible

Artifact adapters consume one scene contract. The shipped ratatui adapter
renders source, alt text, dimensions/state, and calm failure detail as cells.
The current native GPU adapter additionally consumes final-topmost
`ProgramCell::raster_layer` markers, validates each RGBA8 surface and the
64 MiB aggregate, drops every stale texture before replacement, contain-fits
without distortion, and scissors pixels around later panes and overlays. File
opening, PNG parsing, reload detection, and decoded-memory admission remain app
responsibilities, never renderer responsibilities.

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
- artifact load, reload, failure, overlay occlusion, and aspect-ratio resize

## Resize And Rewrap

Lines wrapped at a narrow width stay wrapped after the terminal grows
(classic xterm behavior). This is deliberate for now: rewrap-on-resize is a
terminal-engine concern and would belong in `mandatum-terminal-vt`'s grid,
never in the scene or a frontend. Revisit only with adapter-conformance
coverage for both backends.
