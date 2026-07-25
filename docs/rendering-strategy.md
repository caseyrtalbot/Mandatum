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
(`crates/app/tests/frontend_parity.rs`), and `mandatum-native-renderer` uses
the same contract from `crates/native-renderer` (see
docs/frontend-platform.md).

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

## Native Visual Presentation

Production native polish follows the ordered
[visual polish plan](visual-polish-plan.md). The native presentation layer
builds on the semantic scene rather than treating the terminal cell program as
its design ceiling:

- `WorkspaceScene` and typed scene extensions own product meaning;
- `CellProgram` remains the exact terminal-parity projection and maintained
  fallback;
- the pure native presentation plan translates typed scene surfaces into
  bounded ordered materials, clips, scoped text, and typed transitions without
  reconstructing state;
- the GPU adapter materializes those primitives but never parses
  `detail_lines()` or invents workflow hierarchy;
- typed workflow rows resolve to compact status badges, contained
  failure/approval callouts, bounded metadata/list regions, exact console
  material, and artifact inspector/canvas material;
- hidden workflow nodes retain stable identity without emitting material or
  text, and a ready artifact with missing or mismatched typed canvas geometry
  fails preparation rather than silently dropping pixels;
- terminal content, child mouse reporting, hit targets, and cell ownership stay
  authoritative while native chrome gains materials, density, and motion.

The flagship direction is a compact graphite workbench: quiet continuous tiled
surfaces, raised floating/modal surfaces, one navigation accent, and distinct
semantic colors reserved for waiting, failure, success, and completion.
Decorative effects are not substitutes for hierarchy. Each built-in theme owns
both sides of that presentation contract: `mandatum-light` and
`mandatum-high-contrast` now have complete terminal foreground/background and
16-color palettes as well as their UI palettes, while `mandatum-dark` retains
its accepted palette byte-for-byte. App-owned surfaces and terminal defaults
therefore agree instead of placing a dark terminal palette inside light or
high-contrast chrome.

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

Visual-polish work additionally uses the portable contrast, geometry, motion,
fixed-reference macOS baseline, resize, idle, and frame-preparation procedures
owned by [verification.md](verification.md) when their seam changes or focused
evidence exposes risk. Pixel baselines supplement semantic tests; they never
replace them or update implicitly.

The deterministic visual-scenario catalog lives in
`crates/app/src/visual_scenario.rs`: it prepares product fixtures through the
core model and drives the real `FrontendHost` only with neutral input. The
excluded native lab's `--visual-scenario` route displays those same scenes;
`visual-regression.swift` captures a genuine fixed-reference client surface,
and `visual-diff` owns read-only comparison plus explicit human acceptance.
Neither capture nor comparison derives product meaning or changes production
renderer behavior.

The native translation seam preserves the shared cell paint. App-built scenes
carry stable opaque semantic node ids, fixed-point
logical rectangles, cell projections, PTY mappings, hit targets, transition
targets, and accessibility meaning. `prepare_native_presentation` validates
their hierarchy, bounds, clips, ordering, and aggregate resource ceilings
headlessly. Interface text uses typed metric roles and only the four
provisioned static faces. `NativeTextScope` role identity now controls the
app-owned font face, point size, and line box; cell bold and italic remain
additive semantic emphasis rather than being discarded by the role face.
Metric generation plus role slot are part of shaping-cache identity. A scope
that occupies one painted cell row centers that row in its semantic logical
rectangle. A scope with text on multiple rows divides its rectangle into
nonoverlapping proportional row bands and centers each row in its band.
Horizontal origins and clips remain terminal-grid exact. App-owned shaped
advance scales by the role font size divided by the configured terminal font
size; child-terminal text retains the configured terminal metrics and exact
cell advance. Direct UI colors come from `Theme.ui`, never from terminal ANSI
identity.

