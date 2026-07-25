# Plan

`PLAN.md` points forward. Decisions and historical rationale live in
`docs/decisions.md`; standing procedures and dated evidence live in
`docs/verification.md`.

## Direction

Mandatum is a personal, GPU-native development environment with Ghostty-class
feel. The native wgpu frontend is the product. The terminal frontend is a
maintained tool for SSH, headless use, recovery, and an explicit escape hatch.

Daily-driver quality for Casey on known macOS hardware is the adoption bar.
Production-grade native in-app visual polish is now the next product phase.
Installer, release, rollout, public-GitHub presentation, and public visual
materials remain shelved for a later distribution phase. Latency, idle, resize,
recovery, and fault probes remain regression checks, not permission gates.

The complete ordered plan is
[docs/native-gpu-implementation-plan.md](docs/native-gpu-implementation-plan.md).
The visual phase is specified in
[docs/visual-polish-plan.md](docs/visual-polish-plan.md).

## Current Baseline

The workstation already has the five constitutional boundaries, one
`AppState`/`RuntimeEngine`, the shared `FrontendHost`, one app-owned event
channel, renderer-neutral input/effects, scene-owned layout and presentation,
terminal parity through `CellProgram`, typed Artifact Preview pixels, shared
grapheme/IME contracts, native input and lifecycle routes, GPU recovery, and
measurement tooling. The native typography foundation now retains
glyphon/cosmic-text behind a terminal row-run adapter, provisions pinned
JetBrains Mono with fail-closed explicit system-family overrides, and puts
native terminal colors under the active scene theme. Native startup completes
font, window, and GPU renderer preflight before constructing `FrontendHost`, so
failed preflight cannot start restore or PTY work.

The production native shell and renderer now live in the root workspace as
`mandatum-native` and `mandatum-native-renderer`. The native dependency
boundary is fail-closed, `ci/native-frontend.sh` retains product and lab
regressions, and `./ci/gate.sh` invokes it. Casey's interactive zsh now routes
`mandatum` and `mandatum-native` through the native development command while
`mandatum-terminal` remains the explicit terminal escape hatch. The installed
terminal binary and non-interactive command resolution remain untouched.

## Ordered Work

### 1. Reorder native startup — complete

The native shell keeps `host: None` during preflight and creates the window,
surface, adapter, device, queue, and renderer before `FrontendHost`. Forced
no-display and no-adapter tests prove the host creation seam is never invoked;
the real macOS startup/clean-exit path and restore coverage are green.

### 2. Promote native into the workspace — complete

The production shell and scene-only renderer are workspace packages. Lab
measurement, stress, fault, and terminal probes remain excluded; synthetic
fault injection is not in the product feature closure. Conformance rejects
GPU/window edges in every other production crate and freezes the native shell
at the `FrontendHost` seam. `./ci/gate.sh` invokes the native gate. Terminal
behavior and existing installer/release artifacts are unchanged.

### 3. De-risk typography — complete; focused decision accepted

The displayed comparison exited through the negative branch. At that point,
Ghostty's zero-config embedded JetBrains Mono was unavailable to Mandatum's
system font database, the native palette could not match Casey's Ghostty
colors, and the one-buffer-per-grapheme adapter could not shape across cells.
A Menlo control showed that cursor, styles, selection, Unicode fallback,
resize, and live 1.0→2.0→1.0 backing-scale transitions were functional, but
did not erase those structural gaps. Work 4 has since closed them.

### 4. Implement the accepted typography path — complete

The pinned JetBrains Mono faces/license, pre-host primary resolution, bounded
observable fallback reporting, `Theme::terminal_palette`, paint scopes, and
clipped glyphon/cosmic-text row runs are implemented. The terminal frontend
keeps host-palette behavior. Cell ownership remains exact for clipping,
cursor/selection quads, wide cells, and decorations; RTL/bidi reordering takes
the bounded observable anchored fallback until a renderer-neutral cell/caret
mapping exists.

The displayed corpus, deterministic foundation checks, and generation-aware
shaping cache are green. Only normally admitted runs enter the cache; exact
text/style/topology plus font, metrics, scale, and shaping-policy generations
form identity. Count, per-entry, and conservative accounted-byte ceilings are
independent, and palette/scale/device changes invalidate without carrying
buffers across generations.

