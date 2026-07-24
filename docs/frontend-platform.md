# Frontend Platform Strategy

## Direction

Mandatum is a personal GPU-native development environment. The native
wgpu/winit frontend is the product and the primary daily-driver target. The
terminal frontend is a maintained operational tool. There is no public-release
audience.

Product behavior remains frontend-neutral: one `AppState`/`RuntimeEngine`,
`FrontendHost`, and `WorkspaceScene` support both roles without duplicating
terminal, task, agent, approval, persistence, or recovery truth.

## Frontend Roles

### Native frontend — product

Use for:

- Casey's normal local development;
- Ghostty-class text, input, resize, and frame feel;
- precise pointer, selection, clipboard, and IME behavior;
- native visual identity, motion, and richer typed surfaces;
- artifact and workflow presentation that cannot be expressed honestly as
  terminal cells.

It owns the window, platform events, GPU lifecycle, font/scale state, glyph and
texture caches, and presentation scheduling. It receives product state only
through `FrontendHost` and paints only `WorkspaceScene`.

The implementation currently remains at `spikes/frontend-wgpu` until the
promotion work moves it into the root workspace. That location is current code
state, not a product decision.

### Terminal frontend — maintained tool

Use for:

- SSH and remote operation;
- headless or low-dependency environments;
- recovery when native startup is unavailable;
- deterministic frontend checks;
- an explicit escape hatch.

It remains a first-class terminal experience and continues to preserve L5 input
routing. It is not the design ceiling or default product target.

## Shared Contract

Every frontend must:

- consume neutral input and typed effects through `FrontendHost`;
- paint from `WorkspaceScene`;
- leave layout, product state, command routing, persistence, runtime identity,
  and recovery policy in shared modules;
- expose platform failures clearly;
- keep live frontend resources out of durable state;
- support deterministic checks at the deepest practical seam.

`CellProgram` is the complete terminal-parity representation. Native may also
consume richer semantic scene data, but only through typed `mandatum-scene`
extensions. Artifact Preview's bounded `RasterSurface` is the reference
pattern: durable intent in core, safe live loading in app, typed pixels in the
scene, native presentation, and an honest terminal fallback.

## Selected Stack

Keep:

- winit for window and platform event integration;
- wgpu for portable native GPU rendering;
- glyphon/cosmic-text for the current text path;
- `mandatum-scene` as the frontend contract.

Do not start a Metal or Swift renderer fork. First compare the current text
stack directly with Ghostty using Casey's actual font, size, scale, theme, and
display. If the comparison cannot delight, record a focused stack decision
before investing in broader polish.

## Verified Baseline

The current native implementation:

- drives the real `FrontendHost` and `RuntimeEngine`;
- wakes from the app-owned event channel through `EventLoopProxy`;
- translates platform input into neutral `InputEvent`;
- paints real terminal, task, agent, Empty, artifact, chrome, status, and
  overlay scene data;
- shares layout, paint order, styles, grapheme spans, cursor, selection, and
  composition semantics with the terminal adapter;
- completes window and GPU renderer preflight before constructing
  `FrontendHost`, restore state, or live PTYs;
- handles clipboard, pointer capture, scrollback, focus, resize, scale, restore,
  and shutdown without a second product state machine;
- has typed surface/device recovery, explicit GPU failures, bounded draining,
  resource bounds, stress tooling, and regression probes.

Detailed historical runs are frozen in
[`spikes/frontend-wgpu/RESULTS.md`](../spikes/frontend-wgpu/RESULTS.md). Current
procedures and dated one-line evidence live in
[`docs/verification.md`](verification.md).

## Forward Work

The ordered work is:

1. Reorder startup so GPU preflight succeeds before `FrontendHost` exists
   — complete.
2. Move the native frontend into the workspace, narrow the GPU dependency
   allowlist, and make the authoritative gate run the native checks in CI.
3. Compare text quality directly with Ghostty.
4. Add a bounded generation-aware shaping cache and profile it.
5. Make native Casey's default and build the feel roadmap through daily use.

The authoritative detail is
[`docs/native-gpu-implementation-plan.md`](native-gpu-implementation-plan.md).

## Verification Policy

`./ci/gate.sh`, conformance, doc trace, and the native gate remain mandatory.
Latency, idle, resize, recovery, and fault measurements remain visible
regression checks. They are not adoption permission gates.

Retired requirements include the former sub-20 ms threshold, 25% paired
improvement, mandatory long soak, multi-display matrix, Linux-native target,
accessibility/theme parity before daily use, and Phase 7/8 rollout ceremony.

## Current Implementation Drift

Until promotion lands:

- native still lives under `spikes/frontend-wgpu`;
- `ci/gpu-spike.sh` retains its historical name and is not ordinary CI;
- `ci/conformance.sh` still encodes the retired admission branch;
- native is not yet the default launcher.

These are explicit next-work items. Do not reinterpret them as reasons to return
to the retired product posture.
