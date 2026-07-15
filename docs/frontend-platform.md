# Frontend Platform Strategy

## Purpose

Mandatum's product is not defined by a single frontend. The engine and scene
model should support multiple adapters so the product can serve local, remote,
terminal, native, and high-polish use cases.

## Frontend Roles

### Terminal Frontend (shipped, v1)

The ratatui adapter in `crates/renderer`, driven by `crates/app`. This is
the shipped v1 frontend (see the platform decision below).

Use for:

- fast development verification
- remote sessions
- SSH-friendly operation
- automation-friendly smoke tests
- fallback when no native frontend is available

Requirements:

- preserve child terminal input
- render terminal grids and pane chrome clearly
- support command palette
- support copy/scrollback baseline
- support task and agent panes
- remain lightweight and testable

### Native/GPU Frontend (proven option, not shipped)

A working wgpu adapter exists as a spike (`spikes/frontend-wgpu`); it is
held warm behind the scene contract, not shipped (see the platform decision
below).

Use for:

- smooth resizing
- high-quality text rendering
- precise pointer interaction
- low-latency scrollback
- richer selection behavior
- animation and visual polish
- accessibility integration
- platform-native menus and window behavior

Requirements:

- consume the same scene layer as the terminal frontend
- keep product behavior in engine modules
- expose input and hit-test events to runtime/command routing
- measure frame pacing, latency, memory, and CPU
- support automated smoke verification

### Platform-Specific Frontend

Use when platform fit materially improves the product.

Examples:

- macOS-native windowing and text rendering
- platform clipboard, accessibility, and menu integration
- platform GPU APIs where they improve quality and performance

Requirements:

- product state remains in the shared engine
- platform code remains behind adapter interfaces
- build and verification steps are explicit
- cross-platform assumptions are not smuggled into `core`

## Decision Criteria

Evaluate frontend options against:

- startup time
- input latency
- resize latency
- scroll latency
- frame pacing under output
- text crispness and shaping
- glyph fallback
- color fidelity
- selection precision
- mouse/pointer support
- accessibility hooks
- crash recovery
- test automation
- packaging complexity

## The Spike (Done) And The Platform Decision

The required spike ran and completed (2026-07-09). A winit + wgpu + glyphon
frontend at `spikes/frontend-wgpu` delivered the full vertical slice: a native
window rendering a live PTY-backed terminal grid, typing and paste, resize,
scrollback, mouse selection and copy, a status strip, and self-instrumenting
latency/frame-time measurement. It remains outside the Cargo workspace, product
build, release artifacts, and merge gate. The opt-in `./ci/gpu-spike.sh`
maintenance check runs spike-local format, locked all-target tests, and the
renderer dependency-boundary proof after scene-contract or spike changes. The
paint path is a separate spike-local crate that cannot depend on the PTY or
terminal parser. Full evidence:
`spikes/frontend-wgpu/RESULTS.md`.

### Measured numbers (from RESULTS.md)

| Path | What is timed | p50 | p95 |
|------|---------------|----:|----:|
| GPU spike | key -> GPU present (paint included) | 21.6 ms | 22.2 ms |
| ratatui frontend, 40 ms poll loop (then-current) | key -> app-emitted bytes (host paint excluded) | 42.9 ms | 45.8 ms |

Max latency is omitted: RESULTS.md's original headline max (23.1 ms) disagrees
with the raw run JSON in the same file (41.2 ms), so only the figures
consistent across both are cited (see the correction note in RESULTS.md).

The comparison is asymmetric by construction and the asymmetry favors the
TUI (its number stops before the host terminal paints), so the measured
~2x gap understates the true end-to-end gap. Under a sustained scroll flood
the spike held ~40 fps (frame time p50 25.0 ms, p95 25.8 ms over 94 frames),
a floor set by an intentionally naive per-frame rebuild, not a ceiling. The
spike renders purely through the `mandatum-scene` contract (its renderer
module imports zero parser types), so it is a second conforming frontend,
not a parallel path.

### Verdict: the terminal frontend stays v1

Recorded in docs/decisions.md ("GPU Frontend Spike Verdict"). The spike
succeeded (a real, measured latency win and a clean adapter), but a large
share of the measured gap was the product's own 40 ms input poll loop, and
a production GPU adapter still owes substantial work the spike skipped
(multi-pane/overlay scene binding, grapheme widths, IME, runtime DPI,
surface-loss recovery, damage tracking).

The poll-loop prediction was then confirmed: after the run loop became
event-driven (docs/decisions.md, "Event-Driven Main Loop With Heartbeat And
Redraw Cap"), the same external probe measured the terminal frontend at
**p50 13.30 ms / p95 15.04 ms / max 15.27 ms** key-to-bytes-out (procedure
and before/after table: docs/verification.md, "Input Latency Regression
Check"; addendum in RESULTS.md).

A 2026-07-14 live refresh measured **p50 11.30 ms / p95 13.08 ms**, also
key-to-bytes-out with host-terminal paint excluded. It therefore does not prove
sub-20 ms end-to-end latency. Scene-contract compile drift in the excluded spike
was repaired the same day and its opt-in maintenance check passed.

The wgpu adapter stays warm behind the scene contract, with its probe
(`spikes/frontend-wgpu/src/bin/tui_probe.rs`) kept as the product's standing
latency-regression harness. Revisit when the roadmap needs GPU-only
capability (true GPU visuals, per-frame animation, pixel-precise layout,
embedded non-text surfaces) or sets sub-20 ms end-to-end latency as a
product goal. Neither trigger is currently met, so the adapter remains
unshipped and excluded from the product workspace/build/release.
