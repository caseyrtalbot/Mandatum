# Frontend spike: winit + wgpu GPU terminal frontend

Status: **Phases 3 and 4 are complete in the excluded adapter. Layout,
content/style, input/lifecycle, and bounded Artifact Preview all cross the real
host; production GPU admission remains blocked.**
A native macOS window drives `mandatum_app::FrontendHost` and its real
`RuntimeEngine`, translates winit events to neutral `InputEvent` values, and
renders the host's real header, terminal, task, agent, and Empty panes, status
strip, command palette, context menu, execution timeline, session map, Set agent
objective prompt, session-output Search, generated Help, and generated Welcome.
One scene compiler consumes arbitrary ordered pane layouts, including tiled,
stacked, zoomed, mixed-content, three-plus-pane, moved/custom-float,
multiple-float, and overlay combinations. Typed clipboard effects return to the
native shell.

Phase 4 adds the real Artifact Preview path: the app resolves project-relative
static PNG intent into bounded loading/ready/failed RGBA8 scene data, the
terminal adapter keeps a deterministic labeled fallback, and the isolated GPU
renderer uploads the same ready bytes as an sRGB texture, contain-fits them,
and clips them with final-topmost raster markers. The texture cache is
revision-aware and evicts every stale layer before allocating replacements.

This remains an isolated frontend outside the Cargo workspace (the root
`Cargo.toml` excludes `spikes/frontend-wgpu`), so its heavy GPU dependency tree
never joins the product workspace, build, release artifacts, or merge gate. The
displayed adapter depends on `mandatum-app`; it no longer owns a
`TerminalSession`, parser, grid-to-scene bridge, key-to-byte encoder, or
`AtomicBool` wake coalescer, and it has no direct `mandatum-terminal-vt`
dependency. The separate `tui_probe` binary still uses `mandatum-pty` as an
external terminal latency harness. The isolated renderer consumes only
`mandatum-scene` plus paint/window crates.

Phase 2 verification (2026-07-22): the focused real-host wake test passed once;
`./ci/gpu-spike.sh` passed six tests plus the renderer-boundary scan;
`cargo test -p mandatum-app --lib` passed 248 tests; and the full
`./ci/gate.sh` was green. A displayed macOS smoke typed `printf GPU_HOST_OK`,
opened the real palette with Ctrl+P, closed it with Escape, quit with Ctrl+Q,
and left no native-spike or child-shell process. The fresh terminal probe
measured p50 11.39 ms / p95 12.56 ms / max 13.69 ms over 100 samples with zero
misses. Restore and broader parity remain Phase 3; Artifact Preview and
production GPU admission remain pending.

Phase 3 task/agent verification (2026-07-22): real-host tracer bullets first
failed with `PaneContent("task")` and `PaneContent("agent")`, then passed through
the same `prepare_scene` plan the displayed renderer consumes. The task path
proves live `RuntimeEngine` output below one-row fitted metadata; the agent path
preserves wrapped scene detail. `./ci/gpu-spike.sh` passed ten tests plus the
renderer-boundary scan, and all 248 app library tests passed. Displayed release
smokes showed the real task metadata/live output and the real agent objective
and state, then quit without a native or task child process. Empty content,
multi-pane layouts, remaining overlays, restore, broader input, Artifact
Preview, and production admission remain pending. The final `./ci/gate.sh`
passed after these synchronized documentation edits.

Phase 3 Empty verification (2026-07-22): the fresh real-host tracer bullet first
failed with `PaneContent("empty")`, then passed through the same `prepare_scene`
plan the displayed renderer consumes. The plan retains the scene-composed cwd,
restart generation, and no-live-grid detail with pane-body wrapping.
`./ci/gpu-spike.sh` passed eleven tests plus the renderer-boundary scan, and all
248 app library tests passed. A displayed release smoke used an intentionally
missing shell in a disposable project to produce the real Empty fallback; all
three detail lines, existing header, pane geometry, status, and theme painted,
then Ctrl+Q exited with no native-spike or missing-shell process left. Multiple
panes, remaining overlays, restore, broader input, Artifact Preview, and
production admission remain pending. The final `./ci/gate.sh` passed after
these synchronized documentation edits.

Phase 3 context-menu verification (2026-07-22): a fresh real-host tracer bullet
used the exact pane-body target from the initial frame, sent a neutral
right-click, proved the next frame carried `OverlayScene::ContextMenu`, and
first failed with `Overlay("context menu")`. The final plan retains the menu's
resolved area, rows, chord hints, and selected index. `./ci/gpu-spike.sh` passed
thirteen tests plus the renderer-boundary scan, and all 248 app library tests
passed. A displayed missing-shell smoke kept the real Empty pane and product
chrome beneath the bordered menu, painted all twelve real rows and hints with
the first selected, closed with Escape, and quit with Ctrl+Q without leaving a
native or attempted-shell process. Multi-pane layouts, remaining overlays,
restore, broader input, Artifact Preview, and production admission remain.

Phase 3 timeline verification (2026-07-22): a real-host tracer bullet used a
writable disposable workspace with PTY spawning disabled, drove the neutral
Ctrl+P then `/` route, proved `OverlayScene::Timeline` contained the recorded
Show timeline dispatch, and first failed with `Overlay("timeline")`. The final
plan retains the resolved area, query, ordered glyph/time/text rows, selected
index, skipped-malformed count, footer, and explicit no-match state.
`./ci/gpu-spike.sh` passed sixteen tests plus the renderer-boundary scan, and all
248 app library tests passed. A
displayed missing-shell smoke kept the real Empty pane and product chrome
beneath a centered bordered Timeline, painted the selected event and live
`show` filter, and confirmed a `zzzz` query paints `no matching events` without
crossing the border. It closed with Escape and quit with Ctrl+Q without leaving
a native or attempted-shell process. Multi-pane layouts, remaining overlays,
restore, broader input, Artifact Preview, and production admission remain.

Phase 3 session-map verification (2026-07-22): a fresh real-host tracer bullet
with PTY spawning disabled drove the neutral Ctrl+P then `m` route, proved
`OverlayScene::SessionMap` contained the real active-session heading and
focused pane row, and first failed with `Overlay("session map")`. The final plan
retains the resolved area, ordered tree rows, depth, glyph, label, live state,
focus marker, layout badges, selected index, and footer. `./ci/gpu-spike.sh`
passed eighteen tests plus the renderer-boundary scan, and all 248 app library
tests passed. A displayed missing-shell smoke kept the real Empty pane and
product chrome beneath a centered bordered Sessions map, painted the active
session and selected focused `pane-1 terminal` row with its `idle` state and
bounded footer, closed with Escape, and quit with Ctrl+Q without leaving a
native or attempted-shell process. Multi-pane layouts, remaining overlays,
restore, broader input, Artifact Preview, and production admission remain.

