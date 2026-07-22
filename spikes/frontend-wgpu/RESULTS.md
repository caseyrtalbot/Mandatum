# Frontend spike: winit + wgpu GPU terminal frontend

Status: **Phase 3 underway; task, agent, and Empty one-pane content are covered.**
A native macOS window drives `mandatum_app::FrontendHost` and its real
`RuntimeEngine`, translates winit events to neutral `InputEvent` values, and
renders the host's real header, one terminal, task, agent, or Empty pane, status
strip, and command palette on the GPU. Typed clipboard effects return to the
native shell.

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

`prepare_scene` is the window/GPU-free renderer seam used by the controlled
integration test and by the displayed renderer. It accepts the real header,
one terminal, task, agent, or Empty pane, status, theme, and optional palette
while explicitly rejecting multiple panes and unsupported overlays. The
displayed renderer uses the scene's pane-inner geometry, chrome, terminal/task
surface, scene-composed detail lines, status, and palette data rather than
deriving product presentation itself.

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
- Keyboard, pointer, wheel, resize, focus, and paste input cross into the host as
  neutral `InputEvent` values. Product key-to-byte encoding, scrollback,
  selection, command routing, and quit behavior remain behind the host.
- Cmd+V reads arboard into `InputEvent::Paste`; typed
  `FrontendEffect::SetClipboard` values are drained back to arboard.
- The real scene header, focused terminal pane and chrome, status strip, and
  Ctrl+P command palette render from scene/theme data. Escape closes the real
  palette and Ctrl+Q performs the real host quit path.
- Real one-pane task scenes render scene-composed command/cwd/runtime metadata
  with tail-preserving one-row fitting plus the live task output surface below;
  real one-pane agent scenes render wrapped objective/status/action/approval/
  changed-file detail from the same scene contract.
- Window resize and scale changes recompute scene cell dimensions and send a
  neutral resize to the host; the app/runtime owns PTY and parser resizing.
- ANSI color: 16-color, 256-color cube, grayscale ramp, and direct RGB, plus
  inverse (fg/bg swap). Rendered as GPU quad backgrounds + colored glyph runs.
- Deterministic instrumentation with `--typing-bench`, `--flood`, and
  `--exit-after N` (JSON summary to stdout).
- A headless integration test starts a real PTY through `FrontendHost`, blocks
  on the injected wake callback, drains runtime events, and prepares the real
  terminal scene through the renderer boundary.
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
- **Monospace alignment is font-metric based, not grid-snapped per glyph.** Cell
  width is measured once from a shaped run and columns are laid out by
  cosmic-text line layout. It holds for standard monospace ASCII; wide
  characters (CJK, emoji) and zero-width/combining marks are not width-corrected,
  so a line mixing widths can drift from the background quad grid.
- **No IME / dead keys / composition.** Input uses `KeyEvent.logical_key` and
  named keys directly; there is no `Ime` event handling.
- **Latency correlation is heuristic.** One pending input is correlated per
  runtime-driven present under a FIFO-echo assumption. At 30/sec with sub-frame
  echo this is effectively 1:1, but non-PTY runtime events or batched echoes
  could misattribute a sample. Honest for a spike, not a profiler.
- **No headless GPU presentation.** The real host-to-render-plan path is covered
  without a window, but device/surface acquisition and present still require the
  displayed smoke.
- **Bold/dim/italic/underline are mostly ignored** in rendering (the style bits
  are read; only inverse is honored). Colors and inverse render; weight/slant do
  not yet map to font attributes.

## What a production adapter would still need

- **Complete broader scene parity.** Header, one terminal/task/agent pane,
  Empty fallback, status, theme, and command palette are bound. Production still
  needs restore, multiple panes, hit-target parity, and the remaining overlay
  variants.
- **Damage tracking + shaping cache.** Rebuild only changed rows; cache shaped
  glyph runs across frames. This is the path from 40 to a comfortable 60+ fps and
  is where the GPU approach's real throughput advantage would show.
- **Correct wide-character and grapheme handling.** Unicode width, combining
  marks, and per-cell placement so the glyph grid and background grid never
  diverge.
- **IME / composition, dead keys, and full modifier semantics** (Alt-as-Meta,
  bracketed paste, mouse reporting passthrough when apps request it).
- **DPI / scale-factor changes at runtime** (multi-monitor drag). The scale hook
  exists and recomputes font metrics, but was only exercised at the initial
  scale.
- **Full SGR rendering**: bold/dim via font weight, italic via slant, underline
  and strikethrough via glyphon decorations (the style bits are already carried
  through from the parser).
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
the task (`b`) or agent (`a`) pane and zoom it (`z`) before the next redraw;
multi-pane paint remains deliberately unsupported. Confirm the task shows live
output, the agent shows its objective/state detail, and Ctrl+Q leaves no
native-spike or child process. Source modules: `src/main.rs` (winit translation,
host ownership, wake/effect/heartbeat/redraw scheduling, instrumentation),
`gpu-renderer` + `src/gpu.rs` (structurally isolated scene/theme-to-GPU paint),
`src/stats.rs` (percentiles), and `src/bin/tui_probe.rs` (external terminal
latency probe).

For the displayed Empty smoke, launch the release binary from a disposable
project with `SHELL` set to a nonexistent absolute path and `XDG_CONFIG_HOME`
set to an empty disposable directory. The real host's failed initial PTY spawn
must leave the one terminal intent visible as Empty content with cwd, restart
generation, and no-live-grid detail. Confirm Ctrl+Q leaves no native-spike or
attempted-shell process.

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
status, palette, neutral-input, wake, and typed-effect slice. Phase 3 is now
underway: its first increments add real one-pane task metadata/live output,
agent detail, and the Empty fallback without changing the scene or host
contract. A production wgpu adapter still needs restore, multi-pane and broader
scene parity, correct grapheme width, IME and composition, runtime DPI, full
style mapping, surface-loss recovery, and damage tracking.
Those costs become decisive only when the product needs true GPU visuals,
per-frame animation, pixel-precise layout, embedded non-text surfaces, or adopts
a sub-20 ms end-to-end target. The later Artifact Preview decision selects the
capability branch; Phase 2 still does not prove that surface or admit production
GPU dependencies. The Phase 2 terminal refresh is p50 11.39 ms to bytes-out and
remains incomparable to key-to-present.


## Correction note (2026-07-10)

The original typing-bench headline (max 23.1 ms over 300 samples) disagrees
with the raw run JSON recorded later in this file, which reports
{"p50":21.64,"p95":22.34,"max":41.18} over 302 samples. The p50/p95
figures agree across both; the max and sample count do not, and no second
bench JSON exists here to source the 23.1 ms figure. Summaries above and
downstream docs therefore cite p50/p95 only; this note preserves the disputed
original values rather than presenting them as settled evidence.
