# AGENTS.md

## Product Contract

Mandatum is a development workstation with a terminal soul and IDE-grade session
visibility. It coordinates shells, editors, builds, tests, logs, agents, diffs,
approvals, and recovery surfaces inside a modular workspace.

The product must feel fast, spatial, commandable, inspectable, recoverable, and
beautiful under real development load.

Agents should reason from the current spec set and the code on disk. Product
docs should describe the current target state directly.

## Source Of Truth

Read these before planning product, architecture, or documentation changes:

1. `PLAN.md`
2. `docs/product-principles.md`
3. `docs/architecture.md`
4. `docs/frontend-platform.md`
5. `docs/rendering-strategy.md`
6. `docs/terminal-engine.md`
7. `docs/agent-runtime.md`
8. `docs/interaction-model.md`
9. `docs/workflows.md`
10. `docs/roadmap.md`
11. `docs/verification.md`
12. `docs/repo-structure.md`
13. `docs/decisions.md`

## Architecture Rules

Keep product behavior behind engine and scene interfaces.

- `core` owns durable workspace, session, pane, layout, action, and persistence
  intent. It must not depend on runtime handles, parser objects, renderer
  resources, frontend frameworks, or platform UI types.
- `pty` owns process lifecycle, PTY I/O, resize, exit, termination, and byte
  events. It does not know how output is drawn.
- `terminal-vt` owns terminal parser adapters, terminal grid state, style
  snapshots, scrollback, cursor state, and terminal capabilities.
- `commands` owns command metadata, palette routing, key resolution, and the
  split between durable core actions and runtime commands.
- `workflows` owns task recipes, build/test/dev-server intent, agent launch
  intent, and workflow metadata.
- Runtime modules own live process handles, reader threads, runtime tokens,
  parser instances, launch state, and failure state.
- The scene layer owns renderer-neutral presentation data: panes, bounds,
  terminal snapshots, overlays, hit targets, selections, status surfaces, and
  animation intent.
- Frontend adapters render a scene and report input. They must not own product
  behavior.

## Frontend Rules

Mandatum can have more than one frontend adapter. The current terminal frontend
is useful, but it is not the whole product architecture.

Frontend work must preserve these rules:

- Product logic stays in the engine.
- Platform code stays behind an adapter.
- Rendering code receives scene data and emits input/hit-test events.
- A native or GPU-backed frontend may be introduced when it improves latency,
  text quality, animation, pointer precision, accessibility, or platform fit.
- A terminal frontend remains valuable for fast local verification and remote
  sessions.

## Product Surface Rules

Build the actual workstation surface first:

- live terminal panes
- build/test/dev-server task panes
- agent status panes
- command palette
- session map
- searchable execution history
- failure and approval surfaces
- restore/recovery state

Avoid landing pages, dashboards that hide raw output, decorative cards,
chat-first layouts, and product behavior embedded in renderer code.

## Agent Runtime Rules

Agent panes are session actors, not chat sidebars.

Each agent surface should expose:

- objective
- current state
- latest action
- pending approvals
- changed files
- commands run
- verification results
- blockers
- handoff summary

## Documentation Rules

Docs are product source of truth. When editing them:

- keep only current direction
- remove contradictory instructions
- remove references to missing files
- avoid background-story paragraphs
- keep spec files specific enough for future agents to act without guessing
- update `README.md`, `PLAN.md`, and `docs/repo-structure.md` when the doc set changes

## Verification

Before claiming completion:

- run documentation trace scans relevant to the change
- run `cargo fmt --check` if Rust files changed
- run `cargo clippy --all-targets -- -D warnings` if Rust files changed
- run `cargo test` when behavior or contracts changed
- run `git diff --check`
- report any commands not run

## Done Means

A task is done only when the artifact exists on disk, source-of-truth docs match
the artifact, verification has been run or explicitly scoped out, and remaining
risks are named.
