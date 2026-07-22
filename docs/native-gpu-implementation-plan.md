# Native GPU Frontend Implementation Plan

Status: capability branch accepted; Phase 1A complete; production GPU
admission pending (2026-07-21).

This document is the durable implementation plan for a native window and
GPU-backed renderer. It does not change the current product verdict: the
terminal frontend remains v1, and the GPU adapter remains an excluded spike
until the fail-closed admission conditions in `ci/conformance.sh` are met.

## Outcome

Build a native frontend that runs the real Mandatum workstation state machine,
not a second terminal demo:

```text
terminal shell ─┐
native shell ───┼─> FrontendHost ─> AppState ─> RuntimeEngine
headless tests ─┘                         │
                                         └─> WorkspaceScene ─> adapter paint
```

There is exactly one `AppState` and one `RuntimeEngine` per run. The native
shell owns platform lifecycle, window metrics, IME context, clipboard
integration, surface/device resources, and paint scheduling. It does not own
PTYs, parsers, command routing, approvals, persistence, recovery policy, or a
parallel product model.

## Current Baseline

| Concern | Verified state | Planning consequence |
|---|---|---|
| Product state | `RuntimeEngine` owns all live terminal, task, and agent state; `AppState` folds durable and presentation state. | Reuse this state machine unchanged. |
| Renderer contract | `WorkspaceScene` carries layout, pane surfaces, overlays, themes, and hit targets. | Extend the contract only for an admitted product capability, never for a renderer convenience. |
| Terminal frontend | `app_shell.rs` owns the crossterm lifecycle, input reader, event-driven wait loop, 250 ms heartbeat, and 8 ms redraw cap. | Extract a shared host seam and migrate the terminal frontend before adding a native product crate. |
| GPU spike | The excluded winit/wgpu spike proves window creation, GPU presentation, glyph paint, a live PTY, and scene-only renderer boundaries. It supports only a narrow terminal scene and duplicates runtime/input behavior. | Reuse the evidence and paint techniques; do not promote the spike's app, PTY, parser, input encoder, or scene bridge. |
| Clipboard | `AppState` emits FIFO `FrontendEffect::SetClipboard(String)` values; `app_shell.rs` alone maps them to OSC 52. | Phase 1A is complete and proves the first renderer-neutral platform effect. |
| Wake path | Runtime events wake the terminal loop through the unified channel; winit needs a platform wake signal. | Preserve `std::sync::mpsc` and add a coalesced frontend wake hook rather than an async runtime or polling loop. |
| Performance evidence | The spike measured key-to-GPU-present p50 21.6 ms / p95 22.2 ms. The terminal's 2026-07-14 refresh measured key-to-app-output p50 11.71 ms / p95 13.56 ms / max 17.84 ms. | These endpoints are asymmetric and do not prove a native product win or sub-20 ms input-to-present performance. |
| Admission | The Artifact Preview capability branch is selected, but its typed scene surface and adapter tests do not exist yet. | Host extraction may proceed; production GPU dependencies remain rejected until the later admission evidence and decision. |

The detailed evidence and standing procedures live in
[frontend-platform.md](frontend-platform.md),
[verification.md](verification.md), and the dated
[spike results](../spikes/frontend-wgpu/RESULTS.md).

## Architectural Rules

1. One state machine owns the workstation. A frontend may cache paint data but
   never invent product truth.
2. `WorkspaceScene` remains the only paint input. Renderers do not compute
   layout or read terminal-parser types.
3. Platform input becomes `mandatum_scene::input::InputEvent` before it reaches
   product logic.
4. Runtime identity, generation/token rejection, flow-credit backpressure,
   approval control, persistence, and shutdown ordering remain in the current
   app/runtime modules.
5. Frontend-specific output leaves the state machine as typed effects. The
   first required effect is `SetClipboard(String)`.
6. GPU resources, window handles, surface state, glyph atlases, DPI state, and
   IME composition are live frontend state and are never serialized.
