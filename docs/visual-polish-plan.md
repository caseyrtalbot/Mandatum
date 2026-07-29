# Native Visual Polish Plan

Status: complete and accepted (2026-07-25)

This document is the ordered implementation authority for production-grade
native in-app visual polish. `PLAN.md` owns the broader product sequence,
`docs/decisions.md` owns accepted rationale, and `docs/verification.md` owns
the standing evidence procedures.

## Objective

Make Mandatum feel like a finished, trustworthy development instrument during
hours of daily use. Visual quality is a product capability, not a final coat
of marketing paint.

This phase includes:

- typography hierarchy;
- native materials and visual depth;
- pane hierarchy, focus, spacing, and density;
- the complete overlay family;
- typed task, agent, approval, and artifact presentation;
- purposeful motion and fluid geometry;
- coherent dark, light, and high-contrast themes;
- typed accessibility semantics, keyboard operation, and non-color cues; and
- maintained visual-regression evidence.

A native macOS accessibility adapter and VoiceOver qualification are deferred
rather than implied by the typed scene semantics. They remain valid future
enhancements, not completion gates for the reference environment's current daily-driver surface.

This completed phase did not include installer, updater, release, archives, or
public-repository presentation. Those concerns are handled by the later public
distribution slice and the current root roadmap.

## Starting Point

The current native frontend is the daily-driver product and already has the
correct structural foundation:

- one `AppState` and `RuntimeEngine`;
- one shared `FrontendHost`;
- renderer-neutral `WorkspaceScene` product truth;
- terminal parity through `CellProgram`;
- exact cell, clipping, cursor, selection, grapheme, IME, and hit-target
  contracts;
- a bundled JetBrains Mono primary;
- theme-owned terminal colors;
- clipped row-run shaping and a bounded shaping cache;
- typed Artifact Preview pixels;
- GPU recovery and scale/resize probes; and
- a maintained terminal escape hatch.

The visible gap is presentation. Native still largely paints the flattened
terminal cell program: one typographic voice, touching box-glyph borders,
weak separation between canvas and overlays, inverse-video list selection,
and task/agent/artifact detail flattened into labeled strings.

## Visual North Star: Quiet Instrument

Mandatum should feel like a dense graphite workbench:

- terminal content is the visual center;
- tiled panes form one calm continuous workspace;
- structure comes from material contrast, typography, and spacing;
- one accent communicates focus and navigation;
- semantic hues are reserved for changed operational state;
- floating and modal surfaces gain restrained elevation;
- workflow surfaces expose product meaning rather than debug-like strings; and
- motion explains transitions without competing with terminal output.

Avoid generic dashboard cards, neon hacker styling, glassmorphism, decorative
gradients, ambient glow, ornamental animation, and excessive whitespace.

## Design System Contract

The values below are the flagship-dark starting contract. Phase 1 records them
in canonical scenarios; Phase 2 validates them on the real display before
freezing the built-in theme.

### Color Roles

| Role | Value | Use |
|---|---|---|
| `canvas` | `#0B0D10` | Window and terminal foundation |
| `pane_surface` | `#10141A` | Tiled pane chrome and non-terminal workflow surface |
| `chrome_surface` | `#171B23` | Header, status, and title rails |
| `overlay_surface` | `#1C212B` | Palette, help, prompts, menus, and floating inspectors |
| `border_subtle` | `#2A313D` | Tiled separators and quiet boundaries |
| `border_strong` | `#5E6C84` | Hovered controls and raised boundaries |
| `text_primary` | `#E7EAF0` | Primary app-owned text |
| `text_secondary` | `#AAB2C0` | Supporting text |
| `text_muted` | `#8D97A8` | Metadata and hints |
| `focus` | `#78A9FF` | Navigation, selection, and keyboard focus |
| `running` | `#72D6A0` | Active successful work |
| `waiting` | `#F2C66D` | Approval and blocked attention |
| `failure` | `#FF7A8A` | Failure and destructive state |
| `complete` | `#73D8D4` | Completed/informational state |
| `agent_identity` | `#C0A7F2` | Agent identity only, never status |
| `selection_fill` | `rgba(120,169,255,.14)` | Selected native list row |
| `modal_scrim` | `rgba(5,7,10,.55)` | Workspace behind a modal |

