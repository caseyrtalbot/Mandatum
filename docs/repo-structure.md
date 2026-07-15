# Repository Structure

## Root

```text
README.md      product entrypoint
AGENTS.md      agent operating contract
PLAN.md        shipped charter summary + forward horizon
CONTRIBUTING.md contributor contract (the gate is the review)
SECURITY.md    private vulnerability reporting + scope notes
LICENSE        Apache-2.0
Cargo.toml     Rust workspace manifest (excludes spikes/frontend-wgpu)
Cargo.lock     locked Rust dependencies
rust-toolchain.toml  pinned gate toolchain
install.sh     latest-release installer (checksum verification + both binaries)
ci/            the merge gate: gate.sh, conformance.sh, doc-trace.sh
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
docs/roadmap.md             executed gates + forward horizon
docs/verification.md        proof commands, scans, and quality gates
docs/repo-structure.md      current file layout
docs/decisions.md           decision log (append-only)
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

- `app_shell.rs`: terminal lifecycle, event-driven run loop, renderer handoff
- `app_state.rs`: command dispatch, event application, runtime reconciliation
- `app_state/tests.rs`: private app-state unit and live-PTY tests
- `events.rs`: the unified app event channel (input / PTY / agent)
- `frontend.rs`: crossterm-to-neutral input translation (the only module
  besides `app_shell.rs` allowed to name crossterm)
- `input.rs`: neutral input routing to runtime intents
- `terminal_runtime.rs` / `task_runtime.rs` / `agent_runtime.rs`: live
  runtime registries (generation + token event stamping)
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
                        harness; RESULTS.md is the evidence record
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
