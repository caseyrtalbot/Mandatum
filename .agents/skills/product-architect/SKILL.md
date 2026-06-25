---
name: product-architect
description: Use when planning or reviewing product scope, non-goals, milestones, or architecture for this greenfield terminal-native workspace. Trigger on product principles, architecture plan, milestone planning, or "is this becoming an IDE?"
---

# Product Architect

Use this skill to keep the project aligned with the greenfield product contract.

## Inputs

Read first:

1. `AGENTS.md`
2. `docs/product-principles.md`
3. `docs/architecture.md`
4. `docs/milestones.md`

## Workflow

1. Identify the requested product or architecture decision.
2. Check it against the product category: terminal-native workspace, closer to tmux/zellij than an IDE.
3. Separate terminal substrate, workspace product, app shell, renderer, workflow orchestration, and agent surfaces.
4. Flag any drift into IDE scope.
5. Produce a concrete recommendation with tradeoffs and verification impact.

## Output

Return:

- recommendation
- why it fits or violates the product contract
- affected modules/docs
- verification or milestone impact
- open decisions, only if truly blocking
