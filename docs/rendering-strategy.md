# Rendering Strategy

## Premise

The product should feel as smooth and polished as a serious terminal application, but rendering should not contaminate the core workspace model.

The first architecture decision is not "which renderer forever." The first decision is to keep a clean seam between terminal state, scene composition, and terminal presentation.

## Correction On Ghostty

Ghostty is not primarily CPU-rendered. Its public docs describe a native terminal emulator using GPU acceleration, with OpenGL on Linux and Metal on macOS, plus an optimized CPU parser path.

The useful lesson for this repo is not "use Metal" or "fork Ghostty." The useful lesson is:

- strong terminal correctness
- optimized parser
- renderer tuned for terminal workloads
- careful separation between terminal core and GUI consumers

## Rendering Layers

### Terminal State

Terminal parser output:

- grid cells
- styles
- cursor
- selection
- scrollback
- image/graphics protocol metadata if supported
- mouse protocol state

### Scene Model

Renderer-neutral representation:

- workspace bounds
- pane bounds
- terminal surfaces
- chrome surfaces
- overlays
- command palette
- status surfaces
- animation intent

### Backend Renderer

Terminal-specific implementation:

- terminal drawing backend
- pane chrome
- overlays
- frame scheduling

## Early Renderer Contract

Create an interface that can render to the parent terminal:

- rectangles
- text runs
- terminal grids or terminal-grid-derived views
- cursor
- selection
- pane borders/separators
- overlays

Milestone 4 validates styled terminal-grid snapshots from the default
`TerminalAdapter` parser. Future renderer work should preserve the snapshot
contract and improve interaction/polish without taking ownership of parser
mutation, PTY handles, or runtime processes.

Milestone 5B's task-runtime slices extend that contract with read-only task
runtime views keyed by `PaneId`. The app owns the task PTY, parser, reader
thread, runtime token, exit state, rerun/stop lifecycle, and status mutation;
the renderer receives borrowed task status text and an optional terminal-grid
output snapshot for drawing only.

## Performance Targets

Initial targets:

- responsive input under heavy output
- no visible jank during pane resize
- bounded memory growth for output streams
- graceful backpressure
- recoverable parser/render errors

Later targets:

- smooth scrollback
- synchronized rendering support if terminal parser supports it
- high-DPI correctness
- efficient glyph atlas
- low idle CPU
- stable frame pacing

## What Not To Do First

Do not hand-roll a complete terminal emulator before evaluating `libghostty-vt`.

Do not put product logic in draw calls.

Do not make the core model depend on renderer coordinate types.

Do not block architecture work on GPU rendering.

Do not start by forking Ghostty.

## Renderer Spike Questions

Milestone 0 or 2 should answer:

- Can the chosen terminal renderer handle dense multi-pane output?
- Can terminal grid rendering be isolated behind a backend interface?
- Can pane chrome and terminal content be composed without fighting selection?
- Can frame scheduling avoid repainting the world on every byte?
- Can the renderer preserve terminal usability under resize, scrollback, and mouse capture?
