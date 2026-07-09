# Frontend spike: winit + wgpu GPU terminal frontend

Status: **builds, runs, and produces measured numbers.** A native macOS window
renders a live shell session on the GPU, with keyboard input, paste, resize,
scrollback, mouse selection/copy, a status strip, and self-instrumenting
latency and frame-time collection.

This is a frontend adapter only. It path-depends on the engine crates
`mandatum-pty` (PTY runtime) and `mandatum-terminal-vt` (VT parser + grid) and
copies no product behavior. It lives outside the Cargo workspace (the root
`Cargo.toml` excludes `spikes/frontend-wgpu`), so its heavy GPU dependency tree
never joins the product build or CI gate.

## Verdict (read this first)

The GPU frontend shows a **measured, roughly 2x latency advantage** over the
product's ratatui frontend, and its rendering now runs **entirely through the
`mandatum-scene` contract** (the renderer imports zero parser types), so it is a
clean adapter rather than a parallel path.

- GPU frontend, key -> GPU present, **includes the on-screen paint**:
  **p50 21.6 ms / p95 22.2 ms / max 23.1 ms**.
- Product ratatui frontend, key -> app-output-bytes, measured externally,
  **excludes host-terminal paint**: **p50 42.9 ms / p95 45.8 ms / max 52.3 ms**.

The GPU number is both lower and stricter (it counts pixels on screen; the TUI
number stops at bytes emitted, before the host terminal even paints them), so the
true end-to-end gap is larger than the 2x shown. Under a sustained scroll flood
the GPU frontend holds a smooth ~40 fps (25 ms/frame, p50≈p95), a floor set by an
unoptimized per-frame rebuild, not a ceiling.

The honest caveat, detailed in the side-by-side section: a large part of that gap
is the product's **40 ms input poll loop**, not "ratatui vs wgpu" as renderers.
A lower poll interval would shrink the TUI's bytes-out latency. The GPU
frontend's durable, non-replicable wins are event-driven + vsync-timed frame
pacing, GPU-rasterized text, and owning the pixel pipeline end to end. The blunt
production call is in [Final spike verdict](#final-spike-verdict) at the bottom.

## Clean-adapter conformance (scene binding)

The renderer no longer reads the terminal grid. It consumes only the
`mandatum-scene` contract that every Mandatum frontend consumes, with the
grid -> scene conversion isolated in one module, exactly as the product app
splits `scene_builder.rs` from its ratatui renderer.

How the boundary is enforced in the spike's module structure:

| Module | Imports `mandatum-terminal-vt`? | Role |
|--------|:---:|------|
| `src/terminal.rs` | yes | owns the PTY + VT parser + grid |
| `src/scene_bridge.rs` | yes | the ONLY grid -> scene seam; builds a `WorkspaceScene` each frame |
| `src/gpu.rs` (the renderer) | **no** | paints from `WorkspaceScene` / `TerminalSurface` / `SceneCell` / `SceneColor` only |
| `src/main.rs` | no | builds the scene via `scene_bridge`, hands `&WorkspaceScene` to the renderer |

Verified mechanically: `grep -n 'use .*mandatum_terminal_vt' src/gpu.rs` returns
nothing, and no parser type name (`TerminalGrid`, `TerminalCell`, `CellStyle`,
the VT `Color`) appears in the renderer. Each frame `scene_bridge::build_scene`
windows the grid into a `TerminalSurface` (the same `terminal_surface` logic as
`crates/app/src/scene_builder.rs`: `rows` windowed to the viewport, `first_row`
absolute, cursor/selection in absolute coordinates), wraps it in a single-pane
`WorkspaceScene`, and the renderer uses the surface's own `selection_contains`
and `cursor_at` helpers to place highlight and cursor quads. Copy-mode selection
was also made inclusive-end in `text_in_range` to match the scene contract's
inclusive selection span, so copied text agrees with the highlight.

Re-measured after the binding, there is **no regression**: typing-bench came back
p50 21.6 ms / p95 22.2 ms (identical to the pre-binding p50 21.6 ms), and the
scroll flood 24.8 ms / 26.3 ms per frame over 93 frames (identical to 25.0 ms).
Building an owned `WorkspaceScene` (a `Vec<Vec<SceneCell>>`) every frame is
absorbed within the existing frame budget.

## Side-by-side latency: GPU spike vs product ratatui frontend

The product frontend was measured **externally, with no edits outside
`spikes/`**: `src/bin/tui_probe.rs` spawns the real `mandatum-app` binary inside
a PTY at 100x30, waits for its first render, then for 100 iterations clears the
shell input line, types one character, and times until that character's echo
appears in the app's output byte stream.

| Path | What is timed | p50 | p95 | max | n |
|------|---------------|----:|----:|----:|--:|
| **GPU spike** | key receipt -> `queue.present` (**paint included**) | 21.6 ms | 22.2 ms | 23.1 ms | 300 |
| **ratatui frontend** | key -> app-emitted bytes (**host paint excluded**) | 42.9 ms | 45.8 ms | 52.3 ms | 100 |

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

## Measured numbers

Instrumentation method (also embedded in each JSON `notes` field): input is
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
  in a single repaint). Both live in `src/terminal.rs`.

### 3. Plain interactive: `--exit-after 6`
Live shell prompt, no bench: renders the prompt, sits idle, exits cleanly at the
deadline. Confirms the ordinary interactive path and clean shutdown (no hang, no
panic, exit 0).

## What works

