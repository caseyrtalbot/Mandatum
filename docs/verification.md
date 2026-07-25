# Verification

This document owns standing procedures and a compact dated evidence ledger.
Detailed historical native-spike narration is frozen in
`spikes/frontend-wgpu/RESULTS.md`; decisions and rationale live in
`docs/decisions.md`.

## Standard Commands

The authoritative workspace gate is:

```sh
./ci/gate.sh
```

It runs formatting, warnings-denied Clippy, build, workspace tests, the native
frontend gate, `ci/conformance.sh`, and `ci/doc-trace.sh`. GitHub Actions runs
the same script. Red means the change does not land.

The focused native frontend gate is:

```sh
./ci/native-frontend.sh
```

It checks the production packages plus the separate lab regression harness and
is invoked by `./ci/gate.sh`; it is not a second CI authority. Use
`git diff --check` and inspect `git status --short` before completion.

## Documentation Verification

After documentation changes, search active docs for:

- missing or stale paths;
- retired Phase 7/8 admission language presented as current policy;
- claims that terminal is the primary/default product direction;
- thresholds, soak, platform matrices, or parity requirements presented as
  native adoption gates;
- implementation status that disagrees with `docs/repo-structure.md`;
- verification claims without a dated run.

Historical decisions and `spikes/frontend-wgpu/RESULTS.md` may retain old
language when clearly labeled historical or superseded.

## Architecture Boundary Checks

Verify:

- `mandatum-core` remains runtime-free;
- `mandatum-commands`, `mandatum-scene`, and `mandatum-agent-runtime` remain
  frontend/window/GPU-free;
- durable JSON excludes process handles, runtime tokens, parser state, threads,
  windows, surfaces, devices, queues, textures, and glyph caches;
- frontends receive product truth through `FrontendHost` and `WorkspaceScene`;
- native drawing code never dispatches product mutations or reads app/runtime
  internals directly;
- richer native presentation uses typed `mandatum-scene` extensions;
- parser backends stay behind `TerminalAdapter`;
- GPU/window dependencies are allowed only in `mandatum-native` and
  `mandatum-native-renderer`, with negative tests for every other production
  crate; the shell's internal dependency set is frozen at the shared host,
  scene, and native renderer.

Useful scans:

```sh
rg -n "winit|wgpu|glyphon|cosmic-text|metal|appkit" crates Cargo.toml
rg -n "process_id|runtime_token|JoinHandle|NativePty|Surface|Device|Texture" crates/core
```

## Runtime And Terminal Checks

For runtime or terminal changes, prove:

- a shell starts and input reaches the focused child;
- Shift+Tab reaches the child as `ESC [ Z` unless an explicit workspace chord
  intercepts it;
- output, resize, exit, restart, task rerun, and stop remain visible and correct;
- events from replaced runtimes are rejected;
- restore persists intent without serializing live state;
- staging failure commits no lifecycle facts;
- input-reader failure shuts down runtimes and restores the host terminal;
- PTY floods remain bounded and quittable;
- terminal parsing, styles, cursor, alternate screen, scrollback, wide cells,
  selection, search, and resize invariants remain covered.

## Scene And Frontend Checks

For scene or frontend work, prove:

- terminal, task, agent, Empty, artifact, chrome, status, and overlay surfaces
  render from `WorkspaceScene`;
- hit targets match the exact painted frame;
- `FrontendHost` owns one private `AppState` and exposes no registry;
- input reaches the host as neutral `InputEvent`;
- effects leave in FIFO order as typed `FrontendEffect`;
- bounded draining preserves event truth and cannot strand a wake;
- native and terminal consume the same layout and product meaning;
- `CellProgram` remains terminal parity;
- typed native surfaces retain deterministic terminal fallbacks;
- native startup, focus, pointer, clipboard, IME, resize, scale, recovery, and
  shutdown behave without a second product state machine.

Visual behavior requires a representative displayed check. Headless preparation
tests remain the first deterministic seam.

## Native Startup Check

Startup work is not complete until tests force:

- no display/window;
- no compatible adapter;
- surface/device initialization failure;
- ordinary successful startup and restore.

The failure cases must prove `FrontendHost`, `AppState`, and live PTYs do not
exist before GPU preflight succeeds. An error classification alone is
insufficient.

## Native Frontend Gate And Regression Checks

Current command:

```sh
./ci/native-frontend.sh
```

The native gate covers:

- native package format, warnings-denied Clippy, build, and tests;
- the scene-only renderer dependency boundary;
- forced pre-host startup failures;
- surface outdated/lost, device loss, timeout, occlusion, and out-of-memory;
- bounded event draining and wake races;
- resize/scale stress and resource high-water bounds;
- glyph, clipping, IME, artifact, and overlay correctness.
- shaping-cache identity, admission isolation, generation invalidation,
  count/accounted-byte bounds, and cache-aware frame-stage evidence.

`./ci/gate.sh` invokes the native gate and GitHub Actions continues running
that one authoritative command. Latency, idle CPU, resize storms, fault
injection, and longer manual runs are regression tools. None is an adoption
permission gate.

## Input Latency Regression Check

The standing terminal escape-hatch check measures key-to-app-output bytes:

```sh
cargo build -p mandatum-app --release
cd spikes/frontend-wgpu && cargo run --release --bin tui_probe
```

The endpoint excludes host-terminal paint. A p50 drifting toward the historical
40 ms polling result indicates interval polling has returned. The existing
well-under-25 ms regression bar applies to this specific endpoint only; it is
not a native adoption threshold.

For native presentation, use the symmetric ScreenCaptureKit harness from the
native frontend tooling. Record the endpoint, display refresh, font/scale,
window geometry, raw samples, misses, commit, and build. Compare against prior
native results to detect regressions; do not treat an absolute result as
permission to pursue native polish.

