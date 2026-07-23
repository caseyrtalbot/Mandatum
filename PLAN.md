# Plan

PLAN.md points forward. History and rationale live in `docs/decisions.md`;
verification procedures live in `docs/verification.md`.

## Shipped

The seven-outcome charter build is complete. Each outcome landed behind a
green `./ci/gate.sh`:

1. Constitution and executable gates: five laws, conformance + doc-trace
   scripts, GitHub Actions running the same gate script, Apache-2.0
   (`cdfe04c`).
2. GPU feasibility spike completed (not product-shipped): winit+wgpu window on
   a live PTY with measured latency (`4687a7d`), then scene-contract binding
   and the side-by-side against the ratatui frontend (`94c7cd6`). Verdict:
   terminal frontend stays v1.
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
`mandatum update` now consumes the latest published release without requiring
a checkout or GitHub permissions; publication remains a version-tagged
maintainer action, and every workspace crate inherits one root release version.

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
- `FrontendHost` now owns the terminal run's one private `AppState`, exposes
  neutral input and bounded event consumption, and returns owned
  scene/theme/revision snapshots plus FIFO platform effects. The shipped
  terminal frontend exercises this seam while retaining crossterm, terminal
  lifecycle, heartbeat/redraw scheduling, rendering, and OSC 52 encoding.
- The unified input/PTY/agent channel now has one app-owned `AppEventSender`.
  It keeps `std::sync::mpsc` as event truth and can notify a frontend through
  a coalesced, renderer-neutral callback when the queue changes from empty to
  non-empty; sender/receiver accounting prevents a concurrent final drain and
  enqueue from losing the next wake.
- focus now normally accents only the pane title (bright blue in the default dark
  theme) while the full perimeter stays calm; the explicit `focused` label
  preserves a non-color signal, with a one-cell corner fallback when a pane is
  too narrow to show any title text.
- Shift+Tab now reaches terminal applications as the standard xterm BackTab
  sequence instead of being dropped, while explicit workspace chords continue
  to intercept before terminal fallback.
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
- **Native GPU frontend, admission-gated.** The implementation sequence is
  specified in
  [docs/native-gpu-implementation-plan.md](docs/native-gpu-implementation-plan.md):
  shared host/effect seam, terminal migration, real-state-machine native slice,
  parity, text/IME, recovery/performance, admission, and opt-in rollout.
  The capability branch is selected: task/agent-produced PNG artifacts become
  pixel-native preview panes with a deterministic terminal fallback. Phase
  1 is complete: neutral effects, the shared host, the coalesced wake-aware
  sender, and shipped-terminal adoption all exercise one state machine. Phase
  2 is also complete inside the excluded spike: its winit shell now drives the
  real `FrontendHost`, wakes through `EventLoopProxy`, translates neutral input,
  and paints the real header, terminal pane, status strip, and command palette
  without its former duplicate PTY/parser state machine. Phase 3 is underway:
  its scene-only increments now paint real one-pane task and agent content,
  including live task output, the Empty fallback, the context menu, and the
  execution timeline, session map, objective prompt, and session-output search
  from existing scene data. Multi-pane layouts, remaining overlays, broader
  input, and restore remain parity work. Production GPU dependencies and release
  admission remain blocked until the typed artifact scene surface, adapter
  tests, and later admission decision exist; Artifact Preview remains unbuilt.
- **Rewrap on resize.** Currently xterm-style no-rewrap; content wrapped at
  narrow widths stays wrapped. If adopted, it belongs in the
  `mandatum-terminal-vt` grid, not the scene or renderer layers.
- **Damage tracking.** Per-frame grid-to-surface conversion is an accepted
  cost until profiling says otherwise.
- **Minors worth doing.**
  - Cap `approval_history` growth in `AgentPaneIntent` once long-running
    agents make workspace files noticeably large.
  - Bump ratatui before updating the transitive `lru` dependency.
  - Idle heartbeat repaints the clock UI (~4 Hz); repaint only when a
    time-derived surface is visible if idle output ever matters.
