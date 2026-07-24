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

Use Casey's actual font, size, scale, theme, and display. Render the same corpus
beside Ghostty and capture:

- ASCII, symbols, fallback, ligatures, CJK, combining text, and emoji;
- normal, bold, dim, italic, underline, inverse, and selection;
- cursor and baseline alignment;
- live scale and resize behavior.

Use `spikes/frontend-wgpu/scripts/typography-corpus.sh` through each terminal's
real PTY path. Before treating the windows as matched, prove the requested face
resolved in both applications and record the actual foreground, background,
ANSI palette, physical display, refresh rate, and backing scale. A structurally
valid family name or a visible fallback is not matched-font evidence. If the
active hardware cannot produce a live backing-scale transition, the displayed
matrix remains incomplete until suitable hardware is available; do not
substitute a lab probe.

Record the displayed evidence and a direct verdict: the glyphon/cosmic-text
stack can delight, or a focused stack decision is required before broader
visual-identity investment.

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

## Legacy Distribution Checks

There is no public-release audience. The existing `mandatum` terminal command,
installer, updater, and release archives remain on disk as operational tooling,
not an active distribution roadmap or native adoption path. If those surfaces
change, re-run their existing binary, distribution, installer, checksum, and
update tests. Native promotion does not add a native binary to the legacy
archives.

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
  Ghostty 1.2.3 and the production native shell on the LG ULTRAGEAR+
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

## Completion Rule

Do not claim a task is complete until:

- relevant source and active docs agree;
- required commands pass or are explicitly scoped out;
- displayed checks run when visual behavior changed;
- remaining risks and known implementation drift are named;
- `git diff --check` and `git status --short` are inspected.
