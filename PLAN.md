# Plan

`PLAN.md` points forward. Decisions and historical rationale live in
`docs/decisions.md`; standing procedures and dated evidence live in
`docs/verification.md`.

## Direction

Mandatum is a personal, GPU-native development environment with Ghostty-class
feel. The native wgpu frontend is the product. The terminal frontend is a
maintained tool for SSH, headless use, recovery, and an explicit escape hatch.

Daily-driver quality for Casey on known macOS hardware is the adoption bar.
There is no public-release audience and no Phase 7/8 admission ceremony.
Latency, idle, resize, recovery, and fault probes remain regression checks, not
permission gates.

The complete ordered plan is
[docs/native-gpu-implementation-plan.md](docs/native-gpu-implementation-plan.md).

## Current Baseline

The workstation already has the five constitutional boundaries, one
`AppState`/`RuntimeEngine`, the shared `FrontendHost`, one app-owned event
channel, renderer-neutral input/effects, scene-owned layout and presentation,
terminal parity through `CellProgram`, typed Artifact Preview pixels, shared
grapheme/IME contracts, native input and lifecycle routes, GPU recovery, and
measurement tooling. Native startup now completes window and GPU renderer
preflight before constructing `FrontendHost`, so failed preflight cannot start
restore or PTY work.

The native implementation still lives under `spikes/frontend-wgpu`; the root
workspace, `ci/conformance.sh`, `ci/gpu-spike.sh`, and default launcher still
reflect the retired posture. Those are explicit implementation gaps, not the
product direction.

## Ordered Work

### 1. Reorder native startup — complete

The native shell keeps `host: None` during preflight and creates the window,
surface, adapter, device, queue, and renderer before `FrontendHost`. Forced
no-display and no-adapter tests prove the host creation seam is never invoked;
the real macOS startup/clean-exit path and restore coverage are green.

### 2. Promote native into the workspace

Move the native shell and renderer into a production workspace package. Keep
lab and measurement tooling separate. Narrowly allow winit/wgpu/glyphon in the
native package, retain negative dependency checks everywhere else, rename the
native gate, and make `./ci/gate.sh` invoke it so CI retains one authority.
Terminal behavior and existing installer/release artifacts stay unchanged.

### 3. De-risk typography

Compare glyphon/cosmic-text side by side with Ghostty using Casey's font, size,
scale, theme, and display. Decide whether the stack can deliver delightful text
before investing deeply in the broader visual identity.

### 4. Add a bounded shaping cache

Memoize shaped buffers by grapheme, style, and metrics while preserving
per-grapheme clipping and cell-span invariants. Bound retained count/bytes,
invalidate by font/metrics/scale generation, and profile before considering
row-level damage tracking.

### 5. Make native the default and build feel

Casey daily-drives native with an explicit terminal escape hatch. Daily use
sets the hardening queue. Build the feel roadmap in this order: typography,
pane materials and hierarchy, spacing and density, focus treatment, fluid
resize, purposeful transitions, and richer artifact/workflow surfaces.

## Product Work After The Native Transition

- **Named task and dev-server recipes.** Add a project-local catalog for build,
  test, lint, and server recipes with duration, cwd, start time, port, and
  health facts.
- **Recovery cockpit.** Explain what restore recreated, intentionally detached,
  or needs an explicit rerun; allow resolved failures to be acknowledged.
- **Connector catalog and automation surface.** Add capability-described
  connectors and a scriptable command surface without weakening human approval
  by default.
- **Rewrap on resize.** If adopted, implement it in `mandatum-terminal-vt`, not
  the scene or either renderer.

## Standing Invariants

- One state machine; frontends never invent product truth.
- Rich native presentation enters only through typed `mandatum-scene`
  extensions with honest terminal fallbacks.
- `CellProgram` remains terminal parity; native may consume richer semantic
  scene data.
- Keep wgpu/winit/glyphon; no Metal/Swift rewrite.
- Keep `./ci/gate.sh`, conformance, doc trace, and regression probes.
- Add damage tracking only if profiling after the shaping cache justifies it.
