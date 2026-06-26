---
name: interaction-reviewer
description: Use when designing or reviewing keyboard, mouse, command palette, pane, task, or agent interactions for Mandatum. Trigger on keybindings, command palette, pane controls, UX, discoverability, or workflow ergonomics.
---

# Interaction Reviewer

Use this skill to keep the interaction model keyboard-first, terminal-safe, discoverable, and native.

## Inputs

Read first:

1. `AGENTS.md`
2. `docs/interaction-model.md`
3. `docs/product-principles.md`

## Workflow

1. Identify the user workflow being changed.
2. Check shell-input safety: normal terminal/editor input must not be stolen.
3. Check command-palette discoverability.
4. Check mouse precision and native expectations.
5. Check visual density and pane chrome restraint.
6. Reject F-key-heavy or IDE-like control models unless explicitly justified.

## Output

Return:

- interaction verdict
- concrete control proposal
- shell-safety risks
- discoverability path
- verification steps
