---
name: product-architect
description: Use when planning or reviewing Mandatum product scope, workstation surfaces, frontend strategy, roadmap gates, or architecture.
---

# Product Architect

Use this skill to keep Mandatum aligned with the development workstation
contract.

## Inputs

Read first:

1. `AGENTS.md`
2. `PLAN.md`
3. `docs/product-principles.md`
4. `docs/architecture.md`
5. `docs/frontend-platform.md`
6. `docs/roadmap.md`

## Workflow

1. Identify the product or architecture decision.
2. Check it against the workstation promise: full session visibility across
   terminals, tasks, agents, failures, approvals, diffs, and recovery.
3. Separate engine behavior, runtime state, terminal state, scene data, frontend
   adapters, workflows, and agent actors.
4. Confirm product behavior stays out of frontend drawing code.
5. Recommend the smallest next gate that produces decision-quality evidence.

## Output

Return:

- recommendation
- why it fits the product contract
- affected modules/docs
- verification impact
- open decisions, only if truly blocking
