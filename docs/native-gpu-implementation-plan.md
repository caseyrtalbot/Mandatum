# Native GPU Frontend Plan

Status: native-first direction accepted on 2026-07-24. The current source still
lives under `spikes/frontend-wgpu` until Work 2 promotes it into the workspace.
That path is implementation lag, not product posture.

## Product Direction

Mandatum is a personal, GPU-native development environment with Ghostty-class
feel, living outside the terminal.

- The native wgpu frontend is the product and the primary development surface.
- Daily-driver quality for Casey on known macOS hardware is the adoption bar.
- The terminal frontend is a maintained tool for SSH, headless use, recovery,
  and an explicit escape hatch.
- There is no public-release audience or rollout ceremony.
- Native polish and richer workflow surfaces are direct product work, not
  experiments waiting for permission.

## Product Roles

- **Native:** owns the window/platform lifecycle, GPU resources, DPI, font, IME,
  clipboard, pointer translation, frame scheduling, visual identity, and richer
  typed scene surfaces. It consumes product truth only through `FrontendHost`
  and `WorkspaceScene`.
- **Terminal:** remains maintained for SSH, headless operation, recovery,
  deterministic adapter checks, and an explicit escape hatch. It consumes the
  same state machine and scene truth; it is not native's design ceiling.

## Non-Negotiable Architecture

1. There is exactly one `AppState` and `RuntimeEngine`.
2. `FrontendHost` is the shared application seam.
3. Frontends consume `WorkspaceScene`; they do not reconstruct product meaning.
4. Rich native presentation enters through typed `mandatum-scene` extensions,
   following the Artifact Preview `RasterSurface` pattern.
5. `CellProgram` remains the terminal-parity representation. Native may also
   consume richer semantic scene data.
6. Input reaches product logic as neutral `InputEvent` values.
7. Platform output leaves as typed `FrontendEffect` values.
8. Window, GPU, glyph-cache, and other live resources are never serialized.
9. Constitution laws L1–L5 and their executable gates remain authoritative.
10. wgpu, winit, glyphon, and cosmic-text remain the selected stack unless the
    typography comparison proves they cannot meet the quality bar.

## Verified Starting Point

The implementation already has one real `FrontendHost`, one app-owned event
channel, real workstation scenes in both adapters, scene-owned layout and
presentation, `CellProgram` parity, bounded Artifact Preview pixels, shared
grapheme/IME contracts, native platform input, typed GPU recovery, and
regression probes.

Historical implementation evidence is frozen in
[`spikes/frontend-wgpu/RESULTS.md`](../spikes/frontend-wgpu/RESULTS.md).
Standing procedures and current dated runs live in
[`docs/verification.md`](verification.md).

## Known Implementation Gaps

These are work, not reasons to resist the direction:

- `FrontendHost` is currently constructed before native GPU preflight.
- The native source and its renderer still live under `spikes/`.
- `ci/conformance.sh` still encodes the retired admission policy.
- `ci/gpu-spike.sh` is still spike-named and not run by ordinary CI.
- Native is not yet the default launcher.
- Typography quality has not been compared directly with Ghostty.
- The renderer reshapes repeated graphemes without the planned bounded cache.

## Work 1 — Reorder Startup

Do this first because it is a known correctness defect.

- Store `host: Option<FrontendHost>` during native application boot.
- Inside `resumed()`, create the window, surface, adapter, device, queue, and
  renderer before constructing `FrontendHost`.
- Hold validated configuration, not live application state, during preflight.
- Create `FrontendHost` only after native rendering can start.
- Keep shutdown idempotent when failure occurs at any boot stage.

- Force no-adapter startup and prove it fails before `AppState` exists.
- Force no-display startup and prove it fails before `AppState` exists.
- Prove no PTY or restored runtime starts on either failure.
- Recheck normal startup, restore, native quit, and terminal behavior.
- Run the native gate and `./ci/gate.sh`.

Exit: GPU startup failure cannot strand live PTYs or partially created product
state.

## Work 2 — Promote Native Into The Workspace

End the spike designation and make the native frontend a product component.

- Move the native shell and renderer into a production workspace package.
- Keep the product shell and renderer separate from measurement, stress, and
  fault-injection tooling.