Phase 3 objective-prompt verification (2026-07-22): a fresh real-host tracer
bullet with PTY spawning disabled created and zoomed an agent with a distinctive
configured objective, opened Set agent objective, proved the real prompt carried
its resolved area, focused pane title, objective input, and footer, and first
failed with `Overlay("prompt")`. The final plan retains that scene data and
paints the semantic overlay surface, border, title, bounded input, block cursor,
and pinned footer. `./ci/gpu-spike.sh` passed twenty tests plus the renderer
boundary scan, and all 248 app library tests passed. A displayed missing-shell
smoke queued create-agent and zoom before the next redraw, then showed the real
zoomed agent beneath its centered objective prompt with a visible cursor and
bounded footer. Escape and Ctrl+Q closed it cleanly, and no native or
attempted-shell process remained. Multiple panes, remaining overlays, restore,
broader input, Artifact Preview, and production admission remain pending.

Phase 3 Search verification (2026-07-22): a fresh real-host tracer bullet used a
writable disposable project with PTY spawning disabled, created and zoomed an
agent, drove neutral Ctrl+Shift+F, and matched the deterministic
`search-session` timeline event. It proved the product Search scene retained
resolved geometry, live query, grouped source, result text, char match indices,
selection, overflow/footer state, and the aligned row target before first
failing with `Overlay("search")`. The final plan retains that scene data and
paints the semantic overlay surface, border, title, block cursor, grouped rows,
selection, and pinned footer. Its Search-only pane-text occlusion keeps base
agent glyphs outside the opaque modal. `./ci/gpu-spike.sh` passed 24 tests (two
native-shell, nine real-host, thirteen isolated-renderer) plus the renderer
boundary scan, and all 248 app library tests passed. A displayed missing-shell
smoke pasted `kind:timeline search`, showed the selected first result and
repeated-source elision over the real zoomed agent, then Escape and Ctrl+Q
closed it with exit 0 and no native process left. Current Search indexes runtime
pane output and timeline snapshots, not durable agent-objective text; this
scene-only increment deliberately did not change that product behavior.
Multiple panes, Help/Welcome, restore, broader input, Artifact Preview, and
production admission remain pending.

Phase 3 Help verification (2026-07-22): a fresh real-host tracer bullet with PTY
spawning disabled drove neutral F1 over the supported Empty pane and typed
`search session output`. It proved the product Help scene retained its resolved
area, live query, ordered App heading and Search command row, configured
`ctrl+shift+f` route, selected index, and footer before first failing with
`Overlay("help")`. The final plan retains that scene data and paints the
semantic overlay surface, border, grouped rows, selection, block cursor, key
hints, and pinned footer; opaque Help clipping keeps base-pane glyphs outside
the modal. `./ci/gpu-spike.sh` passed 26 tests (two native-shell, ten real-host,
fourteen isolated-renderer) plus the renderer boundary scan, and all 248 app
library tests passed. A displayed missing-shell smoke opened Help with F1,
filtered to the App heading and Search command, showed its live route, selection,
cursor, and footer without glyph leakage, then Escape and Ctrl+Q closed it with
exit 0 and no native or attempted-shell process left. Multiple panes, Welcome,
restore, broader input, Artifact Preview, and production admission remain
pending.

Phase 3 Welcome verification (2026-07-22): a real-host tracer bullet used a
writable disposable project with no workspace file, startup restore enabled,
and PTY spawning disabled. A neutral resize preserved the real first-run note
over the Empty pane and proved its resolved area, introduction, ordered
generated `ctrl+p`, right-click, F1, and Ctrl+Q routes and descriptions, and
dismissal text before first failing with `Overlay("welcome")`. The final plan
retains that exact scene data, aligns and bounds the route rows, and paints the
semantic opaque surface, palette border, introduction, entries, and dismissal;
Welcome joins Search and Help in pane-text occlusion. `./ci/gpu-spike.sh` passed
28 tests (two native-shell, eleven real-host, and fifteen isolated-renderer)
plus the renderer boundary scan, and all 248 app library tests passed. Because
the excluded native shell deliberately keeps startup restore disabled, the
displayed smoke used a disposable harness compiled against the exact local
`FrontendHost`, scene contract, and GPU renderer. It showed the real Welcome
over the Empty pane, Escape dismissed the non-modal note, Ctrl+Q exited 0, and
no smoke or native-spike process remained. Multiple panes, restore in the
excluded native shell, broader input, Artifact Preview, and production
admission remain pending.

Phase 3 two-horizontal-Empty-pane verification (2026-07-22): a real-host tracer
bullet used PTY spawning disabled, resized to 80x24, and drove neutral Ctrl+P
then `v` through the product's generated Split pane right route. It proved the
two scene-owned 40x22 side-by-side rectangles, `terminal` and `terminal 2`
titles, focus on the right pane, layout flags, and Empty detail before first
failing with `PaneCount(2)`. The prepared plan now retains an ordered per-pane
record, and the GPU adapter paints both panes with separate title/body buffers
while preserving its one-pane API and every covered one-pane content/overlay
path. Admission remains limited to the exact two-horizontal-Empty shape.
`./ci/gpu-spike.sh` passed 29 tests (two native-shell, twelve real-host, and
fifteen isolated-renderer) plus the renderer boundary scan, and all 248 app
library tests passed. A displayed missing-shell release smoke showed the real
header reporting `2 pane(s)`, equal left/right panes, both Empty details, and
focused `terminal 2`; the disposable process was stopped after capture and no
native process remained. Vertical, stacked, floating, dense, mixed-content,
and three-plus-pane layouts, restore, broader input, Artifact Preview, and
production admission remain pending.

