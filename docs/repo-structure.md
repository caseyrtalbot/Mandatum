# Repository Structure

## Root

```text
README.md      product entrypoint
AGENTS.md      agent operating contract
PLAN.md        shipped charter summary + forward horizon
CONTRIBUTING.md contributor contract (the gate is the review)
SECURITY.md    private vulnerability reporting + scope notes
LICENSE        Apache-2.0
Cargo.toml     Rust workspace manifest + shared release version
Cargo.lock     locked Rust dependencies
rust-toolchain.toml  pinned gate toolchain
install.sh     latest-release installer (checksum verification + both binaries)
ci/            the merge gate plus gpu-spike.sh opt-in maintenance check
.github/       GitHub Actions CI + tag-driven native release archives,
               Dependabot config, issue and PR templates
docs/          product and architecture specs
docs/assets/   README frames: SVGs generated from real captured sessions
crates/        implementation modules
examples/      live-slice driven demo (the stranger-test scene)
spikes/        frontend-wgpu GPU spike (outside the workspace)
.agents/       repo-local agent skills
```

## Docs

```text
docs/constitution.md        the five laws and their executable gates
docs/product-principles.md  product thesis and quality bar
docs/architecture.md        engine, runtime, scene, and frontend responsibilities
docs/frontend-platform.md   frontend strategy + GPU spike decision record
docs/rendering-strategy.md  scene and visual performance strategy
docs/terminal-engine.md     terminal parser/grid/backend strategy
docs/agent-runtime.md       agent actor model and runtime surface
docs/interaction-model.md   commands, panes, session map, timeline, input
docs/workflows.md           end-to-end developer workflows (built vs not yet)
docs/native-gpu-implementation-plan.md
                            admission-gated path to a native GPU frontend
docs/verification.md        proof commands, scans, and quality gates
docs/repo-structure.md      current file layout
docs/decisions.md           decision log (append-only)
docs/history/               dated evidence and superseded closure records
```

## Crates

Workspace members: `core`, `commands`, `pty`, `terminal-vt`, `scene`,
`agent-runtime`, `renderer`, `app`, `workflows`.

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
output model (geometry, pane content, terminal surfaces, overlays,
header/status, hit targets), all pane-rect layout math (`layout.rs`),
semantic themes (`theme.rs`), and the neutral input event types
(`input.rs`). Engine-side: deps are `mandatum-core` + serde only (L1 gate).

### `crates/agent-runtime`

`mandatum-agent-runtime`: the agent connector contract (`connector.rs`,
`spec.rs`, `events.rs`, `approval.rs`), the deterministic `FakeConnector`
(`fake.rs`), the Claude CLI connector (`claude/`), and the approval bridge
binary (`bin/mandatum-approval-bridge.rs`) with its socket protocol
(`bridge_protocol.rs`). Engine-side: deps are `mandatum-core`, serde,
serde_json (L1 gate).

### `crates/renderer`

The ratatui terminal frontend adapter. Renders a `WorkspaceScene`; computes
no layout and has no terminal-engine dependency (banned by the L1 gate).

### `crates/app`

The terminal app runtime:

- `app_shell.rs`: crossterm/terminal lifecycle, input-reader lifecycle,
  heartbeat/redraw scheduling, renderer handoff, and terminal effect encoding;
  drives `FrontendHost` for workstation behavior
- `frontend_host.rs`: exported frontend-neutral owner of one private
  `AppState`; blocking/bounded event consumption, heartbeat work, owned
  `FrameSnapshot` scene/theme/revision values, FIFO effects, quit, and
  idempotent shutdown; optional neutral wake-callback installation
- `app_state.rs`: command dispatch plus durable workspace, timeline, status,
  and presentation folds over typed runtime effects
- `app_state/tests.rs`: private app-state unit and live-PTY tests
- `runtime_engine.rs`: deep live-runtime Module over terminal, task, and agent
  registries; owns the event channel, identity, reconciliation, replacement,
  approval control, shutdown, and transactional restore lifecycle facts
- `events.rs`: the unified app event channel (input / PTY / agent) plus the
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
spikes/frontend-wgpu/   winit+wgpu GPU frontend spike + tui_probe latency
                        harness; its gpu-renderer/ member is a scene-only paint
                        crate; RESULTS.md is the evidence record
examples/live-slice/    driven demo workspace for the stranger test
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