For idle CPU, compare process CPU time across a clean 30-second idle window.
The intent is to detect busy spin, not to certify a release.

## Typography Comparison

Use the reference environment's actual font, size, scale, theme, and display. Render the same corpus
beside Ghostty and capture:

- ASCII, symbols, fallback, ligatures, CJK, combining text, and emoji;
- normal, bold, dim, italic, underline, inverse, and selection;
- cursor and baseline alignment;
- live scale and resize behavior.

Use `spikes/frontend-wgpu/scripts/typography-corpus.sh` through each terminal's
real PTY path. Before treating the windows as matched, prove the requested face
resolved in both applications and record the actual foreground, background,
ANSI palette, physical display, refresh rate, and backing scale. A structurally
valid family name or a visible fallback is not matched-font evidence. When a
font-resolution or scale-transition seam changes, exercise that seam on
suitable hardware; otherwise record the actual scale and do not imply that a
single-scale smoke proves a live transition.

Record the displayed evidence and a direct verdict: the glyphon/cosmic-text
stack can delight, or a focused stack decision is required before broader
visual-identity investment.

## Visual Polish Verification

Visual polish is production work for the native daily driver. Verification is
risk-driven and follows the seam that changed. Use:

1. focused portable semantic, geometry, color, shaping, or input tests for the
   changed contract;
2. representative real-window captures when pixels changed; and
3. performance, resize, motion, idle, recovery, or fault probes only when the
   owning seam changed or focused evidence exposes a credible regression.

The ordinary full repository gate runs once after source, accepted evidence,
and active documentation are synchronized. A failed or noisy focused check can
justify escalation; phase history, a broad matrix, or the mere existence of a
probe cannot. Do not claim a check until its command, capture, or observation
actually ran.

Theme completeness means coherent built-in palettes and tested app-owned
contrast pairs. It does not imply platform accessibility projection. The scene
retains typed accessibility nodes/actions, keyboard operation, focus cues, and
non-color state cues; native macOS projection and VoiceOver support must be
claimed only after that platform bridge exists and is exercised.

### Portable Semantic, Geometry, And Contrast Gates

Run the focused scene, app, and native-renderer tests before displayed work.
Use `./ci/native-frontend.sh` during development only when its aggregate scope
adds useful evidence. After the capability family and synchronized
documentation are ready, run the authoritative `./ci/gate.sh` once; it already
includes the native frontend gate.

Portable tests must prove:

- every product state is represented by typed `WorkspaceScene` data and has an
  honest `CellProgram` terminal projection; native-only radius, shadow, scrim,
  elevation, and motion state instead reaches the typed native presentation
  plan without either frontend deriving product meaning;
- pane, chrome, overlay, artifact, text-input, selection, cursor, and attention
  cells remain inside their exact scene-owned clip rectangles;
- later panes and opaque overlays completely occlude earlier text and raster
  content, including fractional physical-pixel bounds;
- painted interactive rows and their hit targets use the same layout math;
- narrow, normal, wide, minimum-usable, restored, stacked, floating, and zoomed
  geometry remains deterministic;
- wrapping and truncation are grapheme-safe, wide continuations remain atomic,
  and no text escapes a border or paint scope;
- focus, waiting, failure, success, disabled, selected, and idle states retain a
  non-color cue as well as a semantic color role; and
- every built-in theme resolves the actual native foreground/background pairs
  used by app-owned chrome, rather than merely asserting that role enum values
  differ.

For resolved native colors, require:

- at least 4.5:1 contrast for normal app-owned text and icons;
- at least 7:1 for normal text in `mandatum-high-contrast`;
- at least 3:1 for essential focus indicators, interactive control boundaries,
  selection boundaries, and other state-bearing non-text cues against adjacent
  colors. Decorative tiled separators may remain subtler because they carry no
  state or interaction meaning by themselves.

Do not reject arbitrary child-terminal ANSI combinations: the contrast gate
owns Mandatum chrome, built-in theme defaults, and app-supplied state surfaces.
User-defined theme overrides may warn instead of failing so explicit personal
choice remains possible.

Extend the existing deterministic seams rather than replacing them:

- `crates/app/src/scene_builder.rs` for product-state-to-scene meaning;
- `crates/scene/src/layout.rs` for cell geometry;
- `crates/scene/tests/cell_program.rs` for final ownership, clipping, styles,
  hit-target alignment, and text topology;
- `crates/scene/src/theme.rs` and `crates/app/src/config.rs` for built-in and
  configured theme contracts;
- `crates/native-renderer/src/gpu.rs` for native color materialization and
  physical-pixel bounds;
- `crates/native-renderer/src/row_run.rs` for shaping and cell ownership; and
- `spikes/frontend-wgpu/tests/host_wake.rs` for real `FrontendHost` scenes
  reaching the native render plan.

### Representative Scenario Selection

Maintain one deterministic visual-scenario catalog driven through
`FrontendHost` and neutral input. Handcrafted renderer-only state may exercise
an isolated boundary, but it cannot substitute for the real-host case.

The current catalog implementation is `crates/app/src/visual_scenario.rs`.
The excluded lab displays a catalog state with:

```sh
cargo run --manifest-path spikes/frontend-wgpu/Cargo.toml \
  --bin mandatum-native-lab -- \
  --visual-scenario calm-terminal \
  --font-size 13 \
  --exit-after 30
```

Every recipe must settle on its typed semantic predicate before the lab paints
it. `spikes/frontend-wgpu/tests/host_wake.rs` is the aggregate real-host and
native-plan gate. The canonical `narrow` recipe creates deliberately narrow
pane geometry inside the fixed 102 x 35 scene; it does not change the
fixed-reference client size.

The catalog contains:

- the shared typography corpus with ANSI/true color, all styles, ligatures,
  fallback scripts, CJK, combining text, emoji, cursor, and selection;