Phase 3 two-vertical-Empty-pane verification (2026-07-22): a real-host tracer
bullet used PTY spawning disabled, resized to 80x24, and drove neutral Ctrl+P
then `s` through the product's generated Split pane down route. It proved the
two scene-owned 80x11 top-to-bottom rectangles, `terminal` and `terminal 2`
titles, focus on the lower pane, layout flags, and complete Empty detail before
first failing with `Layout("only two horizontal tiled Empty panes")`. The
prepared plan now admits the exact vertical tiled shape; the existing
scene-order GPU adapter paints both panes from their separate rectangles and
title/body buffers while preserving every one-pane and horizontal two-pane
path. `./ci/gpu-spike.sh` passed 32 tests (two native-shell, fourteen real-host,
and sixteen isolated-renderer) plus the renderer boundary scan, and all 248 app
library tests passed. A displayed missing-shell release smoke showed the real
header reporting `2 pane(s)`, equal top/bottom panes, complete Empty details,
and focused `terminal 2`; Ctrl+Q exited and no native or attempted-shell process
remained. Stacked, floating, dense, mixed-content, and three-plus-pane layouts,
restore, broader input, Artifact Preview, and production admission remain
pending. A fresh cold read added a real-host regression for the one-visible-pane
stack shape and an isolated negative matrix for vertical overlays, forbidden
flags, invalid geometry, and mixed content; all now fail closed explicitly.

Phase 3 two-pane-floating-Empty verification (2026-07-23): the required
real-host tracer used PTY spawning disabled, resized to 80x24, then drove
neutral Ctrl+P and `v` followed by Ctrl+P and `f`. It proved tiled `pane-1` at
`(0, 1, 80, 22)`, focused floating `pane-2` at `(8, 5, 72, 18)`, durable
titles, exact layout flags, and complete Empty detail before first failing with
`Layout("only two horizontal or vertical tiled Empty panes")`. The prepared
plan now admits that exact default floating shape by comparing against the
scene layer's canonical `FloatingRect::default()` resolution and clamping
result. The existing scene-order GPU path paints both pane records without
owning layout policy. The float paints an opaque background and clips
lower-pane title/body glyph bounds around its scene-owned rectangle. An
isolated negative matrix rejects overlays, forbidden flags, altered tiled or
floating geometry, and mixed content. The first displayed attempt exposed the
real intermediate two-horizontal-Empty plus Palette frame required to dispatch
Float; a second RED now covers that frame, and only that exact two-pane Palette
route was admitted. A cold reviewer found that lower-pane glyphs could render
over the float after its quads; the fix adds opaque fill, bounds clipping, and a
long wrapped-cwd regression. `./ci/gpu-spike.sh` passed 36 tests (two
native-shell, sixteen real-host, and eighteen isolated-renderer) plus the
boundary scan, and all 248 app library tests passed. A displayed missing-shell
release smoke repeated from the review-fixed binary with a long wrapping
project path showed `2 pane(s)`, the tiled `terminal` clipped behind focused
floating `terminal 2`, and complete Empty detail; Ctrl+Q exited 0 and no native
or attempted-shell process remained. Stacked, broader floating, dense,
mixed-content, and three-plus-pane layouts remain unsupported.

Corrective verification (2026-07-23): focused RED first proved that the scene
layer exposed no canonical default-float resolver and that the real
two-horizontal-Empty Palette transition exposed no testable Palette-safe
pane-text regions. `mandatum-scene` now resolves `FloatingRect::default()`
through its existing clamp at both 80x24 and a small 6x3 viewport, and the
adapter consumes that result. A real-host long-path regression proves Empty
detail wraps through the Palette rows while all scene-cell pane-body fragments
stay outside its opaque area. The later aggregate review showed that this did
not prove final fractional-pixel bounds; the correction below supplies that
proof. A cold-review negative test proves an
altered Palette rectangle remains rejected, and a cold-recheck regression at
9x5 proves pane-title glyphs are also removed from the Palette area.
`./ci/gpu-spike.sh` passed 39 tests (two native-shell, seventeen real-host, and
twenty isolated-renderer) plus the boundary scan; all 35 scene tests and all
248 app library tests passed. A
displayed 800x632 macOS smoke drove the exact Palette transition and default
float from the same long-path missing-shell route; screenshot inspection showed
no underlying-glyph leakage at that observed scale, Ctrl+Q exited 0, and no
native or attempted-shell process remained. No additional scene shape or
production surface was admitted.

Aggregate-review correction verification (2026-07-23): focused RED first
failed because final fractional-pixel pane-body bounds and usable-interior
admission did not exist. The renderer now converts the complete pane body to
pixel `TextBounds`, converts every later-float/current opaque-overlay surface
with outward rounding, and subtracts in pixel space before submitting glyphs.
Fractional-cell-width regressions prove every returned body bound is disjoint
from each opaque surface, including the real-host long-path Palette transition.
Header and status glyphs use the same overlay subtraction; a 3x3 full-frame
overlay regression leaves neither chrome region visible. Every admitted
multi-pane rectangle must be at least 3x3 cells. Real-host resize tests accept
default horizontal at 6x5, vertical at 3x8, and float at 11x9, then reject each
immediately smaller width or height. The scene-only 6x3 resolver result remains
`(5, 1, 1, 1)`, but renderer admission correctly rejects it.
`./ci/gpu-spike.sh` passes 50 tests (two native-shell, twenty real-host, and
twenty-eight isolated-renderer) plus the renderer boundary scan; all 35 scene
tests and all 248 app library tests pass. Checked maximum-dimension
right/bottom endpoint regressions reject malformed panes whose true edge would
overflow `u16`. A visible 800x632 release smoke drove the long-path Palette
transition and default float; screenshots showed no leakage at the observed
scale, Ctrl+Q exited cleanly, and no native or attempted-shell process remained.

Capability-family layout/composition verification (2026-07-23): focused
real-host tracers first failed on a two-pane stack (`Layout("stacked panes")`)
and three tiled panes (`PaneCount(3)`). A dynamic buffer-pool test first failed
to compile because no pane pool existed. The final compiler retains the public
`prepare_scene(&WorkspaceScene, &Theme)` seam while removing topology
predicates. It validates usable bordered interiors, checked endpoints,
workspace containment, and a 256-pane aggregate renderer ceiling; layout
identity, flags, overlap, and draw order remain scene-owned.

At this family's original stop point, the displayed renderer grew title/body
glyph buffers with the scene and clipped them against later opaque panes and
the current overlay. Aggregate review corrected zero-pane preparation and
non-floating overlap. The subsequent content/style family below replaced those
per-pane paint resources with one final-topmost cell program.

`./ci/gpu-spike.sh` passed 48 tests (two native-shell, twenty-two real-host, and
twenty-four isolated-renderer) plus the renderer dependency-boundary scan.
Displayed release verification used one missing-shell session: it progressed
from one pane to three tiled panes, stacked the first split while preserving
the durable three-pane header count, added two overlapping floats, and opened
Help over the five-pane composition. Screenshot inspection showed distinct
three-pane buffers, correct stack representation, opaque later-pane
composition, and no underlying text through the Help surface. Ctrl+Q exited 0
and no native-spike process remained.

