---
name: rendering-spike
description: Use when evaluating Mandatum rendering, scene model, terminal frontend, native/GPU frontend, text quality, pane chrome, frame timing, pointer behavior, or frontend platform strategy.
---

# Rendering Spike

Use this skill for rendering and frontend-platform research or implementation
planning.

## Inputs

Read first:

1. `AGENTS.md`
2. `docs/rendering-strategy.md`
3. `docs/frontend-platform.md`
4. `docs/terminal-engine.md`
5. `docs/architecture.md`

## Workflow

1. Define the rendering question precisely.
2. Keep terminal state, scene model, and frontend adapter separate.
3. Compare options against text quality, frame pacing, latency, selection,
   pointer precision, accessibility, automation, maintainability, and product
   leverage.
4. Keep product behavior out of renderer decisions.
5. Recommend the smallest spike that proves or rejects the option.

## Output

Return:

- decision needed
- options compared
- recommended spike
- success criteria
- risks and fallback
