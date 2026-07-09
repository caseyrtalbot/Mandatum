---
name: interaction-reviewer
description: Use when designing or reviewing keyboard, mouse, command palette, pane, task, agent, timeline, session map, approval, or workflow interactions for Mandatum.
---

# Interaction Reviewer

Use this skill to keep the workstation keyboard fluent, pointer precise,
discoverable, and safe around child terminal applications.

## Inputs

Read first:

1. `AGENTS.md`
2. `docs/interaction-model.md`
3. `docs/product-principles.md`
4. `docs/agent-runtime.md`
5. `docs/workflows.md`

## Workflow

1. Identify the user workflow being changed.
2. Check that normal terminal/editor input is not stolen.
3. Check command-palette discoverability.
4. Check direct manipulation: focus, resize, drag, select, inspect.
5. Check attention flow for failures, approvals, blocked agents, and task status.
6. Check visual density and pane chrome restraint.

## Output

Return:

- interaction verdict
- concrete control proposal
- input-safety risks
- discoverability path
- verification steps