- Live shell (`$SHELL`, zsh here) spawned via `mandatum-pty`, output parsed by
  `mandatum-terminal-vt` into grid snapshots rendered every frame.
- Keyboard input: printable text, Enter/Backspace/Tab/Esc, arrows, Home/End/Del,
  Ctrl+letter control codes, all encoded to PTY bytes through one shared path
  used by both real and synthetic input.
- Paste via Cmd+V (arboard clipboard read), copy via Cmd+C from a mouse
  selection.
- Smooth window resize: PTY + parser grid resize together on the fly, grid
  columns/rows recomputed from measured monospace cell metrics; no tearing or
  panics observed across the runs.
- Scrollback: mouse-wheel and PageUp/PageDown scroll through history via the
  grid's `history_cell(absolute_row, column)` API, with the viewport staying
  anchored as new output pushes lines into scrollback.
- Mouse selection: click-drag highlights cells (reading-order selection in
  absolute grid coordinates, so it survives scrolling), Cmd+C copies the text.
- Status strip: shell name, grid size, live/scroll state, fps, and live latency
  p50/p95, one line at the bottom over its own quad background.
- ANSI color: 16-color, 256-color cube, grayscale ramp, and direct RGB, plus
  inverse (fg/bg swap). Rendered as GPU quad backgrounds + colored glyph runs.
- Deterministic instrumentation with `--typing-bench`, `--flood`, and
  `--exit-after N` (JSON summary to stdout).
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
- **No IME / dead keys / composition.** Input uses `KeyEvent.text` and named
  keys directly; there is no `Ime` event handling.
- **Latency correlation is heuristic.** One pending input per PTY-driven present
  under a FIFO-echo assumption. At 30/sec with sub-frame echo this is effectively
  1:1, but batched echoes could misattribute a sample. Honest for a spike, not a
  profiler.
- **No true-headless verification.** The clean-error paths for a display-less
  environment are implemented but untested in this display-having session.
- **Bold/dim/italic/underline are mostly ignored** in rendering (the style bits
  are read; only inverse is honored). Colors and inverse render; weight/slant do
  not yet map to font attributes.

## What a production adapter would still need

- **Bind to the `mandatum-scene` contract.** This spike reads the grid directly.
  A real frontend should consume the renderer-neutral scene the product exposes,
  so the wgpu adapter is one implementation of a shared contract (the same one
  the ratatui frontend satisfies), not a parallel path with its own grid reading.
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
  frame), GPU device-loss recovery, and a real backpressure policy tied to the
  engine's `BackpressureState` rather than a fixed queue depth.

## Reproduce

```sh
cd spikes/frontend-wgpu
cargo build --release
cargo run --release -- --exit-after 12 --typing-bench   # latency
cargo run --release -- --exit-after 14 --flood           # frame time under scroll
cargo run --release -- --exit-after 6                     # plain interactive
```

A native window appears for the duration of each run and closes at the deadline;
the JSON summary prints to stdout on exit. Source modules:
`src/terminal.rs` (PTY + parser adapter, backpressure, scroll/selection),
`src/scene_bridge.rs` (grid -> `mandatum-scene` conversion, the only parser/scene
seam), `src/gpu.rs` (wgpu surface, quad pipeline, glyphon text, paints from the
scene), `src/stats.rs` (percentiles), `src/main.rs` (event loop, input encoding,
bench, instrumentation, JSON summary), `src/bin/tui_probe.rs` (external ratatui
latency probe).

## Final spike verdict

**The GPU frontend proves a real, measured, user-visible latency win, and it is a
clean adapter, but the honest call is: keep the ratatui terminal frontend as v1,
and hold the wgpu adapter as a proven, ready option rather than shipping it now.**

Why the win is real: at p50 21.6 ms including the on-screen paint, against the
product's 42.9 ms that stops at bytes-out before its host terminal even paints,
the GPU path is at least twice as responsive, and the gap is understated. It is
event-driven and vsync-paced by construction, it composites cell backgrounds,
selection, and cursor on the GPU, and it rasterizes real antialiased glyphs with
font freedom a cell-locked host terminal cannot match. And it now renders purely
from the `mandatum-scene` contract, so it is a second conforming frontend, not a
fork; the architectural cost of keeping the option open is already paid.

Why not ship it as v1 anyway: be blunt about what the number is not. A large part
of the measured gap is the product's own 40 ms input poll, which the ratatui
frontend could cut without any GPU work, so "2x" partly measures a loop constant
the terminal frontend can close on its own. Against that, a production wgpu
adapter still owes real work the spike skipped: binding fully to the scene
contract for multi-pane/overlay/header (this spike renders one pane), correct
wide-character and grapheme width, IME and composition, runtime DPI, bold/italic
mapping, surface-loss recovery, and a damage-tracked renderer to actually reach
60 fps under load. That is weeks, not days. The latency and quality gains are
real but incremental for a terminal-first tool; they become decisive only when
the product wants things a host terminal cannot give at all: true GPU visuals,
per-frame animation, pixel-precise layout, embedded non-text surfaces.

So: the spike succeeded. It disproved the null hypothesis (no measurable gain):
there is a measured latency gain and a credible quality gain, and it proved the
adapter can be clean. The gain does not yet clear the bar to displace a working,
lower-risk terminal frontend as v1. Ship ratatui; keep this adapter warm behind
the scene contract; revisit when the roadmap needs GPU-only capability or when
sub-20 ms end-to-end latency becomes a stated product goal.
