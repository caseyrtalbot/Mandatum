---
name: rendering-spike
description: Use when evaluating terminal rendering, TUI drawing, terminal grid presentation, pane chrome, frame timing, ratatui/crossterm, termwiz, notcurses, mouse capture, or terminal renderer strategy.
---

# Rendering Spike

Use this skill for terminal rendering and app-runtime research or implementation planning.

## Inputs

Read first:

1. `AGENTS.md`
2. `docs/rendering-strategy.md`
3. `docs/architecture.md`
4. `docs/ghostty-libghostty-evaluation.md`

## Workflow

1. Define the rendering question precisely.
2. Keep terminal state, scene model, and backend renderer separate.
3. Compare options against terminal feel, performance, text correctness, mouse/input behavior, maintainability, and implementation cost.
4. Do not put product logic into renderer decisions.
5. Recommend the smallest spike that produces decision-quality evidence.

## Output

Return:

- decision needed
- options compared
- recommended spike
- success criteria
- risks and fallback