Terminal ANSI colors remain a separate `TerminalPalette`. UI chrome must not
derive its identity from terminal ANSI slots after the visual token migration.

Every built-in theme needs a coherent terminal palette and UI palette by phase
exit. `mandatum-light` must not remain light chrome around the dark default
terminal palette. `mandatum-high-contrast` must meet the stronger contrast
contract in `docs/verification.md`.

### Typography

Terminal and code retain the bundled JetBrains Mono primary.

| Context | Face | Size / line | Weight |
|---|---|---:|---|
| Terminal content | JetBrains Mono | 13 pt / 17 px | Regular, terminal-driven variants |
| Header and modal title | JetBrains Mono initially | 13 pt / 18 logical px | Bold |
| Pane title | JetBrains Mono | 12 pt / 17 logical px | Regular; bold when focused |
| Workflow body | JetBrains Mono initially | 12.5 pt / 18 logical px | Regular |
| Metadata and status | JetBrains Mono | 11 pt / 15 logical px | Regular |
| Key, path, timestamp | JetBrains Mono | 11 pt / 15 logical px | Regular |

The initial foundation may use bundled JetBrains Mono for every role to retain
deterministic loading and one text stack. A proportional native UI face is a
separate focused decision after the multi-metric text path exists; no unknown
system fallback may silently become the product font.

### Spacing And Geometry

Use a 4 logical-pixel base unit. Unless a verification step explicitly says
physical pixels, every visual dimension below is a logical pixel multiplied by
the live backing scale during materialization.

| Token | Value |
|---|---:|
| `space_1` | 4 px |
| `space_2` | 8 px |
| `space_3` | 12 px |
| `space_4` | 16 px |
| `space_6` | 24 px |
| Pane title horizontal inset | 8 px |
| Pane title vertical inset | 4 px |
| Workflow section gap | 12 px |
| Overlay outer padding | 16 px |
| Overlay row horizontal padding | 8 px |
| Overlay row vertical padding | 6 px |
| Visible tiled separator | 1 px |
| Separator pointer target | 6 px |
| Minimum explicit control target | 28 x 28 px |
| Viewport edge margin | 24 px minimum |

Tiled panes do not receive corner radius or shadow. Floating panes and major
overlays use a 10 px radius; context menus use 8 px. Raised surfaces use:

`0 18px 48px rgba(0,0,0,.45), 0 2px 8px rgba(0,0,0,.28)`

### Motion

| Interaction | Duration | Easing |
|---|---:|---|
| Press or hover | 80 ms | `cubic-bezier(.2,0,0,1)` |
| Focus or selection | 120 ms | `cubic-bezier(.2,0,0,1)` |
| Overlay enter | 180 ms | `cubic-bezier(.16,1,.3,1)` |
| Overlay exit | 110 ms | `cubic-bezier(.4,0,1,1)` |
| Programmatic pane change | 140 ms | `cubic-bezier(.2,0,0,1)` |
| Pointer drag, window resize, typing, output | 0 ms | Direct |

Overlay entry may animate opacity and scale from `0.985` to `1.0`. Pane
creation may fade without moving terminal content. No terminal glyph, cursor,
continuous output, or direct manipulation receives decorative interpolation.

Reduced motion jumps to the stable end state and schedules no animation
frames. Approval attention remains legible as a static semantic surface.

## Architectural Contract

The visual phase deepens existing boundaries; it does not introduce a UI
framework or second product model.

1. `mandatum-scene` owns semantic component meaning, layout, hit targets, and
   animation intent.
2. `CellProgram` remains the final-topmost cell projection for terminal parity.
3. Native may consume richer typed scene extensions in addition to
   `CellProgram`.
4. Add a pure, headless native presentation compiler that consumes only
   `WorkspaceScene + Theme` and emits ordered material primitives, clips, and
   scoped text layers.
5. The GPU adapter materializes that plan. It must not parse `detail_lines()`,
   inspect labels to rediscover state, or reconstruct product hierarchy.
6. The native shell supplies neutral viewport metrics: logical size, physical
   size, backing scale, and measured cell metrics. `mandatum-scene` resolves
   both cell geometry and logical-pixel presentation geometry from that input.
