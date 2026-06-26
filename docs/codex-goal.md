# Codex Goal Setup

This file preserves historical bootstrapping prompts that produced the accepted Milestone 0-1 plan. Do not treat the prompts below as current implementation status. Current status lives in `README.md`, `docs/milestones.md`, `docs/repo-structure.md`, and handoffs.

## First Prompt: Use Plan Mode

Start with `/plan`, not `/goal`, because the first pass should sharpen architecture before writing code.

Use this prompt:

```text
Use this repository's docs as source of truth. Plan Milestone 0 and Milestone 1 for this greenfield terminal-native workspace.

The product should be closer to tmux and zellij than to a general IDE. It should provide persistent project workspaces, terminal panes, split/stack/floating-style layouts inside the terminal, command palette control, build/test task surfaces, and agent/thread orchestration.

Do not inspect or reuse any existing Aetherspace code. Treat this as greenfield.

Do not use Xcode.app, `.xcodeproj`, SwiftUI, AppKit, Metal, MetalKit, CoreText renderer work, or Apple-native GUI app surfaces. The implementation must be buildable, testable, and runnable from terminal commands under Codex.

Do not write implementation code yet unless the plan explicitly calls for a minimal scaffold. Identify decisions that are blocked, propose the smallest viable implementation path, and produce an execution plan with verification gates.

Your plan must cover:
1. product principles and non-goals
2. repo structure
3. module boundaries
4. technology choices with tradeoffs
5. milestone plan
6. verification plan
7. open questions that truly block implementation
```

## Goal Mode Prompt

Historical prompt only; the runnable prototype clause below was superseded by
the accepted core-first Milestone 1 and parser/PTY-seam-first Milestone 2 path.
As of the current implementation, `crates/pty` has a headless native OS PTY
session wrapper, but there is still no runnable app shell, renderer integration,
or visible terminal pane.

After the plan is accepted, start `/goal` with:

```text
Design and scaffold a greenfield terminal-native workspace for developers, closer to tmux than an IDE.

The product should provide persistent project workspaces, terminal panes, split/stack/floating-style layouts, a command palette, keymap-driven workflow control, build/test task surfaces, and first-class agent/thread orchestration. It must not become a general IDE: no built-in source editor, no heavy file explorer, no language-server-first architecture in the first milestone.

Start by creating a Rust workspace foundation with docs, architecture boundaries, and milestone plan. The original prompt also asked for a runnable prototype, but that clause was superseded by the accepted core-first and seam-first milestone path. Keep the workspace/session/layout/action core independent from rendering, terminal app runtime, PTY, and terminal parser implementation. Include a terminal parser adapter boundary suitable for evaluating libghostty-vt later.

Done when:
1. The repo has clear product principles, architecture docs, interaction model, and milestone plan.
2. The codebase has a compilable Rust scaffold with separated core, PTY, terminal adapter, terminal renderer, app runtime, commands, and workflow modules.
3. Superseded: the original prompt expected a runnable placeholder prototype; current implementation status is documented outside this historical prompt block.
4. The architecture explicitly documents what is greenfield, what is deferred, and what would be evaluated from Ghostty/libghostty.
5. Core logic has unit tests for workspace, layout, focus, command dispatch, and session persistence.
6. No implementation depends on an existing Aetherspace code path or copies its files.
```

## Codex Operating Pattern

Use one main goal thread for decisions and implementation.

Use subagents only for independent read-heavy work:

- terminal parser options
- terminal rendering options
- Ghostty/libghostty API scan
- comparable product teardown
- accessibility/input research
- architecture risk review

Do not let multiple agents edit the same core files in parallel.

## Suggested Subagent Prompts

### Terminal Substrate Research

```text
Research terminal parser/substrate options for this greenfield terminal-native workspace. Compare libghostty-vt, self-built parser, alacritty/vte-style approaches, and temporary fake parser strategy. Return a recommendation with risks, integration seams, and what to spike first. Do not edit files.
```

### Terminal Rendering Research

```text
Research terminal rendering options for a Rust-first terminal workspace. Compare ratatui/crossterm, termwiz, notcurses, and a minimal custom renderer. Focus on multi-pane output, input, mouse capture, scrollback, portability, and Codex-verifiable testing. Do not edit files.
```

### Product Reference Review

```text
Review tmux, zellij, Ghostty, iTerm, Warp, and modern coding agent surfaces as product references. Identify what this repo should borrow, avoid, and treat as non-goals. Keep the output grounded in this repo's docs. Do not edit files.
```

## Implementation Start Gate

Implementation starts only after:

1. Milestone 0 plan is accepted.
2. Toolchain is chosen.
3. repo module layout is confirmed.
4. verification commands are defined.
5. architecture boundaries are written into `AGENTS.md`.

For current repo state, those gates are satisfied for Milestone 1. Milestone 2
has started with the fake parser adapter seam and pure PTY abstraction seam.
Use `README.md`, `docs/milestones.md`, `docs/repo-structure.md`, and
`docs/verification.md` for current implementation status and phase gates.