7. The terminal frontend remains available for SSH, headless use, recovery,
   and unsupported native environments.
8. No production GPU dependency or release allowlist changes until the Phase 6
   production-admission decision and its evidence are accepted.

## Proposed Host Interface

The exact Rust spelling may sharpen during test-first extraction, but the
responsibility boundary is fixed:

```rust
struct FrontendHost {
    app: AppState,
}

struct FrontendUpdate {
    revision: u64,
    redraw: bool,
    quit: bool,
    next_deadline: Option<Instant>,
}

struct FrameSnapshot {
    scene: WorkspaceScene,
    theme: Theme,
    revision: u64,
}

enum FrontendEffect {
    SetClipboard(String),
}
```

The host accepts neutral input, drains accepted runtime events, performs the
heartbeat work, produces immutable frame snapshots, exposes typed effects,
and shuts down once. An app-owned event sender retains the current channel and
optionally invokes a coalesced platform waker after enqueueing. A waker is a
notification only; the channel remains the source of event truth.

## Phase 0 — Select And Record The Product Trigger

Result: **complete — capability branch selected.**

The first pixel-native capability is an **Artifact Preview Pane**: open a
task- or agent-produced PNG screenshot, diagram, chart, or visual diff as a
reviewable workspace pane without leaving Mandatum. This is the first concrete
step toward a useful workspace containing task, agent, and artifact panes with
no terminal pane required.

Planned typed contract and ownership:

- `mandatum-core` persists only `ArtifactPaneIntent`: a project-relative source
  path, title/alt text, and contain-fit mode.
- `crates/app` validates, decodes, bounds, reloads, and caches the artifact.
  Decoded bytes and file handles are live state, never durable intent.
- `mandatum-scene` carries typed artifact loading/ready/failed state and a
  bounded RGBA8 sRGB `RasterSurface` with pixel dimensions and revision.
- the terminal renderer paints a deterministic labeled fallback card;
  the native GPU renderer uploads the same surface as a texture.

First-slice guardrails: PNG only; project-relative local files; reject symlink
escapes, URLs, SVG, HTML, PDF, video, and animation; maximum encoded file
16 MiB, dimensions 4096×4096, decoded buffer 64 MiB; malformed or oversized
input becomes visible failed state; useful alt text is required or visibly
defaults to the filename.

Rollout boundary:

- macOS arm64 is the initial displayed development reference;
- `native` is explicit opt-in and the terminal frontend remains the default on
  all four current release targets;
- startup fallback may occur only before `AppState` and live runtimes exist;
- there is no transparent mid-session process switch;
- latency remains an observation, not the selected product trigger.

Phase 0 accepts the product direction only. It is not production GPU-admission
evidence. `ci/conformance.sh` remains unchanged in force until the typed scene
surface, terminal fallback test, excluded-GPU render-plan test, and later Phase
6 admission decision exist.

## Phase 1 — Extract The Frontend-Neutral Host

Dependency: accepted Phase 0 capability decision. This phase adds no winit,
wgpu, glyphon, or
other native/GPU production dependency.

### 1A. Neutral frontend effects — COMPLETE (2026-07-21)

- Added a typed `FrontendEffect` owned by the app.
- Replaced terminal-encoded clipboard payloads with a FIFO raw-text effect
  queue.
- Kept selection extraction, last-copied state, and status updates in
  `AppState`.
- Moved OSC 52 encoding and output fully behind the terminal shell.
- Tests prove FIFO ordering, drain-once behavior, restore clearing, both copy
  paths, and terminal-boundary encoding.

### 1B. Frame and lifecycle seam

- Add `crates/app/src/frontend_host.rs`.
- Move public run-loop operations behind product-shaped methods:
  `handle_input`, `drain_runtime`, `heartbeat`, `frame`, `take_effects`,
  `should_quit`, and `shutdown`.
- Return revision/redraw/deadline information so a frontend does not inspect
  `AppState` internals.