Content/style capability-family verification (2026-07-23): focused
RED/GREEN tracers established one renderer-neutral `CellProgram`, preserving
the existing `SceneCell` contract while making glyph/wide-continuation
occupancy, complete style, selection kind, and cursor explicit. The compiler
owns terminal, task, agent, Empty, header/status chrome, pane border/title, and
all eight overlay presentation rules. It applies scene paint order while
compiling and retains only final topmost cells in deterministic row-major
order, so memory is bounded by frame coverage even with many overlapping
panes.

Both the shipped ratatui renderer and excluded GPU renderer now translate that
same program. The GPU path maps ANSI/indexed/RGB colors, built-in and custom
semantic roles, bold, dim, italic, underline, inverse, hidden,
strikethrough, terminal/item selection, cursor, and opaque replacement into
background quads and styled glyph rows. `PreparedScene` contains no
content-specific pane or overlay shadow plan.

Aggregate review removed the obsolete ratatui modules, collapsed contradictory
selection state, clipped huge/off-frame and degenerate-border paint, moved
final-cell compaction into the shared compiler, added checked GPU pane/frame/
paint-work/row-buffer ceilings, strengthened real-host tests against actual
compiled content, and added warnings-denied all-target clippy to the spike
gate. The final automated matrix passed 45 scene tests, 28 ratatui renderer
tests, and 34 GPU-spike tests (two shell, twenty-three real-host, nine isolated
GPU) plus formatting, clippy, and the renderer dependency-boundary scan.

The final displayed 800x632 release matrix used custom `mandatum-light` roles
and showed the real Empty fallback, successful task output with 256-color
foreground/background plus bold/italic/underline/strikethrough, a fake agent
waiting for approval, and an opaque Palette with custom selection and surface
roles. No covered text leaked through. Escape closed the overlay, Ctrl+Q exited
0, and no native-spike process remained. The later Phase 3B matrix completed
input/lifecycle parity; true grapheme/wide-cell production, IME, Artifact
Preview, production admission, and rollout remain separate.

Phase 3B input/lifecycle verification (2026-07-23): the opt-in GPU gate passed
39 substantive tests (five native shell, twenty-five real-host, nine isolated
renderer) plus formatting, warnings-denied Clippy, and the renderer dependency
scan. The aggregate review corrected modifier/control aliases, native
clipboard precedence and visible failures, pointer drag/capture/focus
cancellation, wheel axes, stale/rejected hit targets, tiny-window suspension,
float shrink/restore containment, scale argument validation, and shutdown
ordering. The final confidence-70-or-higher cold read was clean.

The displayed macOS release matrix used the real host and PTYs in a disposable
project. Keyboard-only Palette/Help, native paste, pointer selection plus
Cmd+C, two-pane creation/save, full-screen resize, focus/minimize recovery,
Ctrl+Q cleanup, and two-PTY startup restore all passed. A bounded scale tracer
ran the exact runtime scale transition, visibly recomputed the grid from 88x30
to 57x20, presented 16 frames, logged `scale_probe_applied=true`, and exited 0.
The standing terminal probe measured p50 11.77 ms / p95 14.68 ms / max
18.56 ms over 100 key-to-app-output samples with zero misses; idle CPU advanced
0.10 s over 30 seconds. This one-display Mac did not prove cross-monitor
movement. Full commands, endpoints, and remaining boundaries are recorded in
`docs/verification.md`. The synchronized post-documentation `./ci/gate.sh`
passed all workspace tests, conformance, the app input-seam scan, and
documentation trace.

## Verdict (read this first)

The 2026-07-09 GPU run showed a **measured, roughly 2x latency advantage** over
the then-current ratatui frontend. Its rendering runs **entirely through the
`mandatum-scene` contract** (the renderer imports zero parser types), so it is
a clean adapter rather than a parallel path.

- GPU frontend, key -> GPU present, **includes the on-screen paint**:
  **p50 21.6 ms / p95 22.2 ms**.
- Then-current ratatui frontend, key -> app-output-bytes, measured externally,
  **excludes host-terminal paint**: **p50 42.9 ms / p95 45.8 ms**.

In that historical comparison, the GPU number is both lower and stricter (it
counts pixels on screen; the TUI number stops at bytes emitted, before the host
terminal paints them). Under a sustained scroll flood the GPU frontend held
~40 fps (25 ms/frame, p50≈p95), a floor set by an unoptimized per-frame rebuild,
not a ceiling.

