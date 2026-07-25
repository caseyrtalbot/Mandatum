# Native Visual Polish Plan

Status: accepted direction; Phase 1 implementation in progress (2026-07-24)

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
- native accessibility semantics; and
- maintained visual-regression evidence.

This phase does not include installer, updater, release, rollout, archives,
public-GitHub presentation, marketing screenshots, or other public-distribution
work. Those remain a separate deferred phase.

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
- a native accessibility adapter under `crates/native` that projects typed
  scene accessibility nodes and routes actions through neutral input/commands;
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
  IDs, PTY content mapping, logical hit targets, terminal fallback, and
  accessibility-node/action projection;
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

- profile ID: `casey-m4pro-metal-scale2`;
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

Implementation status (2026-07-24):

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
- fixed-reference baseline images remain pending because the only active
  display during this implementation run was `LG ULTRAGEAR+` at backing scale
  1.0. Upscaling that capture is not accepted evidence.

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
- define typed accessibility nodes/actions even though the native projection
  lands in Phase 7; and
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

### Phase 4 — Overlay Family

Goal: make every modal and menu feel like one system.

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

### Phase 6 — Motion And Fluid Geometry

Goal: clarify change without adding visual noise or input risk.

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

### Phase 7 — Theme And Accessibility Completion

Goal: make the visual system coherent beyond the flagship dark surface.

Work:

- complete light and high-contrast UI and terminal palettes;
- validate all app-owned contrast pairs;
- add native accessibility roles, labels, values, focus, and hierarchy for
  panes, overlays, attention chips, rows, and approval actions;
- verify keyboard-only operation and non-color state cues;
- verify native font scaling without clipped chrome;
- verify representative color-vision distinctions; and
- confirm minimum 28 px explicit pointer targets.

Exit:

- every built-in is a complete coherent theme;
- high-contrast text reaches the documented 7:1 contract;
- every interactive/state surface has a semantic native description;
- font scaling, focus, keyboard, and reduced-motion checks pass; and
- the accessibility/theme matrix and full gate pass.

### Phase 8 — Aggregate Acceptance And Phase Close

Goal: prove the visual system as one production capability.

Work:

- run the complete pairwise scenario matrix on Casey's reference Mac;
- explicitly review and accept intentional baseline changes;
- run scale 1.0 and 2.0, resize/scale stress, recovery, continuous output,
  motion pacing, startup, frame preparation, and idle procedures;
- run an aggregate architecture/interaction/rendering review;
- correct findings and rerun affected evidence;
- synchronize `PLAN.md`, decisions, rendering/interaction docs, verification,
  README, repo structure, and the project handoff; and
- run the final `./ci/gate.sh`.

Exit:

- text is delightful at Casey's normal settings;
- layout, materials, hierarchy, focus, overlays, and workflow surfaces read as
  one system;
- keyboard, pointer, clipboard, IME, terminal semantics, and recovery remain
  trustworthy;
- the accepted pixel baselines and portable semantic gates agree;
- performance and idle budgets pass or have an explicit accepted decision;
- no known blocking visual defect remains; and
- the synchronized capability family is committed.

## Verification Authority

`docs/verification.md#visual-polish-verification` owns the standing visual
procedure, pairwise matrix, contrast thresholds, fixed-reference baseline
policy, motion checks, and performance budgets.

For every rendered capability family:

1. run focused semantic and native-plan tests;
2. run the representative displayed scenarios;
3. accept intentional baseline changes explicitly;
4. run `./ci/native-frontend.sh`;
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

Implement Phase 1 only: create the deterministic visual-scenario catalog,
capture the current fixed-reference native baselines and metadata, and add the
explicit human baseline-acceptance path. Do not alter production rendering,
theme values, pane materials, overlay styling, workflow presentation, or
motion in that slice.
