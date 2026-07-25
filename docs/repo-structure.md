# Repository Structure

## Root

```text
README.md      product entrypoint
AGENTS.md      agent operating contract
PLAN.md        current direction + ordered forward work
CONTRIBUTING.md contributor contract (the gate is the review)
SECURITY.md    private vulnerability reporting + scope notes
LICENSE        Apache-2.0
Cargo.toml     Rust workspace manifest + shared release version
Cargo.lock     locked Rust dependencies
rust-toolchain.toml  pinned gate toolchain
install.sh     checksum-verifying installer for Mandatum.app + the mandatum launcher
packaging/     Info.plist template, app icon + generator, launcher script,
               the Braille fallback font generator, and the Mandatum.app
               assembly/signing script
ci/            merge gate, distribution smoke, and native frontend maintenance
.github/       GitHub Actions CI + the universal Mandatum.app release pipeline,
               Dependabot config, issue and PR templates
docs/          product and architecture specs
docs/assets/   README screenshots captured from the running app
crates/        implementation modules
examples/      live-slice driven demo (the stranger-test scene)
spikes/        excluded measurement, stress, fault, and terminal probes
.agents/       repo-local agent skills
```

## Docs

```text
docs/constitution.md        the five laws and their executable gates
docs/product-principles.md  product thesis and quality bar
docs/architecture.md        engine, runtime, scene, and frontend responsibilities
docs/frontend-platform.md   native product + maintained terminal roles
docs/rendering-strategy.md  scene and visual performance strategy
docs/terminal-engine.md     terminal parser/grid/backend strategy
docs/agent-runtime.md       agent actor model and runtime surface
docs/interaction-model.md   commands, panes, session map, timeline, input
docs/workflows.md           end-to-end developer workflows (built vs not yet)
docs/native-gpu-implementation-plan.md
                            ordered native-first implementation plan
docs/visual-polish-plan.md ordered in-app visual system and acceptance plan
docs/verification.md        standing procedures + dated evidence ledger
docs/repo-structure.md      current file layout
docs/decisions.md           decision log (append-only)
docs/history/               dated evidence and superseded closure records
```

## Crates

Workspace members: `core`, `commands`, `pty`, `terminal-vt`, `scene`,
`agent-runtime`, `renderer`, `app`, `workflows`, `native`, and
`native-renderer`.

### `crates/core`

Durable workstation model (deps: serde + serde_json only, frozen by the L2
gate): workspaces, projects, sessions, panes, layouts, focus, actions,
persistence schema.

### `crates/commands`

Command vocabulary and routing: `CommandId`s with kebab-case names, labels,
categories, and default palette letters (`BUILT_IN_COMMANDS`); palette key
resolution with context substitutions; core/runtime command targets; the
fuzzy subsequence scorer (`fuzzy.rs`) shared by palette, timeline, and
search filtering.

### `crates/pty`

PTY process mechanics: spawn intent, native PTY session,
reader/writer/controller split, resize, input/output, child exit,
termination.

### `crates/terminal-vt`

Terminal engine: the `TerminalAdapter` interface, the vte-backed default
parser (`vte_backend.rs`), a deterministic fake backend (`fake.rs`),
terminal grid with bounded scrollback, cursor, cell styles, mouse-mode
exposure, snapshots. `[L4-GATE]` conformance tests live in
`crates/terminal-vt/tests/`.

### `crates/scene`

`mandatum-scene`: the renderer-neutral frontend contract. `WorkspaceScene`
output model (geometry, pane content, terminal and bounded artifact surfaces,
overlays, transient text composition,
header/status, hit targets), its final-topmost whole-frame cell compiler
(`cell_program.rs` plus private `cell_program/` modules, including
`text_input.rs`), all pane-rect layout
math (`layout.rs`), semantic themes (`theme.rs`), and the neutral input event
types (`input.rs`). Engine-side: deps are `mandatum-core`, serde, and pure
Unicode segmentation/width policy crates (L1 gate).

### `crates/agent-runtime`

`mandatum-agent-runtime`: the agent connector contract (`connector.rs`,
`spec.rs`, `events.rs`, `approval.rs`), the deterministic `FakeConnector`
(`fake.rs`), the Claude CLI connector (`claude/`), and the approval bridge
binary (`bin/mandatum-approval-bridge.rs`) with its socket protocol
(`bridge_protocol.rs`). Engine-side: deps are `mandatum-core`, serde,
serde_json (L1 gate).

### `crates/renderer`

The ratatui terminal frontend adapter. Translates the scene compiler's neutral
`CellProgram` into ratatui buffer cells; computes no layout/presentation rules
and has no terminal-engine dependency (banned by the L1 gate).

### `crates/native-renderer`