The honest caveat, detailed in the side-by-side section: a large part of that gap
is the product's **40 ms input poll loop**, not "ratatui vs wgpu" as renderers.
A lower poll interval would shrink the TUI's bytes-out latency. The GPU
frontend's durable, non-replicable wins are vsync-timed frame pacing,
GPU-rasterized text, and owning the pixel pipeline end to end. The blunt
production call is in [Final spike verdict](#final-spike-verdict) at the bottom.

## Clean-adapter conformance (scene binding)

The renderer does not read a terminal grid. It consumes the `WorkspaceScene`
and `Theme` from the real host's `FrameSnapshot`; the product app alone performs
grid-to-scene conversion in `crates/app/src/scene_builder.rs`.

How the current boundary is enforced:

| Module | Product/runtime role |
|--------|----------------------|
| `mandatum-app::FrontendHost` | owns the only `AppState`, `RuntimeEngine`, PTY/parser path, command routing, recovery, and scene construction |
| `src/main.rs` | owns winit translation, `EventLoopProxy` wake binding, clipboard integration, heartbeat/redraw scheduling, and instrumentation |
| `gpu-renderer` + `src/gpu.rs` | paints only `WorkspaceScene` + `Theme`; its normal dependency tree cannot contain PTY or parser packages |
| `src/bin/tui_probe.rs` | external terminal latency harness; not workstation state |

`prepare_scene` is the window/GPU-free renderer seam used by controlled
integration tests and by the displayed renderer. It validates usable bordered
interiors, checked workspace containment, and explicit ceilings for panes,
frame cells, compiled cells, and retained row buffers. `mandatum-scene` then
compiles the real header, status, theme, every current pane content and overlay
type, and every ordered pane record into one `CellProgram` without recognizing
a topology. The scene owns pane geometry, identity, flags, overlap, order,
opacity, chrome, text, selection, cursor, and style. The displayed renderer
keeps the final topmost cell at each coordinate, paints background quads, and
shapes styled glyph rows; it has no content- or overlay-specific shadow plan.

The earlier `src/terminal.rs` and `src/scene_bridge.rs` architecture remains
relevant only to the historical 2026-07-09 benchmark evidence below. Both files
were deleted in Phase 2 along with the direct VT-parser dependency and duplicate
input/wake state.

Re-measured after the binding, there is **no regression**: typing-bench came back
p50 21.6 ms / p95 22.2 ms (identical to the pre-binding p50 21.6 ms), and the
scroll flood 24.8 ms / 26.3 ms per frame over 93 frames (identical to 25.0 ms).
Building an owned `WorkspaceScene` (a `Vec<Vec<SceneCell>>`) every frame is
absorbed within the existing frame budget.

## Side-by-side latency: GPU spike vs product ratatui frontend

The product frontend was measured **externally, with no edits outside
`spikes/`**: `src/bin/tui_probe.rs` spawns the real `mandatum` binary inside
a PTY at 100x30, waits for its first render, then for 100 iterations clears the
shell input line, types one character, and times until that character's echo
appears in the app's output byte stream.

| Path | What is timed | p50 | p95 |
|------|---------------|----:|----:|
| **GPU spike** | key receipt -> `queue.present` (**paint included**) | 21.6 ms | 22.2 ms |
| **ratatui frontend** | key -> app-emitted bytes (**host paint excluded**) | 42.9 ms | 45.8 ms |

Methodology and caveats, stated plainly because the comparison is asymmetric by
construction:

- **The measurements are not symmetric, and the asymmetry favors the TUI.** The
  GPU number ends at the GPU present (pixels on screen). The TUI number ends at
  bytes leaving the app, *before* the host terminal (iTerm2/Terminal.app) parses
  and paints them, which adds its own input-to-photon latency (another poll +
  refresh). So the TUI's true on-screen latency is higher than 42.9 ms, and the
  real gap is wider than the table shows.
- **Much of the gap is the 40 ms poll loop, not the renderer.** The app's loop is
  `tick -> draw -> event::poll(40ms)`. A keystroke's echo surfaces on the next
  draw after the shell echo is read, so latency clusters just above one poll
  interval (hence the tight 43-46 ms band). This is an app-design choice, not
  fundamental to ratatui: a shorter poll or an event-driven loop would cut the
  TUI's bytes-out latency substantially. The GPU spike, by contrast, is
  event-driven (renders when PTY bytes arrive) and vsync-paced, so its latency is
  ~one refresh (16.6 ms) plus echo round-trip.
- **Both use the same engine.** Same `mandatum-pty`, same `mandatum-terminal-vt`,
  same shell. The difference measured is purely frontend + loop architecture.
- The probe detects the echo reliably because the app's ratatui diff only emits
  changed cells: the probe char (`z`, never a byte in the app's ANSI control
  output) appears in the output stream exactly when the app paints its echo. 100
  samples, 0 misses.

Reproduce: `cargo build -p mandatum-app --release` (in the workspace), then
`cargo run --release --bin tui_probe` (in the spike).

**Addendum (2026-07-09):** the poll-loop prediction above was confirmed. The
product's run loop is now event-driven (dedicated input thread, unified event
channel, ~8 ms redraw cap); the same probe measures **p50 13.3-13.5 ms /
p95 ~15 ms / max ~18 ms**. The standing regression procedure and the
before/after table live in `docs/verification.md`.

**Maintenance refresh (2026-07-14):** the live terminal probe measured
**p50 11.71 ms / p95 13.56 ms / max 17.84 ms**. This remains
key-to-app-output-bytes only;
host-terminal paint is excluded, so it is not evidence of sub-20 ms end-to-end
latency and does not trigger GPU productization.

**Phase 2 refresh (2026-07-22):** after the excluded native adapter moved onto
the real host, the live terminal probe measured **p50 11.39 ms / p95 12.56 ms /
max 13.69 ms** over 100 samples with zero misses. This is still the terminal
frontend's key-to-app-output endpoint, excludes host-terminal paint, and is not
a native key-to-present or production-admission measurement.

## Text stack chosen

| Crate | Version | Role |
|-------|---------|------|
| winit | 0.30 | native window + event loop (`ApplicationHandler`) |
| wgpu | 30 | GPU surface, device, quad pipeline, present |
| glyphon | 0.12 | GPU glyph atlas + text rendering (cosmic-text 0.19 + swash) |
| arboard | 3.6 | clipboard (copy/paste) |
| pollster | 0.4 | block on async adapter/device request |

glyphon 0.12 was the pin that fixed everything else: it requires `wgpu ^30`,
`winit ^0.30.12`, `cosmic-text ^0.19`, all current stable. Resolving this
compatibility matrix against the crates.io sparse index *before* writing code,
then compiling a minimal window+text hello to surface the exact API shapes, was
the single highest-leverage step. wgpu 30 and cosmic-text 0.19 had both moved
several APIs from what training data suggested (`get_current_texture` returns a
`CurrentSurfaceTexture` enum, `queue.present(frame)` replaced `frame.present()`,
pipeline layout arrays now hold `Option<&_>`, push constants became
`immediate_size`, `set_text`/`set_size` dropped their `font_system` argument,
`RequestAdapterOptions`/`SurfaceConfiguration` gained new required fields). All
were read straight from the vendored sources rather than guessed.

Text rendering is a hybrid: glyphon draws the foreground glyphs (built per frame
as cosmic-text rich-text color runs, one run per contiguous same-color span),
and a small hand-written instanced-quad wgpu pipeline draws everything glyphon
does not: per-cell background colors, the block cursor, the selection highlight,
and the status strip. Backgrounds/cursor/selection are solid quads under the
text; the glyphs composite on top with alpha blending.

## Historical measured numbers (2026-07-09)

The following results preserve the original duplicate-host feasibility run.
Its instrumentation method (also embedded in each JSON `notes` field): input is
timestamped at winit event receipt (real key, or the synthetic bench injection,
through the *same* input path); present is timestamped immediately after
`queue.submit` + `queue.present`. One pending input is correlated per PTY-driven
present, assuming FIFO shell echo ordering. `frame_ms` is the present-to-present
interval, filtered to drop idle gaps > 250 ms. Present mode is Fifo (vsync).

### 1. Typing bench: `--exit-after 12 --typing-bench`
300 synthetic keystrokes injected at 30/sec through the real input-handling path
(deterministic, not through the OS):

```json
{"input_to_present_ms":{"p50":21.64,"p95":22.34,"max":41.18},"frame_ms":{"p50":33.34,"p95":34.50},"frames":302,"notes":"...input_samples=302 frame_samples=302..."}
```

