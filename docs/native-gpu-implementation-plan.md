# Native GPU Frontend Plan

Status: native-first direction accepted on 2026-07-24. Work 2 promoted the
production shell and renderer into the workspace. Work 3 completed with a
negative typography verdict. The accepted typography foundation is now
implemented, displayed, and backed by the bounded Work 4 shaping cache.

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
10. wgpu, winit, glyphon, and cosmic-text remain selected. Native text gains a
    focused row-run adapter; no Metal, Swift, terminal-widget, or second text
    renderer fork follows from the typography decision.

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
contract is accepted.

## Work 4 — Implement Accepted Typography, Then Add A Bounded Shaping Cache

Completed on 2026-07-24. The focused decision, foundation capability family,
and bounded cache are implemented.

### Font provisioning and face truth

- Retain locked glyphon 0.12 / cosmic-text 0.19.
- Make bundled JetBrains Mono 13 the native default. Vendor the unmodified
  Regular, Bold, Italic, and Bold Italic faces from one pinned official release
  with the upstream OFL, source URL, version, and SHA-256 values.
- Build and validate the complete `fontdb` before constructing `FontSystem`.
  Remove or partition every same-family system face before inserting bundled
  faces, and select primary faces by source identity so an installed duplicate
  cannot win an ordinary CSS-style query. Other system fonts remain available
  for script and emoji fallback.
- Treat `--font-family NAME` as a strict installed-family override. Reject
  generic aliases, missing families, non-monospace faces, or missing required
  style faces before the window, GPU, or `FrontendHost` starts. Query success
  is insufficient because `fontdb` performs closest matching: require exact
  `FaceInfo` weight/style metadata for Regular, Bold, Italic, and Bold Italic.
  Variable-only system families are outside the first implementation. Do not
  add an arbitrary `--font-file` path in this family.
- Add `--font-info`, which resolves without a window or host and prints stable
  JSON for source, requested family, size, and the four selected PostScript
  faces. Normal startup emits one concise primary-face record.
- Inspect shaped `LayoutGlyph::font_id` values. Deduplicate and bound observed
  fallback-face and missing-glyph records; legitimate CJK/emoji fallback
  remains allowed and named rather than treated as primary-face failure. The
  report resets per font-catalog generation and retains at most 64 records /
  64 KiB total, with every copied family, PostScript name, and sample truncated
  to 256 UTF-8 bytes.
- Device recreation reuses the resolved provisioning profile. It must not
  rescan into a different primary face.

### Terminal palette ownership

- Add a serializable `TerminalPalette` to `mandatum_scene::Theme`: direct RGB
  foreground/background plus exactly 16 direct RGB ANSI slots.
- Load partial overrides under `[theme.terminal]`; invalid or recursive
  ANSI/default values are rejected, and missing values inherit the selected
  built-in theme.
- `SceneColor` and `CellProgram` remain abstract. The native renderer resolves
  every `Default` and `Ansi(0..=15)` through the active terminal palette,
  including semantic chrome roles; `Indexed(16..=255)` keeps the standard
  cube/grayscale and direct RGB remains direct.
- The maintained terminal renderer deliberately keeps `Reset` and named ANSI
  output so SSH/recovery sessions inherit their host-terminal palette. Exact
  materialization belongs to the native pixel surface; changing the escape
  hatch to emit explicit RGB requires a separate decision.
- Start built-in palettes with the current native constants so implementing
  ownership alone does not smuggle in a visual redesign. Casey's recorded
  Ghostty palette becomes expressible configuration.

### Row-run shaping adapter

- Replace `prepare_cell_program`'s one-buffer-per-grapheme output with one deep
  native-renderer component that converts final topmost `CellProgram` cells
  into bounded shaping runs. It does not read parser, PTY, app, or pane state.
- Extend the renderer-neutral compiled program with a text paint-scope identity
  and exact clip rect assigned by the scene compiler for pane content, pane
  chrome, header/status, and each overlay. Flattening still chooses final
  topmost cells, but native never joins cells across paint scopes and intersects
  every run bound with the scope clip.
- A run is a maximal same-row sequence of adjacent printable graphemes with the
  same resolved glyph style. Gaps, plain whitespace, raster-backed cells,
  hidden cells, orphan wide continuations, row changes, cursor/selection
  transitions, paint-scope changes, and any style change are hard boundaries.
  Decorated whitespace remains a standalone one-cell run.
- A width-two grapheme and its continuation form one atomic standalone run in
  the first implementation. The continuation contributes no text and extends
  the grapheme's declared span to two cells.