- Retain one native executable with a stable development command.
- Leave terminal release and installer artifacts unchanged.

- Allow winit, wgpu, glyphon, and their frontend-only dependencies in the
  production native package.
- Keep negative dependency tests rejecting GPU/window crates in every
  engine-side and non-native production crate.
- Preserve the renderer's scene-only dependency direction.
- Rename the conformance messages from admission policy to dependency-boundary
  enforcement.
- Rename the native maintenance script appropriately.
- Make `./ci/gate.sh` invoke the renamed native gate so CI retains one
  authoritative command.
- Keep `./ci/gate.sh` authoritative for the workspace and Constitution.
- Prove the dependency allowlist fails when a GPU edge enters the wrong crate.
- Keep latency, idle, resize, recovery, and fault probes as regression tools.

Exit: the native frontend is a workspace component; the native gate and
`./ci/gate.sh` are green; terminal behavior is unchanged.

## Work 3 — De-Risk Typography

Run a displayed side-by-side before investing deeply in visual identity.

Use Casey's real font, size, scale, theme, and display. Compare the same corpus
in Mandatum and Ghostty:

- stems, weight, contrast, and baseline stability;
- spacing, line height, and perceived density;
- ASCII, symbols, fallback glyphs, ligatures, CJK, combining text, and emoji;
- cursor, selection, underlines, dim text, and style combinations;
- live scale changes and fluid resize.

- If glyphon/cosmic-text can delight, record and lock the typography direction.
- If it cannot, pause visual-identity investment for a focused stack decision.
- Do not infer text quality from performance measurements.

Exit: record a displayed comparison and explicit typography verdict.

## Work 4 — Add A Bounded Shaping Cache

- Memoize shaped buffers by grapheme, style, and metrics.
- Preserve per-grapheme clipping, declared cell spans, and wide-cell invariants.
- Bound the cache by count and retained bytes.
- Invalidate by generation when font, metrics, scale, or renderer configuration
  changes.
- Keep cache ownership in the native renderer.

- Record shaping and frame-stage cost before and after.
- Confirm correctness across decorated spaces, fallback glyphs, wide text,
  selection, cursor, overlays, and scale changes.
- Add row-level damage tracking only if the remaining profile demands it.

Exit: correctness gates are green and the profile shows a measurable
shaping-cost reduction without unbounded retained resources.

## Work 5 — Make Native The Default And Build Feel

- Make native Casey's default launcher.
- Keep an explicit terminal escape hatch.
- Let daily use determine the hardening queue.
- Fix concrete failures as product bugs; do not recreate pre-certification.

1. Typography.
2. Pane materials and visual hierarchy.
3. Spacing and information density.
4. Focus treatment.
5. Fluid resize.
6. Purposeful transitions with reduced-motion behavior.
7. Artifact surfaces and native workflow affordances.

- startup and shutdown never strand runtimes;
- keyboard, pointer, clipboard, and IME behavior are trustworthy;
- text is delightful at Casey's normal settings;
- resize, recovery, and continuous output remain responsive;
- failures are visible and recoverable;
- probes reveal regressions without becoming permission gates.

## Verification Policy

- `./ci/gate.sh` remains authoritative.
- The native gate runs for native changes and in CI after promotion.
- Conformance proves frontend dependency isolation.
- Scene changes require semantic and adapter coverage.
- Startup and recovery changes require deterministic fault tests.
- Visual changes require a representative displayed check.
- Latency and idle measurements are regression signals only.
- Record only commands and observations that actually occurred.

## Retired Policy

Do not reintroduce these as adoption gates:

- sub-20 ms end-to-end latency;
- 25% paired improvement;
- a 30-minute soak prerequisite;
- a multi-display matrix;
- Linux-native qualification;
- accessibility or theme parity before daily use;
- Phase 7/8 admission or rollout ceremony.

## Non-Goals

- no Metal or Swift renderer rewrite;
- no second product state machine;
- no native reacharound into app or runtime state;
- no generalized damage framework before profiling;
- no transparent mid-session frontend migration;
- no public distribution program.

## Immediate Next Action

Implement Work 1: reorder native startup so window, surface, adapter, and device
succeed before `FrontendHost` creates `AppState` or live runtimes.
