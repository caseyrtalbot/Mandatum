---
name: terminal-conformance
description: Use when evaluating terminal parser correctness, PTY behavior, terminal protocols, mouse/key encoding, terminal backend choices, scrollback, Unicode, or terminal substrate risk.
---

# Terminal Conformance

Use this skill to keep terminal correctness behind the terminal engine interface.

## Inputs

Read first:

1. `AGENTS.md`
2. `docs/terminal-engine.md`
3. `docs/rendering-strategy.md`
4. `docs/architecture.md`
5. `docs/verification.md`

## Workflow

1. Identify whether the question concerns PTY, parser, input encoding, rendering,
   scrollback, or workflow state.
2. Keep PTY/process concerns separate from terminal parser concerns.
3. Evaluate terminal backends behind the `terminal-vt` interface.
4. Prefer fixture-based tests, backend parity tests, and smoke checks over
   product-level assumptions.
5. Name the correctness or capability risk being reduced.

## Output

Return:

- terminal concern being evaluated
- recommended interface or adapter shape
- backend/substrate recommendation
- conformance tests or fixtures needed
- integration risks