- a calm single terminal;
- a dense mixed workspace with three tiled panes and a float;
- failed-task and approval-waiting attention states;
- Palette with selected, disabled, filtered, and overflow states;
- one full list/search modal, Welcome, and the context menu;
- ready landscape/portrait artifacts, loading/failure states, and overlay
  occlusion;
- a narrow/minimum-usable frame; and
- a restored workspace.

Select the smallest set that visibly exercises the changed presentation seam.
For a palette change, sample each affected built-in theme on the dense
workspace and Palette. For geometry or hierarchy, use the representative state
that owns that geometry plus one realistic font/scale smoke when needed.
Unchanged catalog entries do not require recapture.

### Fixed-Reference Mac Visual Baselines

Pixel baselines are reference-Mac evidence, not portable Linux CI truth. Capture
the real native client surface through ScreenCaptureKit with the reference
Apple Silicon Mac, bundled JetBrains Mono profile, exact theme, physical
surface size, scene size, backing scale, display, refresh rate, commit, and
build recorded beside each image. Do not include desktop pixels, window shadow,
or an uncontrolled fallback font in the comparison.

Capture one candidate with:

```sh
spikes/frontend-wgpu/scripts/visual-regression.swift capture \
  --profile macbook-pro-metal-scale2 \
  --scenario calm-terminal
```

The capture command fails before launch when no active 2.0-backing-scale
display can hold the 800 x 600 logical client. It also rejects a scenario
window or ScreenCaptureKit frame whose live scale is not 2.0. ScreenCaptureKit
resampling a scale-1 window is never relabeled as scale-2 evidence.

Compare without writing files:

```sh
cargo run --manifest-path spikes/frontend-wgpu/Cargo.toml \
  --bin visual-diff -- compare \
  --profile macbook-pro-metal-scale2 \
  --scenario calm-terminal
```

After side-by-side human review, accept explicitly:

```sh
cargo run --manifest-path spikes/frontend-wgpu/Cargo.toml \
  --bin visual-diff -- accept \
  --profile macbook-pro-metal-scale2 \
  --scenario calm-terminal \
  --reason "describe the intended visual change"
```

Candidates live under ignored `spikes/frontend-wgpu/visual-candidates/`.
Accepted baselines alone live under tracked
`spikes/frontend-wgpu/visual-baselines/`. Acceptance rejects dirty candidate
metadata and never replaces `mask.json`.

Keep baseline, current, and heatmap/diff images for every canonical case. Judge
different pixel classes separately:

- exact sRGB material and palette values are asserted through the pure native
  resolver/presentation plan; color-managed ScreenCaptureKit pixels are not
  required to be byte-identical to source tokens;
- geometry edges and clip boundaries must match exactly, with at most one
  physical pixel of documented scale-rounding tolerance;
- bundled JetBrains Mono text on the fixed reference Mac must reach SSIM 0.995
  or better and change no more than 1% of unmasked pixels; and
- OS-dependent emoji or fallback-script glyph pixels may be masked from image
  equality, but their semantic bounds, fallback identity, and clipping remain
  hard tests.

The comparison algorithm is fixed:

- decode PNGs to unpremultiplied 8-bit sRGB;
- for SSIM, convert sRGB to linear light, compute luminance as
  `0.2126 R + 0.7152 G + 0.0722 B`, and use an 11 x 11 Gaussian window with
  sigma 1.5, edge-clamped samples, `K1 = 0.01`, `K2 = 0.03`, and `L = 1`;
- discard an SSIM window when any sample in its 11 x 11 kernel is masked;
  never fill, renormalize, or borrow masked neighbors;
- reject a baseline whose masks cover more than 5% of client-surface pixels or
  leave fewer than 90% of otherwise valid SSIM window centers;
- count an unmasked pixel as changed when any 8-bit sRGB channel differs by
  more than 2; and
- apply the 0.995 SSIM and 1% changed-pixel requirements together.

A baseline must never update automatically. Accepting an intentional visual
change requires:

1. a human side-by-side review of baseline, candidate, and diff;
2. an explicit baseline-acceptance command or flag;
3. a short rationale naming the intended visual change;
4. focused portable coverage for the changed seam; and
5. a dated evidence-ledger entry after the accepted images and code are in
   their final state.

A fuzzy threshold cannot approve a font, palette, spacing, material, or
hierarchy change by itself. Portable semantic or contrast failure blocks
baseline acceptance. After explicit acceptance, do not rerun the same
comparison merely to obtain identity against the just-written baseline.

### Motion And Reduced Motion

Motion must clarify a state transition. Keep time and animation intent typed in
the scene; the native renderer must not infer product state from wall-clock
time. Use an injectable clock for deterministic tests.

For every transition, prove:

- exact start, midpoint, and final scene/pixel states;
- monotonic progress and convergence on the stable final state;
- only the documented properties change;
- interrupted or reversed transitions settle on current product truth;
- reduced motion moves directly to the stable state and schedules no animation
  frames; and
- the approval-attention treatment remains visible without relying on motion.

The current approval pulse must retain its existing reduced-motion scene test.
Any new transition joins that test's "no unguarded motion" contract and the
displayed matrix. Prefer short 120-180 ms state transitions; looping decorative
motion is not visual polish.

### Resize, Frame, Startup, And Idle Regression Tools

Use the existing excluded native lab when a change owns one of these seams or
focused evidence makes a regression plausible. Record raw JSON, not only the
summary. Do not repeat a completed resize storm, motion/idle series, recovery
suite, or startup series for unrelated palette, spacing, or typography work.

Resize and scale:

- run:

  ```sh
  cargo run --release \
    --manifest-path spikes/frontend-wgpu/Cargo.toml \
    --bin mandatum-native-lab -- \
    --resize-count 1000 \
    --stress-interval-ms 16 \
    --exit-after 25 \
    --font-size 13
  ```

