# Decision Log

Use this file for durable architectural decisions once `/plan` begins.

Each entry should use this shape:

```text
## YYYY-MM-DD: Decision Title

Status: Proposed | Accepted | Rejected | Superseded

Decision:

Context:

Options Considered:

Rationale:

Consequences:

Verification:
```

## 2026-06-25: Greenfield Product Boundary

Status: Accepted

Decision:

This repo is a greenfield terminal-native workspace. It should not reuse an existing Aetherspace code path or become an IDE-first product.

Context:

The product is meant to transfer the idea of a developer command workspace onto a native, high-quality terminal layer, closer to tmux/zellij/Ghostty than VS Code.

Options Considered:

- Continue from an existing TUI implementation.
- Fork Ghostty and build product features inside it.
- Start greenfield with terminal substrate evaluation behind an adapter.

Rationale:

The durable product idea is the workspace model, not the prior runtime implementation. Forking a terminal emulator too early would shift effort toward terminal maintenance instead of developer-workspace design.

Consequences:

- Early work is docs and architecture first.
- Core state must stay renderer-neutral.
- Terminal parser choice is deferred behind `terminal-vt`.
- No existing Aetherspace code should be copied into the repo.

Verification:

- `AGENTS.md` states the greenfield rule.
- Architecture docs define separate core, PTY, terminal-vt, renderer, app, commands, and workflows layers.

## 2026-06-25: Terminal/Codex Build Constraint

Status: Accepted

Decision:

This repo must be buildable, testable, and runnable from terminal commands under Codex. Xcode.app, `.xcodeproj`, SwiftUI, AppKit, Metal, MetalKit, CoreText renderer work, and Apple-native GUI app surfaces are out of scope.

Context:

The product is intended to be developed through terminal/Codex workflows rather than Apple IDE or GUI-app tooling. MacBook-only remains acceptable as an operating environment, but not as a reason to adopt Apple-native app frameworks.

Options Considered:

- Swift/AppKit/Metal native Mac app.
- Zig-first systems app.
- Rust-first terminal workspace.

Rationale:

Rust gives the best balance for command-line verification, PTY/event-loop work, terminal UI ecosystem, and Codex-friendly incremental development. Zig remains useful only if a later terminal parser or libghostty adapter spike justifies it.

Consequences:

- Use Rust as the default implementation stack.
- Treat terminal rendering as the first product surface.
- Do not create Apple project files or native GUI surfaces.
- Keep libghostty-vt behind a terminal adapter and defer it until after core and fake parser seams exist.

Verification:

- `docs/technology-direction.md` states the prohibited stack and Rust-first recommendation.
- `PLAN.md` and `docs/codex-goal.md` instruct Codex to avoid Apple-native GUI tooling.

## 2026-06-25: Rust Core-First Milestone 1

Status: Accepted

Decision:

Use a Cargo workspace for Milestone 1. Implement only the renderer-neutral domain in `crates/core`, minimal command metadata/dispatch in `crates/commands`, durable task/agent intent helpers in `crates/workflows`, and compile-only placeholder boundaries for `crates/pty`, `crates/terminal-vt`, `crates/renderer`, and `crates/app`.

Context:

The accepted plan calls for the smallest useful implementation foundation: deterministic workspace/session/layout/pane/action state and persistence before any PTY, parser, renderer, or app runtime work.

Options Considered:

- Build a runnable terminal app shell immediately.
- Start with PTY/parser integration.
- Start with renderer and command palette UI.
- Start with pure core state and command dispatch.

Rationale:

Core state can be tested without terminal UI, avoids early coupling to parser or renderer choices, and provides the durable contract that later runtime crates must respect.

Consequences:

- `core` owns workspace, project, session, panes, layout tree, focus, zoom, split, stack, floating panes, restart/rename intent, action results, and session persistence.
- `commands` maps command ids to core actions but does not mutate layout state directly.
- `workflows` does not launch tasks or agents in Milestone 1.
- `pty`, `terminal-vt`, `renderer`, and `app` compile but contain no runtime implementation yet.

Verification:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- Boundary check: `core` imports no PTY, terminal parser, renderer, app runtime, or terminal UI crates.

## 2026-06-25: JSON Session Persistence

Status: Accepted

Decision:

Use JSON for the first durable session persistence format, wrapped in a versioned schema field.

Context:

Milestone 1 needs persistence that is transparent, easy to inspect in tests, and sufficient for durable workspace intent without migration machinery.

Options Considered:

- JSON
- TOML
- SQLite
- Custom binary schema

Rationale:

JSON keeps the first schema simple and verifiable. The versioned wrapper gives later milestones a migration point without pulling in database or config-format decisions too early.

Consequences:

- Persist workspace/project/session/pane/layout/focus/task/agent intent.
- Do not persist PTY handles, parser state, process ids, renderer state, thread handles, or unbounded scrollback.
- Return structured errors for corrupt JSON, unsupported schema versions, and invalid session state.

Verification:

- Unit tests cover serialize/deserialize, unsupported schema, corrupt JSON, invalid session data, and runtime-handle exclusion strings.
