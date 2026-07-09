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

scene model
  pane bounds, surfaces, overlays, selections, hit targets, animation intent

frontend adapter
  terminal drawing, native drawing, GPU drawing, platform input
```

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

Initial targets:

- responsive typing during moderate output
- no visible freeze during pane resize
- bounded memory growth for scrollback
- stable redraw while tasks stream output
- recoverable parser or render failures

Advanced targets:

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
