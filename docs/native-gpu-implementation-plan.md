# Native GPU Frontend Plan

Status: native-first direction accepted on 2026-07-24. Work 2 promoted the
production shell and renderer into the workspace. Work 3 completed with a
focused-typography-decision verdict; that decision precedes the Work 4 cache.

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
10. wgpu and winit remain selected. The focused typography decision must now
    decide whether glyphon/cosmic-text stay and gain a row-run adapter or
    whether the text stack changes; no Metal or Swift renderer fork follows
    from that decision.

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

- Native is not yet the default launcher.
- The production path cannot load Ghostty's embedded JetBrains Mono or verify
  that a requested face resolved instead of silently falling back.
- Native terminal colors are renderer constants rather than the active theme,
  and the one-buffer-per-grapheme adapter cannot shape across grapheme/cell
  boundaries.
- The renderer reshapes repeated graphemes without the planned bounded cache.

## Work 1 — Reorder Startup — Complete

Completed on 2026-07-24.

- `App` stores `host: Option<FrontendHost>` and validated `AppConfig` during
  boot.
- `resumed()` creates the window and complete GPU renderer before invoking the
  sole `FrontendHost` construction seam.
- Window or GPU preflight failure drops configuration without constructing
  `AppState`, running restore, or starting a PTY.
- Shutdown remains idempotent when failure occurs before or after host creation.

- Deterministic no-display, no-adapter, surface, and device failure tests prove
  GPU and host construction stop at the expected boundary.
- A successful-order test proves window, GPU renderer, then host construction.
- The real macOS native shell completed GPU startup and exited cleanly; the
  native gate rechecked restore and quit.

Exit: GPU startup failure cannot strand live PTYs or partially created product
state.

## Work 2 — Promote Native Into The Workspace — Complete

Completed on 2026-07-24.

The native frontend is a production workspace component.

- `crates/native` owns the `mandatum-native` product executable and strict
  font-family/font-size options.
- `crates/native-renderer` owns scene-only GPU presentation.
- `spikes/frontend-wgpu` retains measurement, stress, fault-injection, and
  terminal probes as the excluded `mandatum-native-lab`.
- Synthetic fault injection is feature-gated and absent from the production
  executable's default dependency closure.
- The stable development command is
  `cargo run -p mandatum-native --bin mandatum-native`.
- Terminal release and installer artifacts are unchanged.

- `ci/conformance.sh` allows GPU/window crates only in the two native packages,
  freezes the renderer at `mandatum-scene`, freezes the shell at
  `FrontendHost`/scene/renderer dependencies, and negative-tests modeled GPU
  edges in every non-native production crate.
- `ci/native-frontend.sh` checks product format, Clippy, build, tests, renderer
  boundaries/default features, the lab harness, and fault-feature compilation.
- `./ci/gate.sh` invokes that native gate and remains the only CI authority.
- Latency, idle, resize, recovery, and fault probes remain regression tools.

Exit: the native frontend is a workspace component; the native gate and
`./ci/gate.sh` are green; terminal behavior is unchanged.

## Work 3 — De-Risk Typography — Complete

Completed on 2026-07-24 through the negative decision branch.

- Casey's zero-config Ghostty 1.2.3 resolves JetBrains Mono at 13 points from
  an embedded face and uses default background `#282c34`, foreground
  `#ffffff`, and its built-in ANSI palette on the LG ULTRAGEAR+ at 3440×1440,
  scale 1.0, 85 Hz.
- The nominal native launch with `--font-family "JetBrains Mono"` was rejected
  as comparison evidence: cosmic-text's system database cannot see Ghostty's
  embedded face, and the CLI validates only the string, so native silently
  displayed an unknown fallback. Native also has no configuration surface for
  Ghostty's actual base or ANSI colors.
- The same
  `spikes/frontend-wgpu/scripts/typography-corpus.sh` corpus was displayed in
  the production native shell and Ghostty with mutually available Menlo at
  13 points as a labeled control. Because native does not expose its resolved
  face, this reduced but did not eliminate face-resolution uncertainty. It
  exercised ASCII, symbols, fallback scripts, ligature sequences, CJK,
  combining text, emoji, ANSI styles, prompt cursor, native selection, and
  resize.
- Native styles, prompt cursor, selection, fallback glyphs, and a resize from
  the default window to 1650×1280 remained visible and stable. The display
  showed unjoined Arabic. Independent code inspection established the cause:
  native creates and shapes one buffer per grapheme, so shaping cannot cross
  grapheme/cell boundaries and cross-cell ligatures cannot form.
- With the LG and built-in Retina display active, the same Menlo control moved
  from backing scale 1.0 to 2.0 and back to 1.0 in both production Mandatum and
  Ghostty. Enabling Retina changed the LG from the earlier single-display
  85 Hz mode to 60 Hz. Mandatum recomputed from 191×59 on the wide LG window
  to 89×46 on the narrower Retina window and to 127×48 on return. Corpus order,
  styles, fallback/emoji, prompt cursor, chrome, and scene presentation
  remained coherent; no stale frame or scale-transition corruption was
  observed.

Verdict: the current production typography path cannot yet delight at Casey's
actual settings. A focused decision on font provisioning/resolution, palette
ownership, and row-run shaping is required before broader visual-identity
investment or a shaping cache.

Exit: displayed matrix and negative verdict recorded; the focused typography
decision is next.

## Work 4 — Resolve Typography, Then Add A Bounded Shaping Cache

Do not implement the cache until the focused typography decision defines the
shaping unit and font/theme ownership.

- Decide whether glyphon/cosmic-text stays behind a row-run adapter or the text
  stack changes.
- Make requested-face resolution observable and fail closed for an explicit
  unavailable face; define how Casey's selected font enters the product.
- Put terminal foreground, background, and ANSI colors under explicit theme
  ownership so reference comparisons can be exact.

- Memoize accepted shaped runs by text, style, and metrics.
- Preserve cell clipping, declared cell spans, cursor/selection placement, and
  wide-cell invariants.
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

Make the focused typography-path decision: font provisioning and verified face
resolution, terminal palette ownership, and row-run shaping versus a different
text stack. Stop before implementing the Work 4 cache.