- the 1,000-action form is the full stress probe, not a standing visual-polish
  completion ritual;
- require the stress report to complete with zero action, cadence, or present
  misses and no unexpected surface/device recovery;
- capture minimum, narrow, normal, wide, and 1.0->2.0->1.0 checkpoints;
- require every applied geometry to present the matching scene before it can
  become interactive; and
- require renderer buffers, raster retention, and shaping-cache high-water
  values to remain within their existing hard bounds.

Frame preparation can use:

```sh
cargo run --release \
  --manifest-path spikes/frontend-wgpu/Cargo.toml \
  --bin mandatum-native-lab -- \
  --typing-samples 400 \
  --typing-interval-ms 16 \
  --exit-after 12 \
  --font-size 13
```

Use the established 1600x1200, scale-2, 102x35 reference fixture for comparison
authority. A single current-surface run is only a sanity check and must be
labeled with its actual scale and geometry. Escalate to a repeated reference
series when it nears or exceeds the 8 ms p95 regression line or when shaping
work needs comparison authority. Report shaping separately so a richer
material cannot hide a text regression.

Phase 6 adds the lab argument
`--visual-transition-exercise-seconds 5` plus JSON `redraw_count`,
`present_count`, and refresh-relative interval fields. Its canonical command
uses the same release binary, font, window, scale, and scenario profile. During
that five-second exercise, use the detected refresh period:
p95 present interval must stay within 1.2 times one display period, with fewer
than 1% of frames exceeding two periods.

For idle behavior, the lab supports `--warmup-seconds 5` and
`--idle-measure-seconds 30` plus JSON
`idle_window.{duration_ms,process_cpu_ms,redraw_count,present_count}`. Build the
release lab once, then take the median of three runs of:

```sh
spikes/frontend-wgpu/target/release/mandatum-native-lab \
  --visual-scenario calm-terminal \
  --warmup-seconds 5 \
  --idle-measure-seconds 30 \
  --font-size 13
```

Only the internally delimited post-warm-up interval is measured; startup and
shutdown CPU are excluded. `--idle-measure-seconds` must itself select the
lab's isolated no-restore harness, and the `calm-terminal` scenario must use
the catalog's fixed shell/output state rather than the caller's current
directory or workspace.

- process CPU must stay at or below 1.5% of one core;
- a change may not add more than 0.25 percentage points without a documented
  decision;
- record redraw and GPU-present counts as well as CPU; and
- a static workspace must not repaint solely because child-exit polling reached
  its heartbeat. A scene change or active typed motion is the reason to draw.

Compute one-core CPU percentage exactly as
`100 * process_cpu_ms / duration_ms` from the JSON idle window.

Keep the existing one-second hard ceiling for first usable native frame. Freeze
and compare the three-run median; a visual change may not regress it by more
than 20% without an explicit decision.

## Artifact Preview Checks

Prove:

- project-relative intent persists without pixels;
- no-follow traversal rejects symlinks and escapes;
- encoded, decoded, worker, pane, descriptor, and aggregate bounds hold;
- stale loads cannot replace newer intent;
- loading, ready, and failed states remain visible;
- terminal shows an honest fallback;
- native contains, clips, occludes, reloads, and releases textures correctly.

## Agent Runtime Checks

Prove agent intent, running/waiting/blocked/failed/complete state, approvals,
changed files, output tails, restore behavior, and failed-task investigation.
Adversarial task text must remain bounded, prefixed, JSON-escaped, and labeled
untrusted before it enters a mandate.

## Public Distribution Checks

The public installer, updater, and release archives are product boundaries. Any
change must rerun `ci/distribution-smoke.sh`, the distribution/update tests,
archive membership and checksum checks, and the authoritative repository gate.
The common archive must remain compatible with pre-native updaters; native
macOS binaries ship in a separate per-architecture archive. A public release is
not qualified until a real signed Apple Silicon and Intel build completes
notarization and a fresh install verifies checksums, signatures, pinned Team ID,
launch, and update behavior.

## The Stranger Test

For changes to workstation visibility, start the live-slice demo and verify a
developer unfamiliar with the current implementation can identify:

- project/session and focused pane;
- running, failed, blocked, and approval-waiting work;
- the command that produced a failure;
- changed files and agent objective;
- save/restore truth and the next useful action.

## Dated Evidence Ledger

- **2026-07-09:** initial winit/wgpu feasibility and terminal latency baselines
  were captured; detailed spike evidence is frozen in `RESULTS.md`.
- **2026-07-14:** terminal key-to-app-output measured p50 11.71 ms, p95
  13.56 ms, max 17.84 ms with zero misses; the endpoint excludes host paint.
- **2026-07-21:** the renderer-neutral effect seam and current native lockfile
  maintenance check passed.
- **2026-07-22:** shared-host wake, real-workstation content/layout, and native
  input/lifecycle routes passed focused checks and the full workspace gate.
- **2026-07-23:** Artifact Preview plus grapheme/wide-cell/IME capability
  families passed native, scene, app, and workspace gates.
- **2026-07-24:** recovery/fault checks, the 1,000-change resize/scale run, and
  three paired 1,000-sample timing acquisitions completed; recorded figures are
  regression baselines, not adoption thresholds.
- **2026-07-24:** the native-first direction retired Phase 7/8 admission policy;
  no code promotion or startup reorder is claimed by that documentation change.
- **2026-07-24:** native startup now constructs `FrontendHost` only after
  window and complete GPU renderer preflight. Forced no-display, no-adapter,
  surface, and device failures never invoked the host seam; the successful
  ordering test, 23 shell tests, 27 real-host tests, 25 renderer tests,
  warnings-denied native Clippy/boundary scan, real Apple M4 Pro/Metal
  startup-clean-exit run, and post-documentation `./ci/gate.sh` all passed.