Three paired 400-input runs at the same 2.0 backing scale, 1600×1200 surface,
102×35 scene, and bundled JetBrains Mono 13 reduced median shaping p50 from
0.355 ms uncached to 0.039 ms cached and median p95 from 0.470 ms to 0.074 ms.
Median whole-frame preparation changed from 3.436/4.393 ms p50/p95 to
3.388/4.107 ms. The remaining profile does not justify row-level damage
tracking in this slice.

### 5. Make native the local default — complete

Casey's interactive shell makes native the no-argument `mandatum` default and
also exposes it explicitly as `mandatum-native`. `mandatum-terminal` executes
the unchanged installed terminal release for SSH, recovery, help, version, and
update operations. Non-interactive shells retain the installed terminal
command, so the local daily-driver choice does not mutate the installer,
updater, release workflow, archives, rollout, public GitHub presentation, or
visual materials. Daily use now sets the functional hardening queue.

### 6. Production-grade native visual polish — active

Native in-app visual polish is a cornerstone product capability, not a public
release accessory. The ordered phase covers typography hierarchy, pane
materials, spacing and density, focus treatment, overlays, richer
artifact/workflow surfaces, fluid resize, purposeful transitions, and
accessibility. It preserves `CellProgram` terminal parity and introduces richer
native presentation only through typed scene contracts.

Phase 1 of [docs/visual-polish-plan.md](docs/visual-polish-plan.md) established
the visual acceptance contract, deterministic scenario catalog, fixed-reference
capture/diff workflow, and accepted current-surface baselines without changing
production pixels. Phase 2 — Token And Native Presentation Foundation — now
implements the typed UI-token, logical-geometry, semantic-presentation,
multi-metric text, and headless native-plan capability family while preserving
the terminal `CellProgram`. Phase 3 — Workspace Shell, Pane Materials, Density,
And Focus — is accepted: typed rails/badges/attention, native materials and
floating depth, compact focus, logical separator interaction, density policy,
and native window geometry/title are implemented. The real native Metal route
was reviewed at backing scale 2 through one, split, stacked, floating, zoomed,
minimum, and restored layouts; all 11 canonical references were reviewed and
explicitly accepted. A fresh repeated performance series was intentionally
scoped out when it stopped adding useful confidence; the bounded preparation
tests and existing reference measurement remain the regression guard.

Phase 4 — Overlay Family — is accepted. Palette, Search, Timeline, Session Map,
Help, Prompt, Welcome, and Context Menu now share typed native shells, bands,
selection, constraints, stable item identity, and right-aligned hints. Modal
surfaces receive a scrim; Welcome and Context Menu retain their distinct
non-modal and anchored grammars. The representative fixed references now use
the MacBook Pro built-in Retina display as the reference surface.

Phase 5 — Typed Task, Agent, Approval, And Artifact Surfaces — is accepted.
Task, agent, approval, and artifact panes now expose bounded typed workflow
rows, compact semantic badges, contained callouts, exact console regions, and
stable artifact canvas/inspector geometry without native string parsing. The
terminal fallback remains complete.

Phase 6 — Motion And Fluid Geometry — is accepted. Scene contracts carry typed
Focus, Selection, Overlay, PaneGeometry, and
ApprovalArrival targets plus whole-frame reduced/direct policy. A deterministic
renderer-local motion engine samples an injected monotonic instant; the native
shell schedules its deadlines independently from the child-exit heartbeat and
redraws only for an active deadline or a changed scene generation. Overlay
opacity covers its cell-owned text family while scale and pane geometry apply
only to native materials; glyph placement, child output, and raster placement
remain direct. Pointer admission pauses while hit-bearing material geometry is
between stable endpoints. Typing, pointer drag, live resize, and overlay close
also stay direct. Overlay close does not retain an empty material shell after
its scene-owned text disappears. Reduced motion snaps to product truth and
schedules no transition frames. The displayed approval-arrival start,
midpoint, end, and reduced states were reviewed and accepted from the clean
MacBook Pro reference capture. Three-run median motion p95 was 1.19 display
periods with 0.34% of intervals over two periods; static idle median was 0.067%
of one core with zero redraws and presents. Phase 7 — Theme And Accessibility
Completion — is next.

Known physical debt remains deliberately bounded: pane title rails and overlay
rows still inherit one terminal text row rather than their final native
vertical spacing. Concrete functional failures found through daily use remain
valid hardening work at their smallest owning seam.

### Distribution and public presentation — shelved

Installer, updater, release, rollout, archives, public-GitHub presentation, and
public visual materials remain deferred. They are not part of the native
in-app visual-polish phase and should not be pulled forward by adjacent work.

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