7. Typed logical-pixel rectangles and stable presentation-node identifiers are
   part of `WorkspaceScene`. Every pixel-space interactive surface carries a
   matching scene-owned logical hit target; native only translates OS pixels
   into neutral logical positions.
8. PTY content rectangles and cell-to-logical mapping are explicit scene data.
   A polished rail, inset, or control may not silently consume terminal rows or
   columns or move an input target away from the rendered pixels.
9. Existing terminal cells, PTY dimensions, selection, cursor, IME caret, and
   child mouse routing remain exact.
10. Native hover is limited to explicit workspace chrome and overlay hit
   targets. Pane-body hover must not interfere with child mouse reporting.
11. Window/GPU/text caches and animation progress are live presentation state
   and are never serialized as durable workspace intent.
12. No Metal/Swift fork, second text renderer, second state machine, or
    generalized damage system enters this phase.

Likely implementation seams:

- `crates/scene/src/theme.rs`: complete visual tokens and built-in themes;
- `crates/scene/src/workspace.rs` and `pane.rs`: typed presentation and
  workflow structures;
- `crates/app/src/scene_builder.rs`: state-to-scene composition;
- `crates/scene/src/cell_program/`: honest terminal fallback;
- `crates/native-renderer/src/gpu.rs`: pure native plan and material pipelines;
- `crates/native-renderer/src/row_run.rs`: multi-metric text scopes;
- `crates/native/src/main.rs`: window policy and bounded animation scheduling;
- a deferred native accessibility adapter under `crates/native` may eventually
  project the existing typed scene nodes and route actions through neutral
  input/commands;
- `crates/app/src/config.rs`: compatible theme and reduced-motion config; and
- `spikes/frontend-wgpu/`: fixed-reference capture and performance evidence.

## Component Specifications

### Workspace Shell

- Preserve the native macOS titlebar initially.
- Set an intentional first-launch and minimum usable window size.
- Make the title identify Mandatum and the active project.
- Render header and status as quiet native rails rather than visually dominant
  terminal rows.
- Header left: workspace and session. Header right: compact attention chips.
- Status left: durable status. Status right: one contextual route.
- Preserve the permanent live-keymap control hint defined by the interaction
  model unless a separate discoverability decision supersedes it.

### Panes And Focus

- Tiled panes share the canvas and use only 1 px separators.
- Pane titles occupy a shallow 24-28 px rail with kind, title, and typed badges.
- Focus uses `focus` title color, bold weight, and a 2 px leading or lower
  tick. Do not restore a loud full perimeter.
- Preserve a non-color focused cue; terminal fallback may retain the literal
  `focused` label.
- Hover raises only separator/control contrast. Drag uses `focus` and follows
  the pointer without interpolation.
- Floating panes receive raised material, 10 px radius, and strong boundary.

### Overlay Family

All eight overlays share material, type, spacing, selection, input, and footer
tokens, but they retain three interaction grammars:

- Palette, Timeline, Session Map, Prompt, Search, and Help are modal and receive
  a scrim plus raised shell.
- Welcome is non-modal first-run orientation: it receives a raised shell but no
  modal scrim, and ordinary non-Escape input still dismisses and continues.
- Context Menu is an anchored menu: it receives a raised shell and shadow but
  no viewport scrim.

| Surface | Maximum width | Height |
|---|---:|---:|
| Palette | 720 px | 60-70% viewport |
| Timeline and Search | 920 px | 72% viewport |
| Session map | 680 px | Up to 70% viewport |
| Help | 960 px | Up to 80% viewport |
| Prompt and Welcome | Content bounded | Content bounded |

- Selected rows use `selection_fill` and a 2 px leading focus indicator.
- Labels remain primary, details secondary, and key hints align right.
- Disabled commands remain visible with the typed reason.
- Timeline uses timestamp, state glyph, and content columns.
- Session map uses subtle tree rails and typed badges.
- Search groups results by source without relying on repeated blank strings.
- Context menu uses the same tokens at a smaller 8 px radius.

### Task Surface

- First row: typed status badge and command.
- Supporting metadata: working directory and optional recipe.
- Failure: contained failure callout with exit status and rerun route.
- Output: terminal-like console region with exact cell semantics.
- Never tint the entire pane red.

### Agent And Approval Surface

