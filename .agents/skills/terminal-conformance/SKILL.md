---
name: terminal-conformance
description: Use when evaluating terminal parser correctness, PTY behavior, terminal protocols, mouse/key encoding, libghostty-vt, scrollback, Unicode, or terminal substrate choices.
---

# Terminal Conformance

Use this skill to avoid underestimating terminal correctness and embedding risk.

## Inputs

Read first:

1. `AGENTS.md`
2. `docs/ghostty-libghostty-evaluation.md`
3. `docs/architecture.md`
4. `docs/rendering-strategy.md`

## Workflow

1. Identify whether the question concerns PTY, parser, input encoding, rendering, or workflow state.
2. Keep PTY/process concerns separate from terminal parser concerns.
3. Evaluate substrate choices behind the `terminal-vt` adapter boundary.
4. Prefer fixture-based tests and adapter swaps over product-level assumptions.
5. Name what correctness risk is being reduced.

## Output

Return:

- terminal concern being evaluated
- recommended adapter/interface shape
- substrate recommendation
- conformance tests or fixtures needed
- integration risks

