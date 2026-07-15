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

The post-charter developer-trust pass made the existing workstation safer and
more useful without pretending the wider vision is finished:

- `mandatum --help` and `mandatum --version` are non-interactive public
  contracts; unknown arguments fail clearly instead of entering the TUI.
- config reload now resolves a complete effective snapshot, so deleting or
  invalidating a shell, task, connector, or model override restores the
  product default instead of retaining stale runtime behavior.
- frontend input-reader failures stop all live runtimes, restore the host
  terminal, and return the original error instead of trapping the user in an
  unwakeable alternate screen.
- New session creates a fresh session in the current project without
  duplicating that project or reusing a same-id pane's live runtime; the
  historical `open-project` config name remains a compatibility alias, not a
  fictional project chooser.
- a failed task can become a new agent mandate carrying its command, cwd,
  failure status, and a bounded output tail. Every fact is JSON-escaped,
  line-prefixed, and explicitly labeled as untrusted evidence. It launches
  through the existing connector and approval gate and restores only as
  durable intent.
- `RuntimeEngine` is now the deep app-local Module over terminal, task, and
  agent runtime Implementations. It owns the unified event channel, runtime
  identity, reconciliation, replacement, approval control, shutdown, and
  transactional restore. `AppState` receives typed effects and lifecycle
  facts while durable workspace and presentation state remain outside the
  live engine.
- focus now normally accents only the pane title (bright blue in the default dark
  theme) while the full perimeter stays calm; the explicit `focused` label
  preserves a non-color signal, with a one-cell corner fallback when a pane is
  too narrow to show any title text.
- overlays now paint explicit foreground/background surfaces instead of
  reading as nested panes, and the first-run card separates emphasized live
  key routes, normal descriptions, and dim dismissal guidance.
- first-run status now contributes only `new workspace`; the permanent
  keymap-derived control hint owns palette, menu, and help guidance, so the
  footer names each route once.

## Next horizon

- **Named task and dev-server recipes.** Replace the one configured command
  with a project-local catalog for build, test, lint, and server recipes. Keep
  recipe intent in `mandatum-workflows`; add duration, cwd, start time, port,
  and health facts without moving runtime handles into durable state.
- **Recovery cockpit.** On restore, itemize what was recreated, what was
  intentionally detached, and what needs an explicit rerun or relaunch. Add
  acknowledgement for resolved failures so attention remains signal, not
  history.
- **Connector catalog and automation surface.** Add capability-described
  connectors beyond Claude/Fake plus a scriptable command/control surface for
  projects, sessions, recipes, and approval profiles. Human approval remains
  the default; policy broadening requires its own decision and proof.
- **GPU adapter, when the conditions arrive.** The wgpu adapter stays warm
  behind the scene contract. Revisit when the roadmap needs GPU-only
  capability (per-frame animation, pixel-precise layout, embedded non-text
  surfaces) or sets sub-20 ms end-to-end latency as a product goal. Known
  remaining work: full multi-pane/overlay scene binding, grapheme widths,
  IME, runtime DPI, surface-loss recovery, damage tracking.
- **Rewrap on resize.** Currently xterm-style no-rewrap; content wrapped at
  narrow widths stays wrapped. If adopted, it belongs in the
  `mandatum-terminal-vt` grid, not the scene or renderer layers.
- **Damage tracking.** Per-frame grid-to-surface conversion is an accepted
  cost until profiling says otherwise.
- **Minors worth doing.**
  - Cap `approval_history` growth in `AgentPaneIntent` once long-running
    agents make workspace files noticeably large.
  - Bump ratatui to unblock the open Dependabot `lru` update.
  - Idle heartbeat repaints the clock UI (~4 Hz); repaint only when a
    time-derived surface is visible if idle output ever matters.