- **2026-07-24:** Work 2 promoted `mandatum-native` and
  `mandatum-native-renderer` into the workspace, retained the excluded lab
  probes, added fail-closed native dependency checks, and integrated
  `ci/native-frontend.sh` into the authoritative gate. The final synchronized
  run passed 13 product-shell, 25 default-renderer, 25 fault-feature renderer,
  23 lab-shell, and 27 real-host tests; terminal distribution/live-PTY smoke
  passed 5 + 4 tests; conformance rejected nine modeled non-native GPU edges
  plus a native-shell PTY edge; `./ci/gate.sh` reported `GATE GREEN`; and the
  real Apple M4 Pro/Metal product binary started and exited cleanly through
  Ctrl+Q. No visual matrix was required because Work 2 changed package and CI
  ownership without changing rendered behavior.
- **2026-07-24:** Work 3 displayed one deterministic typography corpus in
  Ghostty 1.2.3 and the production native shell on the external reference display
  (3440×1440, scale 1.0, 85 Hz). The requested actual Ghostty settings
  (embedded JetBrains Mono 13; background `#282c34`; foreground `#ffffff`;
  ANSI `#1d1f21 #cc6666 #b5bd68 #f0c674 #81a2be #b294bb #8abeb7
  #c5c8c6 #666666 #d54e53 #b9ca4a #e7c547 #7aa6da #c397d8 #70c0b1
  #eaeaea`) could not be reproduced in native: its system font database cannot
  see the embedded face, explicit families do not fail on fallback, and its
  renderer constants are background `#121216`, foreground `#dcdce0`, and ANSI
  `#000000 #cd3131 #0dbc79 #e5e510 #2472c8 #bc3fbc #11a8cd #e5e5e5
  #808080 #f14c4c #23d18b #f5f543 #3b8eea #d670d6 #29b8db #ffffff`.
  A labeled Menlo 13 control reduced but did not eliminate face-resolution
  uncertainty and displayed ASCII, symbols, fallback scripts, ligature
  sequences, CJK, combining text, emoji, styles, prompt cursor, native
  selection, and a live resize to 1650×1280. The display showed unjoined
  Arabic; independent code inspection established that one buffer is created
  and shaped per grapheme, so shaping cannot cross grapheme/cell boundaries.
  `./ci/native-frontend.sh` then passed 13 product-shell, 25 default-renderer,
  25 fault-feature renderer, 23 lab-shell, and 27 real-host tests.
- **2026-07-24:** the Work 3 scale matrix then used the same Menlo control with
  the LG at backing scale 1.0 / 60 Hz and the built-in Retina display at
  backing scale 2.0; enabling Retina changed the LG from the earlier
  single-display 85 Hz mode to 60 Hz. Both production Mandatum and Ghostty moved
  1.0→2.0→1.0 with the shared corpus visible. Mandatum recomputed from 191×59
  on the wide LG window to 89×46 on the narrower Retina window and to 127×48
  on return; corpus order, styles, fallback/emoji, prompt cursor, chrome, and
  scene presentation remained coherent, with no observed stale frame or
  scale-transition corruption. Verdict: Work 3 is complete through its
  focused-typography-decision branch; broader polish and the Work 4 cache wait
  on that decision. After the evidence and active docs were synchronized,
  `./ci/gate.sh` reported `GATE GREEN`.
- **2026-07-24:** the focused typography architecture decision retained locked
  glyphon 0.12 / cosmic-text 0.19 behind a cell-aware row-run adapter, selected
  a pinned bundled JetBrains Mono default with strict observable system-family
  overrides, and placed native terminal color materialization under a typed
  scene theme palette while preserving host-palette output in the maintained
  terminal adapter. Three independent read-only lanes traced the font,
  palette, and shaping paths through current repository and locked dependency
  sources. No implementation or rendered-behavior change is claimed by this
  decision-only slice. The first synchronized `./ci/gate.sh` run reported
  `GATE GREEN`.
- **2026-07-24:** the accepted typography foundation shipped as one capability
  family. Native now resolves the four bundled JetBrains Mono v2.304 static
  faces before application launch, rejects generic/incomplete/variable-only
  installed families, prints stable headless `--font-info` JSON, preserves
  selected face IDs across device recreation, and emits bounded deduplicated
  fallback/missing-glyph diagnostics. `Theme::terminal_palette` reloads partial
  foreground/background/ANSI overrides; native materializes all 18 palette
  classes and semantic chrome while the terminal adapter still emits host
  `Reset` and named ANSI colors. The scene compiler assigns paint scopes and
  clips; the native adapter shapes checked same-style row runs, admits a real
  JetBrains Mono multicell ligature, and takes bounded observable split/anchor
  fallback for unrepresentable or visually reordered runs. Focused tests passed
  16 product-shell, 47 default-renderer, 47 fault-feature renderer, 31 terminal
  renderer, 39 scene unit plus 16 scene integration, 285 app, 23 lab-shell, and
  27 real-host checks; warnings-denied Clippy and the native boundary scan
  passed. The real production shell displayed the shared corpus with bundled
  JetBrains Mono 13: ASCII, ligature sequences, fallback scripts, CJK,
  combining text, emoji, normal/bold/dim/italic/underline/inverse combinations,
  ANSI colors, prompt cursor, header, pane chrome, and fallback diagnostics
  remained visible. The first displayed scale-2 move exposed stale physical
  surface dimensions when no separate resize event arrived; the production
  scale seam now refreshes the surface from the live window, its regression is
  green, and the complete corpus remained coherent through the repeated
  1.0→2.0→1.0 run. The excluded lab lockfile was refreshed for the renderer's
  direct `ttf-parser` edge, after which `./ci/native-frontend.sh` passed. With
  source, active docs, and the continuation handoff synchronized,
  `./ci/gate.sh` reported `GATE GREEN`.