- Objective is the visual heading.
- Status and current action are compact structured rows.
- Latest summary is readable prose.
- Changed files are a distinct bounded list.
- Raw agent output remains a console region.
- Pending approval is the dominant amber callout and shows command, scope,
  risk label, risk basis, and approve/reject routes.
- Replace perpetual binary blinking with one brief arrival emphasis, then a
  stable high-salience state.

### Artifact Surface

- Ready pixels render on a neutral contain-fit canvas.
- Source, alt text, dimensions, state, and revision move into a compact
  inspector rail.
- Loading, failed, and ready states retain stable geometry.
- Transparency may use a subtle checkerboard only when present.
- Zoom, pan, copy path, and open externally are functional follow-ups, not
  prerequisites for the visual foundation.

## Ordered Capability Families

### Phase 1 — Visual Acceptance Contract And Scenario Catalog

Goal: make visual judgment reviewable before changing pixels.

Work:

- create one deterministic scenario catalog driven through real
  `FrontendHost` state and neutral input;
- write the accepted dual-geometry contract before implementation: neutral
  viewport metrics, scene-owned logical rectangles, stable presentation-node
  IDs, PTY content mapping, logical hit targets, terminal fallback, and typed
  accessibility nodes/actions;
- cover calm terminal, dense mixed panes, failed task, waiting approval,
  Palette, full modal, Welcome, context menu, artifacts, narrow geometry, and
  restore;
- capture the existing native surface on the fixed reference Mac;
- record font profile, theme, logical/physical size, scale, display, GPU,
  commit, and build;
- document intended hierarchy and component states for each canonical scene;
- add an explicit visual-baseline acceptance command that cannot run
  implicitly; and
- update the live-slice path so its displayed check launches native.

The reference artifact contract is:

- profile ID: `macbook-pro-metal-scale2`;
- fixed surface: 1600 x 1200 physical pixels, backing scale 2.0, expected
  102 x 35 scene, bundled JetBrains Mono 13;
- scenarios:
  `typography`, `calm-terminal`, `dense-workspace`, `attention`,
  `palette`, `full-modal`, `welcome`, `context-menu`, `artifacts`,
  `narrow`, and `restored`;
- storage:
  `spikes/frontend-wgpu/visual-baselines/<profile>/<scenario>/baseline.png`,
  `metadata.json`, and optional `mask.json`;
- capture interface:
  `spikes/frontend-wgpu/scripts/visual-regression.swift capture
  --profile <id> --scenario <id>`;
- comparison interface:
  `cargo run --manifest-path spikes/frontend-wgpu/Cargo.toml
  --bin visual-diff -- compare --profile <id> --scenario <id>`;
- acceptance interface:
  the same tool's `accept` command requires `--reason`, refuses a dirty
  candidate metadata record, and writes no files during `compare`; and
- mask rectangles use physical client-surface pixels and may cover only
  recorded OS-dependent fallback glyph regions.

ScreenCaptureKit output is color-managed compositor evidence. Exact sRGB token
values are verified through the pure resolver/native-plan tests, not by
demanding byte-identical compositor capture colors.

Do not redesign pixels in this phase.

Completion status (2026-07-24):

- the 11-scenario catalog now prepares durable fixtures through
  `mandatum-core`, drives the real `FrontendHost` with neutral input, and
  settles on typed semantic predicates;
- the catalog's aggregate real-host test compiles every final scene through
  the native renderer plan;
- `docs/architecture.md` freezes the Phase 2 dual-geometry, stable identity,
  terminal projection, logical hit-target, PTY mapping, and accessibility
  contract without wiring it into production pixels;
- the excluded lab accepts `--visual-scenario <id>` and presents a fixed,
  undecorated client surface for ScreenCaptureKit;
- `visual-regression.swift` fails closed unless the scenario window and
  captured frame are genuinely backing scale 2.0;
- `visual-diff compare` is read-only, and `visual-diff accept` requires a
  nonblank reason and refuses dirty candidate metadata;
- the live-slice displayed route launches `mandatum-native`; and
- all 11 fixed-reference images and metadata records were captured from clean
  source commit `ebd7ee4` on `external reference display` at genuine backing scale 2.0,
  1600 x 1200 physical / 800 x 600 logical / 102 x 35 scene, 60 Hz, Apple M4
  Pro / Metal, and bundled JetBrains Mono 13;