- **Input-to-present p50 21.6 ms, p95 22.3 ms, max 41.2 ms.** This is the
  headline latency: type a key, see it on screen in ~one vsync. Tight p50/p95
  spread means consistent, not occasionally-janky.
- `frame_ms` here (33.3 ms) is **not** a throughput number: with one render per
  echo at a 30/sec injection cadence, present-to-present is dominated by the
  33 ms inject interval, not GPU capability. The flood run is the real
  frame-time test.

### 2. Scroll flood: `--exit-after 14 --flood`
Programmatically runs `seq 1 200000` in the shell at startup and measures frame
time while ~1.28 MB streams and scrolls:

```json
{"input_to_present_ms":{"p50":0.00,"p95":0.00,"max":0.00},"frame_ms":{"p50":25.01,"p95":25.84},"frames":94,"notes":"...flood=true frame_samples=94..."}
```

- **Frame time p50 25.0 ms, p95 25.8 ms over 94 sustained frames (~40 fps).**
  The p50≈p95 gap is the important part: the scroll is *smooth*, with no dropped
  frames or stutter spikes across the whole flood.
- 40 fps rather than 60 fps is the cost of an intentionally naive renderer: it
  rebuilds the entire screen's rich-text spans and re-shapes every glyph every
  frame, with no damage tracking or cross-frame shaping cache. That is the
  obvious optimization target, and it means 25 ms/frame is a floor.
- Getting a *sustained* flood to measure at all required two real fixes: a
  bounded reader→render `sync_channel` (so the reader blocks when the frontend
  falls behind, back-pressuring the shell exactly like a real terminal instead
  of buffering the whole flood instantly), plus a per-frame byte cap on the
  parser feed (so one pump does not race the reader and swallow the entire flood
  in a single repaint). Both lived in the now-deleted `src/terminal.rs`; this
  paragraph is historical evidence, not the Phase 2 architecture.

### 3. Plain interactive: `--exit-after 6`
Live shell prompt, no bench: renders the prompt, sits idle, exits cleanly at the
deadline. Confirms the ordinary interactive path and clean shutdown (no hang, no
panic, exit 0).

## What works

- A real shell is spawned and owned by the host's existing `RuntimeEngine`;
  output reaches the GPU only through real runtime events and a real
  `FrameSnapshot` terminal surface.
- The host's coalesced callback wakes winit through `EventLoopProxy<UserEvent>`;
  the spike has no second wake latch and does not interval-poll for PTY output.
- Keyboard, pointer, wheel, resize, focus, and paste input cross into the host
  as neutral `InputEvent` values. Configured workspace chords have first
  refusal; the product encoder covers xterm baseline modifier, control, and
  F1-F24 families. Scrollback, selection, command routing, and quit behavior
  remain behind the host.
- Exact Cmd+V fallback reads arboard into `InputEvent::Paste`; exact Cmd+C
  fallback requests the app's selection copy; typed
  `FrontendEffect::SetClipboard` values drain back to arboard. Clipboard
  failures remain visible in the shared status strip.
- The real scene header, focused pane and chrome, status strip, Ctrl+P command
  palette, context menu, execution timeline, session map, objective prompt,
  session-output Search, Help, and Welcome render from scene/theme data. Escape
  closes modal overlays, dismisses the non-modal Welcome note, and Ctrl+Q
  performs the real host quit path.
- Real one-pane task scenes render scene-composed command/cwd/runtime metadata
  with tail-preserving one-row fitting plus the live task output surface below;
  real one-pane agent scenes render wrapped objective/status/action/approval/
  changed-file detail from the same scene contract.
- Real one-pane Empty scenes render the scene-composed cwd, restart generation,
  and no-live-grid detail without querying app or runtime state.
- Pointer motion becomes drag while a button is held; focus/geometry changes
  cancel workspace gestures, release child capture, and clear stale hit
  targets. Unpresentable frames suppress pointer input until a successful
  present.
- Window resize and scale changes recompute glyph metrics and pointer cells,
  then send a neutral resize to the host; the app/runtime owns PTY and parser
  resizing. Restored workspaces recreate every visible terminal runtime.
- ANSI color: 16-color, 256-color cube, grayscale ramp, and direct RGB, plus
  inverse (fg/bg swap). Rendered as GPU quad backgrounds + colored glyph runs.
- Deterministic instrumentation with `--typing-bench`, `--flood`,
  `--exit-after N`, and the bounded `--scale-after` / `--scale-factor`
  lifecycle tracer (JSON summary to stdout).
- Headless integration tests start a real PTY through `FrontendHost`, exercise
  supported pane and overlay routes through neutral input, and prepare the real
  product scenes through the renderer boundary.
- No-display / wedge safety: `EventLoop::new()`, window creation, and GPU
  adapter/device requests each fail to a clean JSON error line and a non-zero
  exit rather than hanging, and a watchdog thread hard-exits if the event loop
  ever fails to terminate within budget. (These paths are coded and reasoned
  about but not exercised here, since this Mac has a display.)

## What does not work / known limitations

- **The side-by-side is latency only, and asymmetric.** The A/B now exists (see
  the side-by-side section), but it compares key->present against
  key->bytes-out, not photons against photons, and it does not compare *text
  quality* (that still needs eyes on the window, below).
- **Naive per-frame rebuild.** Full-screen rich-text is reassembled and reshaped
  every frame. No damage tracking, no glyph-run cache. This is why the flood
  sits at 40 fps rather than 60.
- **Glyphs are grid-anchored, not terminal-font guaranteed.** Each grapheme is
  clipped to its declared one- or two-cell span at shared fractional pixel
  boundaries. A missing glyph may still use the configured font system's
  fallback, so exact aesthetics vary by installed fonts even though cell
  geometry remains bounded.
- **Latency correlation is heuristic.** One pending input is correlated per
  runtime-driven present under a FIFO-echo assumption. At 30/sec with sub-frame
  echo this is effectively 1:1, but non-PTY runtime events or batched echoes
  could misattribute a sample. Honest for a spike, not a profiler.
- **No headless GPU presentation.** The real host-to-render-plan path is covered
  without a window, but device/surface acquisition and present still require the
  displayed smoke.
- **IME coverage is platform-bounded.** Neutral preedit/commit/cancel, dead-key
  composition, caret placement, focus cancellation, and overlay routing are
  implemented. The displayed Mac had one active keyboard/input-source path, so
  the full locale/input-source matrix remains future platform qualification.

## What a production adapter would still need