- **2026-07-24:** Work 4 added the native renderer's admitted-run shaping
  cache. Exact text/style/byte-cell topology plus font, metrics, scale, and
  policy generations form identity; rejected parents and forced-anchor/bidi
  fallbacks cannot populate or hit it. Amortized O(1) LRU retention is capped
  at 4,096 entries, 512 KiB conservative accounted bytes per entry, and 32 MiB
  conservative aggregate accounted bytes. The renderer suite passed 57 tests
  and the lab suite passed 23. Three paired 400-input displayed runs used the
  same Apple M4 Pro/Metal 60 Hz surface, backing scale 2.0, 1600×1200, scene
  102×35, and bundled JetBrains Mono 13. Median uncached/cached shaping p50 was
  0.355/0.039 ms and p95 was 0.470/0.074 ms; median whole-frame preparation
  p50/p95 was 3.436/4.393 ms uncached and 3.388/4.107 ms cached. Cached runs
  retained 335–368 entries and 5.13–5.65 MiB accounted with 70,700–72,391
  hits, zero evictions, and zero rejections. Row-level damage remains deferred.
  With source and active docs synchronized, `./ci/gate.sh` reported
  `GATE GREEN`.
- **2026-07-24:** Work 5 made native the reference environment's local interactive default without
  changing the installed terminal or any tracked distribution surface.
  Interactive zsh resolved `mandatum`, `mandatum-native`, and
  `mandatum-terminal` through the new local functions; native
  `--font-info` returned the bundled JetBrains Mono 13 profile from
  `/private/tmp` without changing the caller directory, and the terminal
  escape hatch returned `mandatum 0.2.0`. The installed terminal SHA-256
  remained
  `b53a3238aead593344ec3f25ce421bef2e4efcb4a6e5af61dec7fc5d07968dd2`.
  Locked release builds for both binaries, 16 native-shell tests, five terminal
  distribution tests, and conformance passed. The interactive `mandatum`
  launcher displayed the real native window and exited cleanly through Ctrl+Q;
  `mandatum-terminal` displayed the maintained terminal frontend and restored
  the host terminal through Ctrl+Q. The first post-documentation
  `./ci/gate.sh` run reported `GATE GREEN`; the synchronized rerun after this
  evidence entry also reported `GATE GREEN` and is the completion authority.
- **2026-07-24:** the first native daily-drive after Work 5 reproduced a
  first-run input defect on the reference environment's vi-mode zsh: Escape dismissed the welcome
  note but also reached the child, so the following `pwd` characters edited
  and executed a prior history command. A second fresh workspace proved the
  route when Escape consumed a leading `i` as zsh's insert-mode command before
  the remaining `printf` executed. `AppState` now treats exact bare Escape as
  a one-shot consumed welcome dismissal while every other first action still
  continues normally. The RED regression first observed the existing no-PTY
  child route; the GREEN version proves first Escape consumed, second Escape
  restored to child routing, Ctrl+P then Escape still opens and closes the
  palette, directly opened Help owns its Escape without reviving Welcome, and
  an ordinary first key still follows the child route. The app library passed
  286 tests and the real-host welcome path reached the GPU plan.
  The fixed native build displayed `Esc dismisses · other input continues`;
  Escape then left `printf MANDATUM_ESCAPE_CONSUMED_OK` intact, and
  Ctrl+Q exited cleanly. Pre-completion gate runs exposed the real-host test's
  stale old-copy assertion, guidance that fit native but truncated in the
  80-column terminal renderer, and excluded-lab rustfmt drift. Each
  synchronization defect was corrected before completion. The synchronized
  code, test, and documentation `./ci/gate.sh` run then reported `GATE GREEN`.
- **2026-07-24:** the visual-polish planning audit launched the production
  native application from `/private/tmp`, inspected the real first-run Welcome,
  opened the Palette and Help surfaces, created a three-pane tiled layout, and
  exited through Ctrl+Q with status 0. Screen captures showed the functional
  surface still following the flattened terminal-cell presentation: touching
  box borders, one text hierarchy, weak material separation, and inverse-video
  selection. A separate source audit found task, agent, and artifact details
  flattened into labeled strings before native rendering. These observations
  informed `docs/visual-polish-plan.md`; they are baseline planning evidence,
  not a claim that the planned visual system or its future baseline harness
  exists.
- **2026-07-24:** Phase 1's portable catalog and tooling pass reached all 11
  typed scenarios through the real `FrontendHost`, compiled them through the
  native plan, typechecked the ScreenCaptureKit script, displayed
  `calm-terminal` through the lab, and verified the live-slice native launch
  route. The fixed-reference capture command then refused the only active
  `external reference display` display because its live backing scale was 1.0. No baseline
  was captured, accepted, or claimed from resampled pixels; Phase 1 remains
  open until a genuine scale-2 display or mode is active.
- **2026-07-24:** Phase 1 then used the LG display's genuine 1720 x 720
  logical / backing-scale-2 mode to capture all 11 fixed 1600 x 1200
  client-surface scenarios from clean source commit `ebd7ee4`. Visual review
  rejected the first set because the context-menu scenario had been dismissed
  by initial window geometry; the harness now waits for that geometry to
  settle before driving product state. The reviewed set visibly includes
  Typography selection/cursor, filtered/disabled/overflow Palette state,
  context menu, and artifact overlay occlusion. Every image was explicitly
  accepted with a nonblank reason. All 11 strict comparisons returned SSIM
  1.0, zero changed pixels, zero masked pixels, and 1,920,000 compared pixels.
  The LG display was then restored to its 3440 x 1440 default and independently
  reported backing scale 1.0.