- visual review caught and corrected a lab startup-order defect that dismissed
  the context-menu scenario, then confirmed Typography selection/cursor,
  filtered/disabled/overflow Palette, and artifact overlay occlusion before
  acceptance;
- every baseline was explicitly accepted with the recorded reason
  `Initial Phase 1 fixed-reference baseline after visual review`; and
- strict comparison of every accepted baseline against its reviewed candidate
  returned SSIM 1.0, zero changed pixels, and zero masked pixels across
  1,920,000 compared pixels per scenario.

The canonical `narrow` baseline means narrow pane geometry inside the same
fixed 1600 x 1200 / scale-2 / 102 x 35 reference surface. Smaller-window
variants remain part of the later pairwise matrix and do not weaken the fixed
metadata contract.

Exit:

- canonical scenarios are generated from product state, not handcrafted
  renderer fixtures;
- the dual-geometry, stable-identity, terminal-fallback, and accessibility
  contracts are explicit enough for Phase 2 implementation without a renderer
  judgment call;
- existing baselines and metadata are captured;
- baseline update requires explicit human acceptance;
- focused catalog tests pass; and
- `./ci/gate.sh` is green after documentation synchronization.

### Phase 2 — Token And Native Presentation Foundation

Goal: establish a complete semantic visual language and pure translation seam.

Work:

- add UI color, typography, spacing, radius, elevation, opacity, selection,
  and motion tokens while retaining `TerminalPalette`;
- give the flagship dark theme real native values;
- add resolved contrast tests;
- extend config defaults, validation, aliases, and reload behavior;
- introduce a pure headless native presentation plan with ordered materials,
  stable semantic node IDs, typed transition targets, text scopes, clips,
  z-order, and resource ceilings;
- implement neutral viewport metrics, scene-owned logical-pixel geometry,
  cell/logical coordinate mapping, matching logical hit targets, and PTY
  content rectangles;
- implement multi-metric UI shaping, baseline alignment, and cache identity
  using only the provisioned Regular/Bold/Italic/BoldItalic faces; and
- define typed accessibility nodes/actions without claiming a native platform
  projection; and
- preserve existing `CellProgram` output byte-for-byte unless an intentional
  terminal fallback change is separately accepted.

Exit:

- no native UI chrome depends on terminal ANSI identity;
- app-owned dark-theme text/state contrast passes;
- native plan tests prove exact bounds, order, clipping, and resource limits;
- cell and logical hit targets match their rendered surfaces at 1.0 and 2.0
  scale;
- multi-metric text stays clipped and cache identity includes every metric
  generation;
- config compatibility is green; and
- displayed token sampler and full gate pass.

Implementation status (2026-07-24):

- `Theme` now keeps `TerminalPalette` intact while adding typed UI palette,
  typography, spacing, radius, elevation, opacity, selection, and motion
  contracts for dark, light, and high-contrast themes;
- app config supports validated `[theme.ui]` overrides, compatible aliases,
  reload/reset behavior, and explicit low-contrast warnings, while resolved
  built-in contrast tests pass;
- the native shell supplies one coherent `ViewportMetrics` snapshot and the
  scene owns deterministic 1/64-logical-pixel geometry, stable opaque node
  identity, terminal projections, logical hit targets, PTY viewport mappings,
  transition targets, and dependency-free accessibility semantics;
- app-built scenes populate that semantic contract without changing
  `CellProgram`; stable overlay item keys come from product identity rather
  than label, geometry, filtered position, or vector index;
- the renderer prepares a pure bounded native presentation plan with ordered
  materials, clips, text scopes, typed transitions, and exact direct UI-token
  colors before GPU allocation;
- multi-metric UI text has typed Regular/Bold/Italic/BoldItalic selection,
  shared-baseline/clipping checks, and cache identity that includes metric
  generation and slot;
- a displayed isolated token sampler showed all 17 direct UI color roles in
  the real native window without deriving them from terminal ANSI slots;
- portable scene, app, native-shell, renderer, and excluded-lab tests pass,
  `./ci/native-frontend.sh` passes, and the synchronized source gate is green;
  and