- Keep hit targets tied to the exact scene snapshot that was painted.

### 1C. Wake-aware event sender

- Wrap the unified app sender in an app-owned sender type.
- Preserve the current `std::sync::mpsc` event stream and runtime stamps.
- Add an optional coalesced wake callback suitable for winit's event-loop
  proxy.
- Prove PTY, agent, and input events wake a fake frontend without polling or
  lost wakeups.

### 1D. Terminal migration

- Rewrite `app_shell.rs` to drive `FrontendHost`.
- Preserve the 250 ms heartbeat, 8 ms redraw cap, input failure propagation,
  runtime shutdown, reader join, terminal restoration, and primary-error
  precedence.
- Add fake-host tests before any native frontend consumes the interface.

Exit gate:

- `./ci/gate.sh` is green.
- The terminal latency probe remains below its p50 25 ms regression bar.
- Tests prove input, PTY wake, agent wake, redraw coalescing, clipboard,
  quit, and error shutdown.
- No native or GPU dependency enters a production workspace member.

## Phase 2 — Prove One Real Native Workstation Slice

Dependency: Phase 1. Keep this work in `spikes/frontend-wgpu` and outside
product release surfaces.

- Instantiate `FrontendHost` instead of the spike's `TerminalSession`.
- Translate winit input directly to neutral `InputEvent` values.
- Wake the event loop through the host's wake-aware sender.
- Render the real scene header, one real terminal pane, status strip, and
  command-palette overlay.
- Remove the duplicate spike PTY/parser/input path after the real host path is
  proven.

User-visible proof:

1. Start the excluded native spike in a project directory.
2. Type `printf GPU_HOST_OK` and observe output from the real RuntimeEngine.
3. Open and close the real command palette.
4. Quit and verify no child or reader thread remains.

Exit gate:

- Exactly one AppState/RuntimeEngine owns behavior.
- `./ci/gate.sh` and `./ci/gpu-spike.sh` are green.
- A headless test paints a real host scene through the GPU renderer.
- A displayed native-window smoke passes on the reference Mac.
- The spike remains excluded from workspace and release artifacts.

## Phase 3 — Complete Scene And Input Parity

Dependency: Phase 2.

Render every current scene:

- tiled, stacked, floating, zoomed, and dense multi-pane layouts;
- terminal, task, agent, and empty pane content;
- every overlay, header attention segment, status surface, hit target, focus,
  selection, and cursor;
- all built-in and custom semantic theme roles;
- bold, dim, italic, underline, inverse, hidden, and strikethrough styles.

Complete input parity:

- workspace chords before terminal fallback;
- BackTab, Alt-as-Meta, paste, pointer capture/passthrough, scrollback,
  selection, focus, resize, and quit;
- keyboard-only completeness and reduced-motion behavior.

Exit gate: no `UnsupportedScene` result is reachable for a product-generated
scene, and semantic/golden tests cover every scene and input enum variant.

## Phase 4 — Make Text And IME Correct

Dependency: Phase 3.

- Define grapheme clusters, wide-cell continuation, combining-mark, fallback,
  cursor, and selection alignment in the scene contract.
- Preserve terminal-cell semantics rather than reshaping raw terminal output
  into proportional text.
- Add a neutral text/IME composition contract; composed text is not paste.
- Cover CJK, emoji/fallback, combining marks, dead keys, preedit, commit,
  cancellation, and runtime DPI changes.
- Define native font family/size/scale settings without adding inert settings
  to the terminal frontend.

Exit gate: fixtures and visual checks prove cell alignment, selection, cursor,
IME, and scaling across the supported platform matrix.

## Phase 5 — Harden And Measure

Dependency: feature parity.

Implement and test:

- surface outdated/lost reconfiguration;
- GPU device-loss recreation;
- explicit out-of-memory behavior;
- no-adapter and no-display startup results;
- multi-monitor scale changes and resize storms;
- bounded caches and damage tracking only where profiling justifies them;
- current PTY flow-credit backpressure rather than a parallel queue policy;
- structured measurement output containing platform, GPU, display refresh,
  workload, sample count, misses, and latency percentiles.

