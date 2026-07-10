# Plan

PLAN.md points forward. History and rationale live in `docs/decisions.md`;
verification procedures live in `docs/verification.md`.

## Shipped

The seven-outcome charter build is complete. Each outcome landed behind a
green `./ci/gate.sh`:

1. Constitution and executable gates: five laws, conformance + doc-trace
   scripts, GitHub Actions running the same gate script, Apache-2.0
   (`cdfe04c`).
2. GPU frontend spike: winit+wgpu window on a live PTY with measured latency
   (`4687a7d`), then scene-contract binding and the side-by-side against the
   ratatui frontend (`94c7cd6`). Verdict: terminal frontend stays v1.
3. Renderer-neutral scene contract: `mandatum-scene` owns the WorkspaceScene
   output model and layout math; the renderer became one adapter (`cd457ed`).
4. Agent runtime: connector contract, session actors, and a real approval
   gate enforced at the connector boundary, proven against live Claude CLI
   (`7a3cd29`).
5. Intuitive UX: pointer support honoring L5, fuzzy palette, config files,
   remappable keymap, themes (`703d53f`, host-termios fix `10a7043`).
6. Workstation visibility: execution timeline, session map, attention strip,
   verified by a stranger test (`e82626e`).
7. Brilliance pass: event-driven loop (key-to-bytes-out p50 42.6 -> 13.3 ms),
   PTY flood backpressure, session search, generated help/first-run/legends,
   calm failure states (`6b5c209`).

Operational hardening after the charter added a public `mandatum` executable,
checksum-verified release archives for macOS and Linux (the approval bridge
ships beside the app), a latest-release installer, and cross-renderer-safe
README captures. The Cargo package remains `mandatum-app`; package selection
is an internal build concern, while `mandatum` is the user-facing command.

## Next horizon

- **GPU adapter, when the conditions arrive.** The wgpu adapter stays warm
  behind the scene contract. Revisit when the roadmap needs GPU-only
  capability (per-frame animation, pixel-precise layout, embedded non-text
  surfaces) or sets sub-20 ms end-to-end latency as a product goal. Known
  remaining work: full multi-pane/overlay scene binding, grapheme widths,
  IME, runtime DPI, surface-loss recovery, damage tracking.
- **Rewrap on resize.** Currently xterm-style no-rewrap; content wrapped at
  narrow widths stays wrapped. If adopted, it belongs in the
  `mandatum-terminal-vt` grid, not the scene or renderer layers.
- **Connector breadth.** The connector protocol is model-agnostic (anything
  that can emit `ApprovalRequested` and accept a decision fits the trait);
  only the Claude CLI and Fake connectors exist today.
- **Damage tracking.** Per-frame grid-to-surface conversion is an accepted
  cost until profiling says otherwise.
- **Minors worth doing.**
  - Cap `approval_history` growth in `AgentPaneIntent` once long-running
    agents make workspace files noticeably large.
  - Bump ratatui to unblock the open Dependabot `lru` update.
  - Idle heartbeat repaints the clock UI (~4 Hz); repaint only when a
    time-derived surface is visible if idle output ever matters.