- the scenario harness now normalizes isolated fixture identity in its
  review-facing snapshot, preventing random temporary paths from making strict
  comparisons nondeterministic while leaving runtime isolation intact.

Fixed-reference completion evidence (2026-07-24):

- all 11 scenarios were captured from clean Phase 2 source commit `6979318` on
  the exact 1600 x 1200 physical / 800 x 600 logical / scale-2 / 60 Hz profile;
- visual review confirmed the intended product-content change was limited to
  replacing random fixture names and paths with `visual-project` and
  `$VISUAL_PROJECT`;
- the comparison exposed a Phase 1 evidence defect: its first four baselines
  were captured before 18:00 with the normal compositor color state, while the
  remaining seven abruptly shifted darker beginning three seconds after 18:00,
  consistent with the observed Zoom screen-share transition;
- the post-presentation Phase 2 set is internally color-consistent, and the
  acceptance rationale explicitly records both deterministic fixture
  normalization and, for the affected seven, correction of the external
  compositor-state contamination;
- every reviewed candidate was explicitly accepted, after which all 11 strict
  comparisons returned SSIM 1.0, zero changed pixels, and zero masked pixels
  across 1,920,000 compared pixels per scenario; and
- the LG display was restored to its 3440 x 1440 / scale-1 / 60 Hz mode and
  independently verified there.

### Phase 3 — Workspace Shell, Pane Materials, Density, And Focus

Goal: make the everyday multi-pane workspace feel coherent.

Work:

- implement canvas, pane, chrome, and separator materials;
- implement header/status rails and typed attention chips;
- implement pane title rails and typed badges;
- add compact focus treatment and separator hover/drag states;
- add floating-pane radius, boundary, and elevation;
- define initial/minimum window geometry and dynamic native title; and
- verify one, split, stacked, floating, zoomed, tiny, and restored layouts as
  one family.

Exit:

- PTY content geometry remains explicit and terminal input/hit targets match
  the scene-owned cell/logical mapping;
- focus is unmistakable without a loud perimeter or color-only meaning;
- no tiled pane reads as an independent card;
- narrow geometry degrades deliberately; and
- the displayed layout matrix, performance comparison, and full gate pass.

Completion status on 2026-07-24:

- the typed scene now distinguishes header/status rails, attention kinds,
  pane badges, compact focus, separator hover/drag, density, and floating
  state without parsing labels;
- the pure native plan maps those semantics to continuous tiled materials,
  one-logical-pixel separators, typed chips, and raised 10 px floating shells;
- native input preserves logical pointer coordinates and uses the explicit
  six-logical-pixel separator target while terminal input retains cell
  coordinates;
- compact and comfortable density change native rail breathing room without
  changing the terminal `CellProgram` or PTY geometry;
- the native shell owns a 1200 x 800 logical initial size, 720 x 480 minimum,
  and `Mandatum — <active project>` title; and
- focused source, parity, scenario-plan, renderer, and native-shell tests pass;
- the real native Metal route was reviewed at backing scale 2 through one,
  split, stacked, floating, zoomed, 720 x 480 minimum, and restored states;
- all 11 canonical candidates were individually reviewed and explicitly
  accepted from clean source commit `7221937`; and
- the repeated fresh performance series was explicitly scoped out once it
  stopped adding useful confidence. Existing reference preparation evidence,
  bounded native-plan tests, and the repository gate remain the regression
  guard.

Known physical debt: pane title rails still occupy one 17-logical-pixel text
row rather than the specified 24–28-pixel rail. This does not change PTY
geometry or invalidate the accepted shell, but it remains a visible density
improvement to revisit deliberately.

### Phase 4 — Overlay Family

Goal: make every modal and menu feel like one system.

Status: accepted and displayed on the MacBook Pro built-in Retina reference
display. The shared typed surface, scrim grammar, constrained geometry, bands,
soft selection, stable row identity, exact Context Menu hit geometry, and
right-aligned key hints are implemented across all eight overlays. The
representative `palette`, `full-modal`, `welcome`, and `artifacts` references
were reviewed and explicitly accepted from clean source commit `1988b0b`; the
complete repository gate reported `GATE GREEN`.

The finishing slice closes the overlay-spacing debt with centralized two-row
control blocks. Painted selection and logical hit geometry cover both rows,
producing disjoint targets of at least 28 logical pixels at the default and
18 pt smoke sizes without changing terminal viewport or PTY geometry.