Proposed thresholds for the later admission decision, not current promises:

- at least three 1,000-sample symmetric runs;
- native p95 below 20 ms with zero misses and at least a 25% p95 improvement;
- flood frame p95 within one 60 Hz frame after warmup;
- 1,000 resize/scale changes without a blank or wedged surface;
- 30-minute flood/resize/input soak without crash or monotonic memory growth;
- idle below 1% of one CPU core;
- first usable frame within one second on the reference matrix.

Exit gate: every threshold actually accepted in Phase 0 passes. Historical
p50-only or bytes-out measurements cannot substitute for symmetric evidence.

## Phase 6 — Admit And Promote The Product Adapter

Dependency: accepted trigger plus Phase 5 evidence.

- Add a production frontend crate only now.
- Keep it dependent on the app host and scene contract, never concrete runtime
  registries or parser types.
- Replace the blanket GPU hold with a narrow package allowlist while retaining
  negative tests that reject GPU dependencies in all other production crates.
- Add a dedicated native/GPU verification script.
- Keep the adapter out of the ordinary merge gate until its headless checks are
  deterministic in CI.
- Do not change release archives or the installer in this phase.

Exit gate: admission decision, dependency-boundary negative tests, full gate,
native gate, parity checks, and evidence record are all green.

## Phase 7 — Roll Out Without Losing The Terminal

Dependency: production admission.

Introduce explicit frontend selection:

- `terminal` remains the default for the first experimental release;
- `native` fails clearly before runtime creation if the window/display/adapter
  is unavailable;
- `auto` may fall back only before `AppState` and live runtimes are created;
- unrecoverable mid-session GPU failure reports clearly and restarts from
  durable intent rather than pretending live PTYs can be serialized.

Update the release workflow, installer, update path, binary allowlists, archive
checks, distribution tests, and fresh-install/update smoke together. Either
prove every existing macOS/Linux release target or explicitly narrow the
accepted support matrix.

Native becomes the default only after complete scene/input/theme/accessibility
parity, accepted performance on the support matrix, device-loss recovery, a
native stranger test, release/update proof, and one experimental release
without a release-blocking regression.

## Verification Matrix

| Change | Required proof |
|---|---|
| Documentation or admission status | `./ci/gate.sh`, doc trace, decision/plan sync |
| Scene contract or excluded adapter | `./ci/gate.sh` and `./ci/gpu-spike.sh` |
| Host/run-loop/input/wake path | full gate plus `tui_probe` latency procedure |
| GPU paint or shaping | semantic/golden tests, headless paint, displayed smoke |
| DPI/surface/device recovery | deterministic fault tests plus resize/soak smoke |
| Product crate admission | full gate, native gate, dependency negative tests |
| Release surface | four-target or explicitly narrowed release smoke and update proof |

Every verification claim belongs in [verification.md](verification.md) with
the date, environment, command, endpoint, and result.

## Non-Goals

- Replacing or removing ratatui immediately.
- Moving product behavior into the renderer.
- Promoting the spike's duplicate terminal runtime.
- Serializing windows, GPU resources, atlases, or live handles.
- Adding a pixel-native scene type without a named capability.
- Adding damage tracking before profiling.
- Claiming Windows support without adding it to the release matrix.
- Building a separate Swift/AppKit/Metal product branch for the first native
  implementation.
- Seamless mid-session cross-process fallback.

## Next Implementation Slice

Implement Phase 1B only: add `crates/app/src/frontend_host.rs` around the
existing `AppState` lifecycle and frame operations, then make `app_shell.rs`
drive that host without changing the unified event channel, heartbeat, redraw
cap, shutdown ordering, or terminal behavior. Do not add a native dependency,
window, renderer, or platform waker in that slice.