- **Production admission for Artifact Preview.** The selected pixel-native
  capability now works in the excluded adapter, but its winit/wgpu dependency
  tree, device/surface recovery, packaging, and rollout are still unadmitted.
- **Damage tracking + shaping cache.** Rebuild only changed rows; cache shaped
  glyph runs across frames. This is the path from 40 to a comfortable 60+ fps and
  is where the GPU approach's real throughput advantage would show.
- **Multi-display support policy.** The runtime scale transition is exercised,
  but this one-display Mac cannot prove cross-monitor movement or define the
  eventual supported display matrix.
- **Robustness**: surface-lost/outdated reconfigure loop (currently skips the
  frame), GPU device-loss recovery, and native scheduling policy under mixed
  runtime-event floods.

## Reproduce

```sh
cargo test --manifest-path spikes/frontend-wgpu/Cargo.toml --test host_wake
./ci/gpu-spike.sh
cargo build --release --manifest-path spikes/frontend-wgpu/Cargo.toml \
  --bin mandatum-frontend-wgpu-spike
spikes/frontend-wgpu/target/release/mandatum-frontend-wgpu-spike --exit-after 120
```

For the displayed Phase 3 task/agent smoke, use neutral palette input to create
the task (`b`) or agent (`a`) pane alongside existing panes; zoom (`z`) is an
optional layout check, not an admission workaround. Confirm every mixed-content
pane paints, the task shows live output, the agent shows its objective/state
detail, and Ctrl+Q leaves no native-spike or child process. Source modules:
`src/main.rs` (winit translation, host ownership, wake/effect/heartbeat/redraw
scheduling, instrumentation),
`gpu-renderer` + `src/gpu.rs` (structurally isolated scene/theme-to-GPU paint),
`src/stats.rs` (percentiles), and `src/bin/tui_probe.rs` (external terminal
latency probe).

For the displayed Empty smoke, launch the release binary from a disposable
project with `SHELL` set to a nonexistent absolute path and `XDG_CONFIG_HOME`
set to an empty disposable directory. The real host's failed initial PTY spawn
must leave the one terminal intent visible as Empty content with cwd, restart
generation, and no-live-grid detail. Confirm Ctrl+Q leaves no native-spike or
attempted-shell process.

For the displayed context-menu smoke, use the same disposable missing-shell
launch, right-click inside the Empty pane body, and confirm the bordered menu
keeps every product label and chord hint visible with the first row selected.
Escape must close the menu, and Ctrl+Q must leave no native-spike or
attempted-shell process.

For the displayed timeline smoke, use the same writable disposable
missing-shell launch, press Ctrl+P then `/`, and confirm the real Empty pane and
product chrome remain beneath a centered bordered Timeline. The recorded
`show-timeline` dispatch must appear selected with its glyph and relative time;
the filter prompt and footer must paint. Type `show` to exercise the live query,
then Escape must close the overlay and Ctrl+Q must leave no native-spike or
attempted-shell process.

For the displayed session-map smoke, use the same disposable missing-shell
launch, press Ctrl+P then `m`, and confirm the real Empty pane and product chrome
remain beneath a centered bordered Sessions map. The active session heading and
selected focused `pane-1 terminal` row must paint with the focus glyph, `idle`
state, and footer contained inside the border. Escape must close the overlay,
and Ctrl+Q must leave no native-spike or attempted-shell process.

For the displayed objective-prompt smoke, use the same disposable missing-shell
launch, create and focus an agent with Ctrl+P then `a`, and open its prompt with
Ctrl+P then `p`. Confirm the mixed Empty/agent scene remains beneath the
centered bordered Set agent objective prompt and that its focused pane title,
configured objective input, block cursor, and footer paint inside the border.
Escape must close the prompt, and Ctrl+Q must leave no native-spike or
attempted-shell process.

For the displayed Search smoke, use the same writable disposable missing-shell
launch, create an agent with Ctrl+P then `a`, and optionally zoom it with
Ctrl+P then `z`. Open Search with Ctrl+Shift+F, paste `kind:timeline search`,
and confirm the mixed scene remains around a centered opaque Search modal
without base-pane glyph leakage. The title, query and block cursor, grouped
timeline source, selected result, repeated-source elision, and footer must
remain inside the border. Escape must close Search, and Ctrl+Q must exit 0
without leaving a native-spike or attempted-shell process.

For the displayed Help smoke, use the same writable disposable missing-shell
launch, press F1, and type a filter retaining `Search session output`. Confirm
the real Empty pane remains around a centered opaque Help modal without
base-pane glyph leakage. The title, live query and block cursor, App heading,
Search command label, configured `ctrl+shift+f` route, selected row, and footer
must remain inside the border. Escape must close Help, and Ctrl+Q must exit 0
without leaving a native-spike or attempted-shell process.

For the displayed Welcome smoke, use a writable disposable project with no
workspace file and a harness that enables startup restore on the real
`FrontendHost` while consuming the exact local GPU renderer. Confirm the real
Empty pane remains around the centered opaque Welcome card without base-pane
glyph leakage. The title, introduction, ordered generated key routes and
descriptions, and dismissal must paint inside the border. Escape must dismiss
the non-modal note without quitting, and focused Ctrl+Q must exit 0 without
leaving a harness or native-spike process.

For the displayed two-horizontal-Empty-pane smoke, launch the release native
shell from a writable disposable project with an intentionally missing shell.
Press Ctrl+P then `v`. Confirm the header reports `2 pane(s)`, the workspace is
split into equal left/right panes, both panes show the scene-owned Empty cwd,
restart generation, and no-live-grid detail, and the right `terminal 2` title
has focus styling. Stop the disposable process and confirm no native-spike
process remains.

For the displayed two-vertical-Empty-pane smoke, use the same writable
disposable missing-shell launch and press Ctrl+P then `s`. Confirm the header
reports `2 pane(s)`, the workspace is split into equal top/bottom panes, both
panes show the scene-owned Empty cwd, restart generation, and no-live-grid
detail, and the lower `terminal 2` title has focus styling. Ctrl+Q must exit
cleanly with no native-spike or attempted-shell process remaining.

For the displayed two-pane-floating-Empty smoke, use the same writable
disposable missing-shell launch. Press Ctrl+P then `v`, followed by Ctrl+P then
`f`. Confirm the header reports `2 pane(s)`, `terminal` fills the workspace
behind a bordered focused `terminal 2` float, and both panes paint their
scene-owned Empty detail. Ctrl+Q must exit cleanly with no native-spike or
attempted-shell process remaining.

## Final spike verdict