The everyday workspace is materialized through that seam. Canvas,
tiled-pane, header/status, title, badge, attention, separator, focus, and
floating-shell primitives come from typed plan commands. Modern app-owned
chrome/default pane backgrounds no longer repaint those materials through the
legacy cell path; explicit terminal backgrounds, cursor, selection, raster, and
overlay behavior remain authoritative. Pane decoration suppression is a typed
`CellProgram` scope, native text color comes from bounded plan projections, and
floating shadow fragments are bounded and clipped around later raised panes.
Compact/comfortable density changes native rail geometry only; terminal cell
and PTY geometry remain exact.

Fixed-reference visual baselines live under
`spikes/frontend-wgpu/visual-baselines/`. They may be replaced only through
explicit reviewed acceptance. The set covers typed rails, focus treatment,
badges, separators, attention chips, bounded floating material and shadow,
overlays, typography, workflow surfaces, and motion checkpoints while retaining
exact terminal paint.

The native overlay material stack sits above workspace materials, terminal
backgrounds, and raster artifacts: modal scrim, overlay
shadow, raised shell, inset bands/soft selection/leading indicator, late
overlay cursor quads, then text. Welcome omits the scrim; Context Menu stays
anchored and also omits it. Representative references cover Palette, full
modal, Welcome, and artifact-plus-menu states.

One deterministic renderer-local `PresentationMotion` state machine resolves
typed transition targets. The scene names Focus,
Selection, Overlay, PaneGeometry, and ApprovalArrival intent against stable
nodes; the renderer applies only typed geometry, opacity, or scale properties
with the configured motion tokens and an injected monotonic instant. Overlay
opacity covers root/band/item materials and cell-owned text, while scale and
pane geometry apply only to native material-backed commands. Cell-owned glyph
placement, child output, and artifact raster placement remain direct. Overlay
close snaps because the new scene no longer contains its glyph rows; retaining
only renderer-owned material would show an incoherent empty shell. Equal plans
do not restart motion; interruption and reversal start from the currently
sampled presentation and converge on current scene truth.

The GPU exposes its next animation deadline separately from product/runtime
time. The native shell chooses the earlier of that deadline and the child-exit
heartbeat, requests redraw for an elapsed motion deadline or changed scene
generation, and does not repaint a static workspace merely because the
heartbeat ran. Direct pointer geometry, live resize, typing, and output snap to
the latest stable scene. Reduced motion also snaps, clears scheduled animation
work, and retains static semantic emphasis. Pointer routing is suspended while
pane or overlay hit-bearing geometry is visually between the scene-owned
endpoint targets.

The fixed-reference motion matrix freezes approval arrival at start,
midpoint, end, and reduced-motion state only after the target frame has
presented. The capture path retains the exact real-viewport snapshot and target
instant across bounded surface retries, publishes readiness through the final
window title, and fails closed rather than accepting a stale pre-checkpoint
frame.

Filtered overlays reserve two-row blocks for the input, each visible item, and
the footer; Session Map reserves two-row item/footer blocks; Context Menu uses
two-row item blocks. Paint, native presentation nodes, and pointer hit targets
share those exact rectangles. At the default native font and the maintained
18-point large-font setting, each control is at least 28 logical pixels high
and adjacent targets do not overlap. Selection-aware windows preserve access
when fewer items fit. Empty-result copy is confined to an item block and never
overwrites the filter input, and the prompt's IME target remains anchored to
its full input block. These overlay-only rows do not change pane layout,
terminal viewport mappings, or PTY size.

The maintained 18-point display check showed no clipping or overlap, and
app-owned text no longer falls back to the terminal's global metric. Pane title
rails remain one terminal row; making them taller without consuming terminal
content requires a later pane-layout/PTY contract change rather than a
renderer-only adjustment.

## Resize And Rewrap

Lines wrapped at a narrow width stay wrapped after the terminal grows
(classic xterm behavior). This is deliberate for now: rewrap-on-resize is a
terminal-engine concern and would belong in `mandatum-terminal-vt`'s grid,
never in the scene or a frontend. Revisit only with adapter-conformance
coverage for both backends.