- **2026-07-24:** Phase 2 implemented the native presentation foundation while
  preserving the terminal cell projection. Focused evidence passed 10 theme
  tests, 18 config tests, three logical-geometry tests, 16 `CellProgram`
  parity tests, 296 app tests, 18 native-shell tests, 61 renderer unit tests,
  three native-plan integration tests, and the excluded lab's 13 library,
  23 shell, and 28 real-host tests. The isolated native token sampler displayed
  all 17 direct UI color roles while retaining the independent terminal
  palette. `./ci/native-frontend.sh` passed and the synchronized
  source-and-documentation `./ci/gate.sh` run reported `GATE GREEN`.
  The first fixed-reference comparison also exposed random temporary fixture
  paths in review-facing scenes; the harness now normalizes those exact
  isolated identities and proves independently prepared snapshots equal.
  All 11 scenarios were then captured from clean source commit `6979318` on
  the exact LG scale-2 / 60 Hz profile. Visual review confirmed the intended
  content change was limited to `visual-project` and `$VISUAL_PROJECT`.
  Comparison also exposed that Phase 1's first four images had the normal color
  state while the seven beginning three seconds after 18:00 were uniformly
  darker, consistent with the observed Zoom screen-share transition at the
  class boundary. The post-class Phase 2 set was internally consistent; its
  explicit acceptance reasons record fixture normalization and, for those
  seven, correction of the external compositor-state contamination. All 11
  accepted comparisons then returned SSIM 1.0, zero changed pixels, zero
  masked pixels, and 1,920,000 compared pixels. The display was restored and
  verified at 3440 x 1440 / scale 1 / 60 Hz before the final gate. The
  synchronized `./ci/gate.sh` run reported `GATE GREEN`; source commit
  `6979318` and accepted-evidence commit `f300662` were then pushed to
  `origin/main`.
- **2026-07-24:** Phase 3's source capability reached focused green before
  displayed work. The run passed 302 app tests, 16 core tests, 44 scene unit
  plus 16 cell-program and three geometry tests, 31 terminal-renderer tests,
  65 native-renderer unit plus four presentation-plan tests, 23 native-shell
  tests, two frontend-parity tests, and the excluded lab's exact
  `every_visual_scenario_reaches_its_typed_state_and_gpu_render_plan` test.
  These runs prove typed materials/text, logical separator input, density
  geometry, focus/badges, floating occlusion, window policy/title, terminal
  parity, and resource bounds.
- **2026-07-24:** Phase 3 displayed acceptance used the real native Metal route
  on Apple M4 Pro and `external reference display` at 1720 x 720 logical / 3440 x 1440
  physical / backing scale 2 / 60 Hz. Interactive review covered one, split,
  stacked, floating, zoomed, and 720 x 480 minimum layouts; the locked
  `restored` scenario covered restored split geometry. All 11 canonical
  candidates from clean source commit `7221937` were visually reviewed and
  explicitly accepted. Intended changes were the quiet header/status and title
  rails, compact focus tick, typed badges/attention chips, one-logical-pixel
  separators, and rounded bounded floating material/shadow. Terminal content,
  cursor/selection/raster paint, and overlay content remained authoritative.
  A fresh repeated performance series was explicitly scoped out when it stopped
  adding useful confidence; the existing reference preparation result, focused
  resource ceilings, and repository gate remain the regression checks. The LG
  was restored and independently verified at 3440 x 1440 / backing scale 1 /
  60 Hz before the synchronized final gate, which ended `GATE GREEN`.
- **2026-07-24:** Phase 4 migrated all eight overlays onto one typed native
  family. Focused and aggregate review corrected overlay cursor ordering and
  contrast, item text preservation, rounded band insets, constrained Context
  Menu hit geometry, stable Help identities, right-aligned clipped hints, and
  degenerate shells. The representative `palette`, `full-modal`, `welcome`,
  and `artifacts` matrix was captured through the real native Metal route from
  clean source commit `1988b0b` on the MacBook Pro built-in Retina display at
  800 x 600 logical / 1600 x 1200 physical / backing scale 2 / 120 Hz. Each
  image was visually reviewed and explicitly accepted; repeated comparisons
  returned SSIM 1.0, zero changed pixels, zero masked pixels, and 1,920,000
  compared pixels. The complete repository gate, including the excluded lab
  and all canonical scenario-plan tests, ended `GATE GREEN`. A broader fresh
  performance series and unchanged-scenario recapture were intentionally
  scoped out as overkill.
- **2026-07-24:** Phase 5 replaced flattened task, agent, approval, and
  artifact details with bounded typed scene rows, stable workflow identities,
  compact status badges, contained callouts, exact console material, and
  stable artifact inspector/canvas geometry. Aggregate review corrected
  unbounded agent lines, resize identity loss, diagnostic status semantics,
  prompt-prefix inference, task-output material coverage, approval attention,
  compact badge geometry, and missing/mismatched artifact canvases. The
  canonical real-host aggregate test asserts the resulting scene roles and
  native materials. `./ci/native-frontend.sh` passed, and the repository gate
  ended `GATE GREEN` before capture. From clean source commit `d17bdd2`,
  `dense-workspace`, `attention`, and `artifacts` were captured through the
  real native Metal route on the MacBook Pro built-in Retina display at
  800 x 600 logical / 1600 x 1200 physical / backing scale 2 / 120 Hz. Each
  was visually reviewed and explicitly accepted; repeated comparisons returned
  SSIM 1.0, zero changed pixels, zero masked pixels, and 1,920,000 compared
  pixels. Unchanged scenarios and a fresh performance series were deliberately
  scoped out because Phase 5 changed only this representative workflow family.
