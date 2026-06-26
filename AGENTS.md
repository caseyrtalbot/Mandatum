# AGENTS.md

## Product Contract

Mandatum is a greenfield terminal-native workspace for developers. It is closer to tmux and zellij than to VS Code, with Ghostty treated as a quality reference for terminal feel rather than an app-framework dependency.

The product should provide persistent project workspaces, terminal panes, split/stack/floating-style layouts within the terminal, a command palette, keymap-driven workflow control, build/test task surfaces, and first-class agent/thread orchestration.

Do not build a general IDE in early milestones. Do not add a full source editor, language-server platform, heavy file explorer, extensions marketplace, debugger platform, or project-management dashboard unless a milestone explicitly asks for it.

Do not choose an Apple-native app stack. Xcode.app, `.xcodeproj`, SwiftUI, AppKit, Metal, MetalKit, CoreText-dependent renderer work, notarization-first packaging, and Apple GUI surfaces are out of scope for this repo. The project must be buildable, testable, and runnable from terminal commands under Codex.

The first-class workflow is:

- open a project workspace
- create terminal panes
- split, stack, float, resize, and focus panes quickly
- run builds, tests, scripts, and agents
- inspect output and status surfaces
- preserve/reopen useful session state
- stay keyboard-first with precise native mouse support

## Greenfield Rule

Treat this repo as greenfield. Do not copy files, architecture, runtime shape, code style, or internal abstractions from any prior Aetherspace build unless a future task explicitly asks for a comparison or migration.

If the user references Aetherspace, tmux, zellij, Ghostty, Warp, iTerm, VS Code, Cursor, or another product, use them as product references only. Re-derive this codebase from the product contract and current repo docs.

## Architecture Rules

Keep these boundaries strict:

- `core` owns workspace/session/layout/action state and must be renderer-neutral.
- `pty` owns process lifecycle, terminal I/O, resize, child exit, and stream backpressure.
- `terminal-vt` owns terminal parser adapters and hides concrete parser choices such as libghostty-vt.
- `renderer` owns terminal drawing abstractions, terminal-grid presentation, pane chrome, overlays, and frame timing.
- `app` owns the terminal application runtime, lifecycle, config loading, persistence triggers, and top-level orchestration.
- `commands` owns command palette data, keymap resolution, action dispatch tables, and help text.
- `workflows` owns builds, tests, task recipes, agent threads, logs, and command recipes.

No platform UI type may leak into `core`.

No PTY handle, parser instance, window handle, render resource, or thread handle may be serialized into durable session state.

No renderer may own product or workflow logic.

No task runner may mutate layout state except through core actions.

No terminal parser adapter may assume a specific GUI toolkit.

## Design Rules

Favor dense, calm, professional terminal surfaces.

Avoid IDE chrome. Avoid decorative dashboards. Avoid marketing-style UI. Avoid large empty panels, card grids, and explanatory onboarding screens as the primary product surface.

Use command palette, spatial panes, and status surfaces as the primary workflow.

Every visible UI element must earn its place during a coding session.

The product should feel terminal-native, fast, and stable before it feels feature-rich.

Use text-first symbols sparingly and keep pane chrome restrained. Do not turn the surface into a toolbar-heavy IDE.

## Interaction Rules

Keyboard-first does not mean keyboard-only. Mouse interactions should support direct focus, split resizing, pane dragging when the terminal supports it, text selection where feasible, and clear fallback commands.

Default commands should be discoverable through the command palette and help overlay. Do not rely on F-key-heavy controls as the main interaction model.

Global shortcuts must avoid breaking normal shell, editor, and TUI input.

When a terminal application requests mouse capture or alternate-screen behavior, respect the child application unless the user invokes workspace-level controls.

## Development Workflow

Before implementation work, read:

1. `docs/product-principles.md`
2. `docs/architecture.md`
3. `docs/interaction-model.md`
4. `docs/milestones.md`
5. `docs/codex-goal.md`

For complex tasks, start in `/plan`. Convert accepted plans into `/goal` only when success criteria are measurable.

Use subagents for independent read-heavy work such as technology scans, API surface research, UI reference analysis, terminal conformance research, and risk review. Avoid parallel write-heavy subagents touching the same files.

## Verification

Before claiming completion:

- run formatting for touched code
- run unit tests for touched modules
- run build/typecheck when code exists
- verify architecture boundaries in touched files
- verify docs still match the implementation
- summarize what remains unimplemented

If a verification command is unavailable because the project has not chosen a toolchain yet, say that explicitly and verify the scaffold with filesystem and git checks instead.

## Done Means

A task is done only when the artifact exists on disk, the relevant docs or tests have been updated, and the verification result has been reported.

Do not claim a milestone is complete because a plan was written. Milestones complete when their stated validation criteria pass.