`mandatum-native-renderer`: scene-only wgpu/glyphon presentation. It owns GPU
surface/device recovery, bundled/static font provisioning, bounded fallback
diagnostics, terminal-palette materialization, the pure bounded native
presentation plan, direct UI-token materials, multi-metric interface text,
clipped row-run shaping, text and raster resource bounds, a
generation-and-metric-aware admitted-run shaping cache, and frame
preparation/stage timing. Vendored JetBrains Mono
faces, OFL, and provenance live under `assets/fonts/`, alongside the
generated `mandatum-braille/` fallback face (regenerated by
`packaging/make-braille-font.py`). Its only internal
workspace dependency is `mandatum-scene`; synthetic fault injection is
feature-gated for the excluded lab.

### `crates/native`

`mandatum-native`: the production winit shell over `FrontendHost`. It owns GPU
preflight ordering, native input/IME/pointer/clipboard translation, strict
font-family/font-size options plus headless `--font-info`, scale/surface
coordination, bounded event draining, redraw scheduling, renderer recovery,
and clean shutdown. Its internal dependencies are frozen to `mandatum-app`,
`mandatum-scene`, and `mandatum-native-renderer`.

### `crates/app`

The shared workstation runtime and maintained terminal shell:

- `app_shell.rs`: crossterm/terminal lifecycle, input-reader lifecycle,
  heartbeat/redraw scheduling, renderer handoff, and terminal effect encoding;
  drives `FrontendHost` for workstation behavior
- `frontend_host.rs`: exported frontend-neutral owner of one private
  `AppState`; blocking/bounded event consumption, heartbeat work, owned
  `FrameSnapshot` scene/theme/revision values, FIFO effects, quit, and
  idempotent shutdown; optional neutral wake-callback installation used by the
  current winit shell
- `app_state.rs`: command dispatch plus durable workspace, timeline, status,
  and presentation folds over typed runtime effects
- `app_state/tests.rs`: private app-state unit and live-PTY tests
- `runtime_engine.rs`: deep live-runtime Module over terminal, task, and agent
  registries; owns the event channel, identity, reconciliation, replacement,
  approval control, shutdown, and transactional restore lifecycle facts
- `artifact_preview.rs`: project-relative observation, descriptor-relative
  no-follow opening on Unix hosts, bounded PNG header/decode, aggregate
  reservation/worker queue, reload, and live RGBA8 cache
- `events.rs`: the unified app event ingress with priority input and bounded
  runtime lanes (input / PTY / agent / artifact)
  plus the
  app-owned sender that coalesces optional frontend wakes without replacing
  channel truth
- `frontend.rs`: crossterm-to-neutral input translation (the only module
  besides `app_shell.rs` allowed to name crossterm)
- `frontend_effect.rs`: renderer-neutral platform effects; terminal/native
  shells provide their concrete clipboard integration
- `input.rs`: neutral input routing to runtime intents
- `terminal_runtime.rs` / `task_runtime.rs` / `agent_runtime.rs`: low-level live
  runtime registry Implementations behind `RuntimeEngine` (generation + token
  event stamping)
- `process_events.rs`: PTY reader threads and flow-credit backpressure
- `persistence.rs`: workspace file persistence coordinator
- `config.rs`: config loading/validation and effective runtime-setting
  resolution; `keymap.rs`: remappable keymap
- `palette.rs`: fuzzy command palette model
- `scene_builder.rs`: builds the per-frame `WorkspaceScene` from app state
- `attention.rs`: header attention strip aggregation
- `session_map.rs`, `timeline.rs`, `timeline_view.rs`, `search.rs`,
  `help.rs`: the visibility overlays and the durable JSONL timeline
- `copy_mode.rs`, `pointer.rs`, `clipboard.rs`: selection, pointer routing,
  OSC 52
- `tests/frontend_parity.rs`: cross-frontend scene parity;
  `tests/terminal_smoke.rs`: live PTY smoke;
  `tests/distribution.rs`: public executable and non-interactive CLI contract

### `crates/workflows`

Durable workflow intent and cross-actor handoff policy: `TaskRecipe` and
`AgentThreadSpec` shape pane intent for `mandatum-core`;
`TaskFailureHandoff` bounds, JSON-escapes, prefixes, and labels every
failed-task fact before creating an agent mandate. No runtime launching, no
history (see docs/workflows.md for what remains unbuilt here).

## Spikes And Examples

```text
spikes/frontend-wgpu/   excluded mandatum-native-lab measurement, stress,
                        fault, and deterministic visual-scenario harness;
                        ScreenCaptureKit capture, explicit visual-diff
                        acceptance, tracked fixed-reference visual baselines,
                        terminal latency, typography-corpus tools, and frozen
                        RESULTS.md
examples/live-slice/    driven native demo workspace for the stranger test
```

## Repo-Local Skills

```text
.agents/skills/product-architect/
.agents/skills/interaction-reviewer/
.agents/skills/rendering-spike/
.agents/skills/terminal-conformance/
```

These skills should point to the current spec set and avoid hidden product
constraints.