- **2026-07-25:** Phase 6 added scene-owned typed Focus, Selection, Overlay,
  PaneGeometry, and ApprovalArrival intent plus renderer-local deterministic
  progress, direct/reduced policy, motion deadlines independent from the
  child-exit heartbeat, and pointer suspension during hit-bearing material
  interpolation. Review of the first checkpoint candidates exposed a
  fixed-reference projection defect: the retained arrival snapshot used a
  compatibility 102×35 viewport, compressing native materials into the
  upper-left while GPU-measured text stayed full-size. The checkpoint route now
  retains the real `ViewportMetrics`, snapshot, and target instant across
  bounded surface retries, freezes only after `Presented`, and publishes the
  exact capture title only after that presentation so stale pixels fail closed.
  From clean source commit `4732ba8`, `attention-motion-start`,
  `attention-motion-midpoint`, `attention-motion-end`, and `attention-reduced`
  were captured through the real native Metal route on the MacBook Pro
  built-in Retina display at 800×600 logical / 1600×1200 physical / backing
  scale 2 / 120 Hz. Each was visually reviewed and explicitly accepted;
  repeated comparisons returned SSIM 1.0, zero changed pixels, zero masked
  pixels, and 1,920,000 compared pixels. Three five-second motion runs reported
  p95 intervals of 1.741, 1.186, and 1.191 display periods and fractions over
  two periods of 2.21%, 0%, and 0.34%; the required three-run medians were
  1.191 periods and 0.34%. Three 30-second post-warm-up idle windows reported
  0.600%, 0.067%, and 0.067% of one core with redraw/present counts of 1/1,
  0/0, and 0/0; the medians were 0.067% and 0/0, improving on the prior 0.93%
  reference rather than consuming the 0.25-point regression allowance. The
  synchronized final `./ci/gate.sh` ended `GATE GREEN`.
- **2026-07-25:** the native visual-polish finishing slice completed its
  focused source and displayed review before the final synchronized repository
  gate. Three independent cold reviews found an overlay empty-state overwrite,
  lost semantic emphasis, line-box clipping/overlap, and an invisible light
  ANSI 15 value; each defect was corrected. The synthetic hardcoded 18-point
  test was removed in favor of real displayed evidence. From clean provisional
  capture commit `95d9d74`, six captures were serialized across
  `dense-workspace` and `palette` in `mandatum-dark`, `mandatum-light`, and
  `mandatum-high-contrast`. All six were visually reviewed. Only the two
  changed dark references were accepted: before acceptance, dense-workspace
  measured SSIM `0.6264913090`, changed-pixel fraction `0.8773026042`
  (`1,684,421 / 1,920,000`), while Palette measured SSIM `0.7299262968`,
  changed-pixel fraction `0.7008130208` (`1,345,561 / 1,920,000`). No
  post-accept identity comparison was run because it would only compare the
  candidate with the baseline just written. An earlier dark dense candidate
  exposed multirow overlap and was rejected before the fix. An earlier
  high-contrast capture overlapped an 18-point window, was invalidated, and was
  rerun serially rather than recorded as evidence.
- **2026-07-25:** the final 18-point smoke from clean provisional commit
  `8092f7c` used the real M4 Pro/Metal route on the MacBook Pro built-in Retina
  display at backing scale 2, 1600×1200 physical pixels, 74×25 cells, and
  120 Hz. It passed visual review with no clipping or overlap, zero app-owned
  `AdvanceMismatch` diagnostics, a 90-entry shaping cache with 662 hits and
  114 misses, and first usable frame at 605.5 ms. Remaining fallback
  diagnostics were terminal Unicode fallback only. One final-code,
  current-surface scale-1 frame-preparation sanity run produced 401 samples:
  whole-frame p50 `3.408666 ms`, p95 `3.998792 ms`; shaping p50
  `0.055083 ms`, p95 `0.072833 ms`. It is below the 8 ms regression line but
  is not mislabeled as the scale-2 three-run reference authority. Motion,
  idle, recovery, and 1,000-resize probes were not repeated because their
  owning seams did not change. The final synchronized `./ci/gate.sh` ended
  `GATE GREEN`.
- **2026-07-25:** public-release preparation replaced the repository entrance
  with capability-first product documentation and explicit current limits,
  sanitized the current tree's agent fixtures and machine-specific labels,
  renamed the fixed-reference profile, and prepared version `0.3.0`. The
  release boundary now builds backward-compatible common archives plus native
  macOS archives, pins workflow actions, requires exact Developer ID team and
  authority evidence, requires Apple notarization status `Accepted`, verifies
  checksums and exact members at install time, and preserves recovery copies
  if rollback itself fails. The updater permits equal-version completion when
  a platform binary is missing. Approval execution now uses a private per-user
  runtime root, preflights its bridge, and converts any bridge execution
  failure into a blocking hook result. Focused bridge, private-runtime,
  installer, retained-recovery, equal-version migration, parser, distribution,
  conformance, syntax, and documentation checks passed. `ratatui` moved to
  `0.30.2`, removing the affected transitive `lru 0.12.5` dependency. Live
  repository settings confirmed private vulnerability reporting, secret
  scanning, and push protection enabled. The first Linux remote gate exposed
  real input latency under an unbounded `yes` flood: 256 KiB of admitted PTY
  output could remain ahead of a queued quit chord. The physical per-pane cap
  was tightened to 64 KiB, and ten consecutive focused flood/quit runs passed
  before the synchronized gate was repeated. No native release was tagged:
  this checkout has no Developer ID Application certificate and the required
  Apple release secrets are not configured. The synchronized final
  `./ci/gate.sh` ended `GATE GREEN`.

## Completion Rule

Do not claim a task is complete until:

- relevant source and active docs agree;
- required commands pass or are explicitly scoped out;
- displayed checks run when visual behavior changed;
- remaining risks and known implementation drift are named;
- `git diff --check` and `git status --short` are inspected.