**The 2026-07-09 GPU run proved a real, measured, user-visible latency win and a
clean adapter. The decision was still to ship the ratatui terminal frontend as
v1 and hold the wgpu adapter as a maintained option.**

In that historical comparison, the GPU path measured p50 21.6 ms including
on-screen paint, while the then-current terminal path measured p50 42.9 ms only
to bytes-out. The GPU path was event-driven and vsync-paced, composited the cell
backgrounds, selection, and cursor, and rasterized antialiased glyphs. It also
rendered through `mandatum-scene`, proving a second frontend could stay behind
the shared adapter boundary.

The comparison also exposed why the adapter did not ship. Much of the gap came
from the terminal frontend's then-current 40 ms poll loop, which was later
removed without GPU work. Phase 2 subsequently replaced the duplicate spike
host with the real `FrontendHost` and completed the header, one-terminal,
status, palette, neutral-input, wake, and typed-effect slice. Phase 3 is
complete: its layout/composition family was superseded by one compiler over the
complete ordered pane vector; its content/style family compiles every pane,
chrome, and overlay surface into one renderer-neutral cell program shared by
the ratatui and GPU adapters; and its input/lifecycle family covers native
key/modifier translation, clipboard, pointer, scrollback, focus, resize/scale,
restore, and shutdown through the real host. Phase 4 adds the bounded typed
Artifact Preview surface, safe app loader, terminal fallback, and GPU
contain-fit/cache path. A production wgpu adapter still needs correct advanced
grapheme and IME behavior, surface/device recovery, damage tracking, dependency
admission, and release integration.
Those costs become decisive only when the product needs true GPU visuals,
per-frame animation, pixel-precise layout, embedded non-text surfaces, or adopts
a sub-20 ms end-to-end target. The later Artifact Preview decision selected the
capability branch and Phase 4 now proves that surface in the excluded adapter;
it still does not admit production GPU dependencies. The Phase 2 terminal
refresh is p50 11.39 ms to bytes-out and remains incomparable to
key-to-present.

## Phase 4 Artifact Preview Verification (2026-07-23)

- The real host opens a project-relative PNG through the fuzzy palette/prompt,
  wakes on bounded decode completion, reaches typed RGBA8 scene data, and
  prepares the same GPU plan the displayed renderer consumes. Restart Pane
  forces a new revision after rewrite.
- Safe app-side loading rejects traversal, every symlink component, descriptor
  swap races, non-regular/missing/malformed/animated/oversized sources,
  dimensions above 4096×4096, aggregate decoded RGBA above 64 MiB, more than
  four concurrent workers, and more than 64 artifact panes/open descriptors.
- The GPU preflight independently checks surface length/dimensions and the
  64 MiB aggregate. Contain-fit, fractional scissor boundaries, later-pane/
  overlay occlusion, all-stale cache eviction, and revision replacement have
  focused tests.
- `./ci/gpu-spike.sh` passed 46 substantive tests: five native shell,
  twenty-six real-host, and fifteen isolated renderer tests, plus formatting,
  warnings-denied all-target Clippy, and the renderer-boundary scan.
- The displayed release matrix painted 600×300 landscape and reloaded 300×600
  portrait PNGs contain-fit, covered them with generated Help without bleed,
  preserved fit across 88×30 to 380×72 full-screen resize, rendered a calm
  missing-file failure, and exited cleanly through Ctrl+Q.
- Three independent reviewers plus a final cold read drove containment,
  aggregate-resource, animation, reload, cache-high-water, header-parsing,
  descriptor-cap, and restore-reservation fixes. The final confidence-70+
  review was clean.

This completes the selected pixel-native capability, not production
admission. Phase 5 advanced text/IME is complete; device/surface recovery,
multi-display proof, structured measurement/soak, production dependencies,
packaging, and rollout remain.

## Phase 5 Advanced Text And IME Verification (2026-07-23)

- Terminal cells, scene cells, and the final cell program now carry bounded
  extended grapheme strings plus explicit wide continuations. Writes, erases,
  edits, resize, copy, search scalar-range snapping, selection, cursor, wrapping,
  truncation, and both adapters preserve the same display-width contract.
- The GPU plan owns one buffer per visible grapheme, anchors it to exact cell
  coordinates, retains decorated spaces, clips glyphs to declared spans, and
  uses shared fractional pixel boundaries so adjacent spans never overlap.
- `InputEvent::Composition` carries preedit with a validated UTF-8 range,
  one-shot commit, and cancel. `AppState` locks composition to the active
  terminal, prompt, palette, search, timeline, or Help target; modal, pointer,
  paste, key, focus, and shutdown transitions cancel without leaking text.
- The winit shell translates platform IME events only while focused and
  allowed, supplies the candidate/caret rectangle, treats multi-scalar
  characters as one commit, keeps left Option for native dead keys, and uses
  right Option as terminal Meta. Font family, size, and runtime scale are
  bounded native-only settings.
- Three independent correctness, boundary/security, and acceptance reviews
  drove fixes for late-commit ordering, unfocused IME re-enable, public-scene
  validation, buffer admission, copy/search/wrap/scrollback wide edges,
  placeholder clearing, decorated spaces, glyph overhang, attention geometry,
  and fractional span overlap. All three final reruns returned no finding.
- The current-code displayed macOS matrix showed terminal and Command Palette
  preedit, a single committed `é`, focus-loss cancellation followed by plain
  `e`, mixed `A界é👩‍💻Z` output, Menlo 16 at runtime scale 1.25, resize from
  66×23 to 99×29, and clean Ctrl+Q exit. The one-display host did not claim a
  cross-monitor or full installed-input-source matrix.
- The standing terminal probe measured p50 14.58 ms / p95 16.67 ms / max
  18.28 ms over 100 samples with zero misses. A clean 30-second idle window
  advanced CPU time by 0.28 seconds (about 0.93% of one core).

Phase 6 surface/device recovery, explicit failure modes, resize/scale storms,
structured symmetric measurement, and soak evidence is next. The spike remains
excluded from production dependencies, packaging, and release.


## Correction note (2026-07-10)

The original typing-bench headline (max 23.1 ms over 300 samples) disagrees
with the raw run JSON recorded later in this file, which reports
{"p50":21.64,"p95":22.34,"max":41.18} over 302 samples. The p50/p95
figures agree across both; the max and sample count do not, and no second
bench JSON exists here to source the 23.1 ms figure. Summaries above and
downstream docs therefore cite p50/p95 only; this note preserves the disputed
original values rather than presenting them as settled evidence.