Work:

- add modal scrim where appropriate plus the shared raised shell, shadow,
  radius, and title/input/footer bands;
- implement soft list selection and right-aligned key hints;
- apply max-width and edge-margin constraints;
- migrate Palette, Search, Timeline, Session Map, Help, Prompt, Welcome, and
  Context Menu together; and
- keep painted rows and hit targets on the same scene-owned geometry.

Exit:

- all overlays use the same token/component family;
- opaque overlay and later-pane clipping remain exact;
- disabled, selected, filtered, empty, and overflow states are covered;
- reduced and minimum sizes remain usable; and
- the overlay baseline matrix and full gate pass.

### Phase 5 — Typed Task, Agent, Approval, And Artifact Surfaces

Goal: express Mandatum's product-specific workflows natively.

Work:

- replace string-prefix styling with typed detail rows, badges, callouts, and
  console regions in `mandatum-scene`;
- keep `detail_lines()` or an equivalent formatter as the terminal fallback;
- implement task failure hierarchy and rerun affordance;
- implement agent objective, action, summary, changed files, and raw output;
- implement the stable approval callout;
- implement the artifact canvas and inspector; and
- verify loading, ready, failed, detached, blocked, running, waiting, complete,
  and long-content states.

Exit:

- native renderer never parses product strings;
- terminal fallback remains complete and accurate;
- approvals and failures are high-salience without flooding whole panes;
- artifact state changes do not shift layout; and
- workflow baselines and full gate pass.

Completion status (2026-07-24):

- `mandatum-scene` owns bounded typed workflow rows and compact status badges;
- task diagnostics, failures, rerun routes, agent objectives/actions/summaries,
  changed files, approvals, raw output, and artifact inspector state cross the
  native boundary without parsing display strings;
- offscreen workflow nodes retain stable semantic identity across resize;
- ready artifact pixels require the exact typed canvas geometry, while
  loading, ready, and failed states retain one stable layout;
- raw agent output is bounded at ingestion and again at scene projection;
- the real-host canonical matrix asserts the Phase 5 scene roles and native
  material plan; and
- `dense-workspace`, `attention`, and `artifacts` were reviewed and explicitly
  accepted on the MacBook Pro built-in Retina reference from clean source
  commit `d17bdd2`; strict comparisons returned SSIM 1.0, zero changed pixels,
  and zero masked pixels.

### Phase 6 — Motion And Fluid Geometry

Goal: clarify change without adding visual noise or input risk.

Status: accepted; representative displayed motion evidence and explicit
reference acceptance are complete. The final synchronized gate is recorded in
`docs/verification.md`.

Work:

- add typed animation intent and an injectable visual clock;
- implement focus, selection, overlay, and programmatic pane transitions;
- replace the binary approval blink with brief arrival emphasis;
- separate animation deadlines from the child-exit heartbeat;
- schedule redraw only during active motion or real scene change;
- keep pointer drag, live window resize, typing, and output direct; and
- implement complete reduced-motion behavior.

Exit:

- start, midpoint, end, interruption, reversal, and convergence are
  deterministic tests;
- reduced motion schedules no transition frames;
- static workspaces produce no animation-driven redraw;
- frame pacing and idle budgets pass; and
- the motion matrix and full gate pass.

Implementation status (2026-07-24):

- `ScenePresentation` carries a live `SceneMotionPolicy` and typed transition
  targets for Focus, Selection, Overlay, PaneGeometry, and ApprovalArrival;
- the scene builder targets coherent overlay opacity families and
  material-backed scale/pane-geometry families while cell-owned glyph
  placement, terminal/task output, and artifact raster placement remain
  direct;
- `mandatum-native-renderer` owns deterministic adapter-local interpolation,
  accepts an injected monotonic instant, applies the configured typed timing
  tokens, and converges interrupted or reversed motion on current product
  truth;
- pointer drag and live resize mark authoritative direct geometry; typing and
  child output update the stable scene without decorative interpolation;
- overlay entry moves the complete typed material family; close is direct
  because scene-owned overlay glyph rows no longer exist and retaining an empty
  shell would be incoherent;
- reduced motion omits transition targets, snaps any retained presentation
  progress, and schedules no transition frames;