- Each run stores UTF-8 text, rich-text style ranges, and a checked byte-range
  to declared-cell-span map. Shape it with `Shaping::Advanced`, `Wrap::None`,
  `Buffer::set_monospace_width(Some(cell_width))`, and one exact
  run-width/run-height `TextArea` bound. The locked API rounds glyph advances
  to cell-width multiples while retaining ligatures and font fallback.
- Generalize the existing one/two-cell `u8` text-bound helper to checked `u16`
  run widths. Group laid-out glyphs by byte cluster; every cluster must map to
  complete input spans, its unioned x/advance interval must match the mapped
  declared-cell interval, and the total advance must match the run width, all
  within 0.5 physical pixel at the active scale.
- Terminal grid order is authoritative. The first row-run adapter admits only
  monotonically increasing left-to-right cluster intervals. A layout carrying
  RTL levels or visual bidi reordering fails admission and follows the
  split/anchored path; native does not silently move a glyph away from the
  cursor/selection cell that owns it. Correct bidi plus cell/caret mapping
  requires a later renderer-neutral text-order contract, not an inference in
  the GPU adapter.
- Background, cursor, selection, inverse, and decorated-space geometry remains
  final-cell quad truth. Shaped glyph positions never move those cell-aligned
  marks. Text is clipped to the run's declared cell rectangle; no glyph may
  paint into an adjacent pane, row, or artifact layer.
- On a mapping/advance failure, split at the offending grapheme boundary.
  Retain the existing anchored grapheme path only as the ultimate bounded
  fail-safe, with a visible bounded diagnostic. It is not the default shaping
  path or the cache key.

Custom HarfBuzz/swash atlas work, a terminal widget/parser import, and a
different GPU renderer all duplicate boundaries glyphon/cosmic-text already
satisfy. Reconsider a focused lower-level positioning adapter only if the
admitted row-run tracer cannot preserve clipping and cell geometry. Full bidi
support is explicitly separate from this cache-preparation family.

### Verification and cache order

- Prove exact bundled/system face resolution, pre-host failure, stable
  `--font-info`, bounded fallback reporting, and device-recreation identity.
- Prove theme overlay/reload behavior, all 18 native palette colors, unchanged
  abstract `CellProgram` colors, and unchanged host-palette terminal output.
- Prove multi-cell LTR ligatures within a run; style/gap/row/cursor/selection/
  raster/paint-scope boundaries; checked per-cluster maps and advances;
  combining, emoji, fallback, wide cells, decorations, pane/artifact clipping,
  and scale changes. Prove RTL/bidi input takes the bounded observable fallback
  without bleed, panic, or cursor/selection drift; do not claim bidi support.
- Display the shared typography corpus at Casey's accepted font, size, theme,
  and scale before beginning the cache.

The foundation checks and cache are green:

- accepted shaped runs are memoized by text, resolved style,
  byte-to-cell topology, font-catalog/profile
  generation, metrics, and scale generation; observed fallback identities live
  in the cached value and diagnostics because they are shaping outputs;
- the cache is bounded at 4,096 entries, 512 KiB conservative accounted bytes
  per entry, and 32 MiB conservative aggregate accounted bytes; these are
  explicit accounting limits because cosmic-text does not expose exact
  allocator retention for `Buffer`;
- amortized O(1) LRU eviction prevents refill churn from scanning the cache;
- generation changes invalidate when font, palette, metrics, scale, or renderer
  configuration changes;
- device recreation retains lifetime counters but no cached buffers;
- cache ownership remains in the native renderer; and
- the lab records cache statistics, shaping time, full frame-preparation time,
  actual backing scale, surface size, and scene geometry.

Three paired 400-input runs on the same 60 Hz Apple M4 Pro/Metal surface used
bundled JetBrains Mono 13 at backing scale 2.0, 1600×1200, and 102×35. Median
uncached versus cached shaping p50 was 0.355 ms versus 0.039 ms; median p95 was
0.470 ms versus 0.074 ms. Median whole-frame preparation p50/p95 changed from
3.436/4.393 ms to 3.388/4.107 ms. Cached runs retained 335–368 entries,
5.13–5.65 MiB accounted, 70,700–72,391 hits, zero evictions, and zero
rejections. The profile does not justify row-level damage tracking now.

Exit: accepted face, palette, and row-run behavior is displayed and gated; the
bounded cache shows a measurable shaping-cost reduction without an unbounded
entry or accounted-byte resource.

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

Begin Work 5 by making the native shell Casey's default local launcher while
preserving an explicit terminal escape hatch. First inventory the current
development command, `mandatum` terminal command, installer, and updater
entrypoints; choose one narrow default-launch seam without changing legacy
archives or creating public distribution work. Stop before pane-material,
spacing, transition, installer, release, or rollout changes.
