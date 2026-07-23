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

A working wgpu adapter exists as an excluded spike (`spikes/frontend-wgpu`).
It now drives the real `FrontendHost`/`RuntimeEngine` and paints real scene
snapshots, but remains held outside the product workspace and release surfaces
(see the platform decision below).

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

The required feasibility spike ran and completed (2026-07-09). It initially
delivered a native window rendering a live PTY-backed terminal grid, typing and
paste, resize, scrollback, mouse selection and copy, a status strip, and
self-instrumenting latency/frame-time measurement. Phase 2 then replaced that
spike-local PTY/parser/input state machine with the product's real
`FrontendHost`: winit emits neutral `InputEvent` values, the host's coalesced
wake callback drives `EventLoopProxy`, typed clipboard effects return to the
native shell, and the GPU renderer paints the real header, one terminal, task,
agent, or Empty pane, status strip, command palette, context menu, and execution
timeline plus the session map, objective prompt, session-output Search,
generated Help and Welcome surfaces, and exactly two horizontally or vertically
tiled Empty panes plus the smallest default two-pane floating Empty layout from
`FrameSnapshot` scene/theme data. Phase 3 remains underway; stacked, broader
floating, dense, mixed-content, and three-plus-pane layouts, restore in the
excluded native shell, and broader input parity are still explicit gaps.

The adapter remains outside the Cargo workspace, product build, release
artifacts, and merge gate. The opt-in `./ci/gpu-spike.sh` maintenance check runs
spike-local format, locked all-target tests, and the renderer
dependency-boundary proof after scene-contract or spike changes. The paint path
is a separate spike-local crate that cannot depend on the PTY or terminal
parser. Full evidence:
[`spikes/frontend-wgpu/RESULTS.md`](../spikes/frontend-wgpu/RESULTS.md).

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
spike's renderer consumes only the `mandatum-scene` contract and imports zero
parser types. That paint boundary remained conforming through Phase 2, while
the enclosing native shell stopped owning PTY, parser, command-routing, or
product input behavior. The deleted `TerminalSession`, `scene_bridge`, duplicate
key encoder, and duplicate `AtomicBool` wake latch are now historical spike
implementation rather than current architecture.

### Verdict: the terminal frontend stays v1

Recorded in [`docs/decisions.md`](decisions.md#accepted-gpu-frontend-spike-verdict--terminal-frontend-stays-v1).
The spike
succeeded (a real, measured latency win and a clean adapter), but a large
share of the measured gap was the product's own 40 ms input poll loop, and
a production GPU adapter still owes substantial work the spike skipped
(multi-pane and broader-overlay scene binding, grapheme widths, IME, runtime DPI,
surface-loss recovery, damage tracking).

The poll-loop prediction was then confirmed: after the run loop became
event-driven (docs/decisions.md, "Event-Driven Main Loop With Heartbeat And
Redraw Cap"), the same external probe measured the terminal frontend at
**p50 13.30 ms / p95 15.04 ms / max 15.27 ms** key-to-bytes-out (procedure
and before/after table: docs/verification.md, "Input Latency Regression
Check"; addendum in RESULTS.md).

A 2026-07-14 live refresh measured **p50 11.71 ms / p95 13.56 ms / max
17.84 ms**, also key-to-bytes-out with host-terminal paint excluded. It
therefore does not prove sub-20 ms end-to-end latency. The authoritative dated
run and procedure live in [verification.md](verification.md).

The 2026-07-22 Phase 1C refresh, after all input, PTY, and agent producers
moved behind the coalesced app-owned sender, measured **p50 10.60 ms / p95
12.06 ms / max 13.38 ms** over 100 samples with zero misses. It has the same
key-to-app-output endpoint and therefore does not change the admission verdict.

The 2026-07-22 Phase 2 refresh, after the excluded native adapter moved onto the
real host, measured **p50 11.39 ms / p95 12.56 ms / max 13.69 ms** over 100
samples with zero misses. This remains the terminal frontend's
key-to-app-output measurement, excludes host-terminal paint, and is neither a
native input-to-photon result nor production-admission evidence.

The wgpu adapter stays warm behind the scene contract, with its probe
(`spikes/frontend-wgpu/src/bin/tui_probe.rs`) kept as the product's standing
latency-regression harness. Production admission remains gated on the selected
pixel-native capability (Artifact Preview), or on a separately accepted
sub-20 ms symmetric end-to-end latency goal; Phase 2 host integration alone is
not admission evidence.

The capability branch is now selected: an Artifact Preview Pane will display a
task- or agent-produced PNG as a typed pixel-native scene surface, while the
terminal frontend renders a deterministic labeled fallback card. The latency
branch is not selected. This product-trigger decision is not production
admission: the scene type and executable terminal/GPU adapter tests do not
exist yet, so the adapter remains unshipped and excluded from the product
workspace/build/release.

## Implementation Plan

The native frontend has a durable, admission-gated implementation sequence in
[native-gpu-implementation-plan.md](native-gpu-implementation-plan.md). It
keeps one `AppState`/`RuntimeEngine`, extracts a shared frontend host and typed
platform effects, migrates the terminal shell first, and only then connects the
excluded native adapter to real workstation state. Phases 1 and 2 are complete:
the terminal and excluded native shells now exercise the same host, runtime,
neutral input, scene, wake, and typed-effect boundaries. Phase 3 is underway:
scene-only increments cover real one-pane task and agent content, the Empty
fallback, context menu, execution timeline, session map, objective prompt, and
session-output Search plus generated Help and Welcome, followed by exactly two
horizontally or vertically tiled Empty panes and the smallest default two-pane
floating Empty layout. Restore in the excluded native shell, stacked, broader
floating, dense, mixed-content, and three-plus-pane layouts, and broader input
parity remain. Selecting the capability branch does not weaken the production
conformance gate, and Artifact Preview remains unbuilt.