- pending approval remains a stable high-salience semantic callout while each
  newly arriving request receives a monotonic typed sequence and one brief
  inward emphasis, including back-to-back requests on the same pane;
- `FrontendHost` exposes a monotonic scene generation and a heartbeat that
  reports whether child-exit work changed visible product state;
- the native shell schedules animation deadlines independently from the
  250 ms child-exit heartbeat, redraws for active motion or a changed scene
  generation, and keeps hit targets at the stable end-state geometry while
  pane or overlay visuals glide between endpoints; and
- focused deterministic source tests cover start, midpoint, end, interruption,
  reversal, convergence, direct/reduced policy, redraw, and idle behavior;
- `attention-motion-start`, `attention-motion-midpoint`,
  `attention-motion-end`, and `attention-reduced` were captured from clean
  source commit `4732ba8` on the MacBook Pro built-in Retina reference,
  visually reviewed, and explicitly accepted with exact post-accept
  comparisons;
- the three-run median motion p95 was 1.19 display periods and the median
  fraction over two periods was 0.34%; and
- the three-run median static idle window used 0.067% of one core with zero
  redraws and zero presents.

### Finishing Slice — Physical Theme, Metrics, And Controls

Goal: close the remaining visible native gaps without turning regression tools
into a second acceptance product.

Implementation:

- complete light and high-contrast UI and terminal palettes and validate the
  app-owned painted contrast pairs;
- project typed UI roles through their own point size, line height, face, and
  proportional advance while terminal output retains exact terminal metrics;
- centralize two-row overlay control geometry so painted rows and logical hit
  targets remain aligned and at least 28 logical pixels tall;
- preserve selection-aware overlay windows, keyboard routing, non-color state
  cues, terminal viewport geometry, and prompt IME behavior;
- review a dense native scene at 18 pt without clipped chrome; and
- retain the one-row pane title rail as explicit bounded debt because a taller
  rail requires an intentional PTY/layout contract.

Typed accessibility nodes and actions remain renderer-neutral scene truth. A
native macOS accessibility adapter and VoiceOver qualification are explicitly
deferred; this slice does not claim platform projection that does not exist.

Close:

- review representative dark, light, and high-contrast displayed surfaces;
- run focused semantic, interaction, rendering, and contrast checks for the
  seams changed here;
- run one aggregate architecture/interaction/rendering review;
- synchronize the active plans, implementation docs, verification record, and
  project handoff;
- run the authoritative `./ci/gate.sh`; and
- commit the complete synchronized slice.

### Retired Phase 8 Ceremony

There is no standalone Phase 8. The former complete pairwise matrix, repeated
unchanged performance/stress procedures, and rollout-style admission sequence
were retired because they duplicated existing regression authority without
improving the product. Resize, scale, recovery, motion, frame, and idle probes
remain available and should run when their owning seam changes or focused
evidence exposes risk.

## Verification Authority

`docs/verification.md#visual-polish-verification` owns the standing visual
procedure, contrast thresholds, fixed-reference baseline policy, motion
checks, and performance budgets. Its scenario and probe catalog is a
risk-driven regression toolbox, not a requirement to rerun every permutation
after an unrelated change.

For every rendered capability family:

1. run focused semantic and native-plan tests;
2. run the representative displayed scenarios;
3. accept intentional baseline changes explicitly;
4. run only additional regression probes owned by the changed seam;
5. synchronize active docs and handoff;
6. run `./ci/gate.sh` after the final documentation changes;
7. inspect `git diff --check` and `git status --short`; and
8. commit the complete family.

Screenshots do not overrule semantic, contrast, clipping, or hit-target
failures. CI never regenerates baselines.

## Optional Later Flourish

Defer until the complete system above succeeds without them:

- macOS vibrancy or background blur;
- custom titlebar and traffic-light integration;
- gradients, texture, glow, or animated branding;
- bespoke icons beyond a small functional set;
- onboarding illustration or splash screen;
- sound;
- a theme marketplace or user-authored motion system; and
- marketing screenshots or public-repository redesign.

## Immediate Next Action

Begin the named task and dev-server recipe catalog in `PLAN.md`. Do not pull
native VoiceOver qualification, installer, release, rollout, or public
presentation work forward.
