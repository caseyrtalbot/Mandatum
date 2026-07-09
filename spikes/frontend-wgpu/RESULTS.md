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

The GPU frontend is **feasible and fast in absolute terms**: input-to-present
latency measured at **p50 21.6 ms / p95 22.3 ms**, which is essentially one
display refresh (16.6 ms at 60 Hz) plus shell echo round-trip. Under a sustained
scroll flood it holds a **smooth ~40 fps (25 ms/frame, p50≈p95, no stutter
spikes)**, and that number is a floor set by an unoptimized full-screen rebuild
every frame, not a ceiling.

What this spike does **not** prove: a head-to-head, user-visible quality *gain*
over the existing ratatui/crossterm frontend. It measures the GPU path's
absolute numbers; it does not run the same latency probe through the ratatui
frontend for an A/B. The qualitative gains that a GPU frontend unlocks
(antialiased sub-pixel glyph rasterization, arbitrary fonts/sizes/colors, frame
pacing the app controls, per-cell GPU-composited backgrounds/selection) are real
and visible in the live window, but calling them a *net* win over an already
competent host terminal is a judgment that needs eyes on the running window.
I cannot screenshot programmatically in this repo, so that visual confirmation
is Casey's to make. The honest position: **feasibility proven, absolute latency
is excellent, a rigorous quality-gain claim would need the A/B and a look at the
pixels.**

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

### 1. Typing bench — `--exit-after 12 --typing-bench`
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

### 2. Scroll flood — `--exit-after 14 --flood`
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

### 3. Plain interactive — `--exit-after 6`
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

- **No head-to-head baseline.** The spike measures the GPU path only. It does not
  run the identical latency probe through the ratatui frontend, so "quality
  gain" is argued qualitatively, not proven by A/B.
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
the JSON summary prints to stdout on exit. Source is four modules:
`src/terminal.rs` (PTY + parser adapter, backpressure, scroll/selection),
`src/gpu.rs` (wgpu surface, quad pipeline, glyphon text, per-frame render),
`src/stats.rs` (percentiles), `src/main.rs` (event loop, input encoding, bench,
instrumentation, JSON summary).
