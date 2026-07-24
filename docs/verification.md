# Verification

## Standard Commands

The merge gate is one script, run from the repository root:

```sh
./ci/gate.sh
```

It runs, in order: `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo build --workspace --all-targets`,
`cargo test --workspace`, `./ci/conformance.sh` (the L1/L2 dependency
laws), and `./ci/doc-trace.sh` (every Constitution law has docs and an
executable gate). GitHub Actions runs exactly this script. Red means the
change does not land.

Run `cargo run` when verifying the current terminal frontend manually, and
`git diff --check` before committing.

## Documentation Verification

After documentation changes, search the active docs for:

- references to files that no longer exist
- prompt text or background-story sections
- frontend bans that contradict `docs/frontend-platform.md`
- implementation status claims that disagree with `docs/repo-structure.md`
- read-order lists that omit a source-of-truth document

Any match must be intentionally current. Missing-doc references and
contradictory frontend constraints should not remain in active documentation.

## Architecture Boundary Checks

Verify:

- `core` has no PTY, parser, renderer, frontend, platform UI, or runtime-handle
  dependencies
- durable workspace JSON excludes process handles, runtime tokens, parser state,
  thread handles, render resources, and scrollback
- frontend drawing code does not dispatch product mutations directly
- runtime modules own live process and agent state
- parser backends stay behind the terminal engine interface

Useful scans:

```sh
rg -n "mandatum-pty|mandatum-terminal-vt|mandatum-renderer|mandatum-app|crossterm|ratatui|portable-pty|winit|wgpu|swift|appkit|metal" crates/core Cargo.toml crates/core/Cargo.toml
rg -n "process_id|runtime_token|reader_thread|JoinHandle|NativePty|parser|scrollback" crates/core
```

## Runtime Checks

For terminal and task runtime work, prove:

- a shell launches in a pane
- typed input reaches the focused pane
- Shift+Tab / BackTab reaches the focused child as `ESC [ Z`, while an explicit
  workspace chord using the same physical key still intercepts first
- output reaches terminal parser state
- resize updates PTY and parser size
- process exit becomes visible status
- pane restart replaces the runtime for the same pane identity
- task launch/rerun/stop works
- a known failed task can create an agent mandate containing command, cwd,
  failure status, and only bounded/prefixed JSON labeled as untrusted evidence;
  injected newlines/markers cannot escape the frame, and transient runtime
  errors do not claim the child task exited
- events from replaced runtimes are ignored
- restore preserves durable intent without live handles
- runtime mutation stays behind `RuntimeEngine` product operations; production
  callers do not receive concrete terminal, task, or agent registry handles
- restore staging is transactional and a staging error commits no lifecycle
  facts; committed facts distinguish fresh, deferred, detached, and
  not-replayed outcomes with valid next actions only; first geometry updates
  the same restore epoch without duplicate facts
- a frontend input-reader failure shuts down live runtimes and restores the
  host terminal before returning the original error; the lifecycle coordinator
  test proves shutdown -> reader stop -> restore ordering and primary-error
  precedence
- New session and session-map activation replace same-id live PTYs rather than
  reusing another session's process/parser

## Terminal Engine Checks

Cover:

- plain text
- invalid UTF-8 bytes
- CR/LF behavior
- wrapping
- SGR styles
- true color
- cursor addressing
- erase display and line
- alternate screen
- scroll regions
- bounded scrollback
- output stress

## Scene And Frontend Checks

For scene/frontend work, prove:

- scene renders terminal panes
- scene renders task panes
- scene renders agent panes
- command palette renders from scene data
- hit targets match pane bounds
- resize preserves layout semantics
- drawing code does not own product behavior
- terminal frontend and any native frontend consume the same scene contract
- `FrontendHost` owns one private `AppState` and exposes no runtime registry
- direct and unified-channel neutral input reach the shared state machine
- blocking wait applies at most one event and nonblocking drain stays within
  its per-call budget
- each owned `FrameSnapshot` carries scene, theme, and a revision that advances
  by snapshot order rather than claiming semantic dirty state
- effects preserve FIFO order and drain exactly once through the host
- quit state and behaviorally idempotent shutdown are available without
  exposing `AppState`
- pointer input resolves against hit targets from the exact prior snapshot,
  including when product state changes before the next frame
- input, PTY, restore-preserved input, and agent producers all use the same
  app-owned sender; no raw producer or receiver bypasses its accounting
- an optional fake-frontend callback fires when the unified queue changes from
  empty to non-empty, coalesces a queued burst without dropping FIFO events,
  and cannot strand an enqueue racing the final receive
- the excluded winit shell binds that callback to `EventLoopProxy` while the
  app channel remains event truth and no interval PTY polling is reintroduced
- winit key, pointer, paste, resize, and focus events become neutral
  `InputEvent` values before reaching the real host
- the real host's header, one terminal, task, or agent pane, status, theme, and
  command palette pass through the scene-only GPU renderer's headless
  preparation seam
- the displayed native smoke paints the covered pane variants, exercises real
  `RuntimeEngine` output and palette input, and leaves no child process on quit

Dated Phase 1B host verification (2026-07-22): all 6 focused host tests and
all 244 `mandatum-app` library tests passed. The full `./ci/gate.sh` passed 463
tests with 2 intentionally ignored live-Claude-CLI tests, including formatting,
Clippy with warnings denied, build, conformance, and doc trace.

Dated Phase 1C wake verification (2026-07-22): controlled sender tests proved
input callback plus channel truth, one callback across a 64-event FIFO burst,
4,096 concurrent send/drain events without a stranded wake, and real PTY plus
agent forwarders sharing the sender. The host callback-injection test passed;
all 248 `mandatum-app` library tests passed. The full `./ci/gate.sh` passed 467
tests with 2 intentionally ignored live-Claude-CLI tests, including formatting,
Clippy with warnings denied, build, conformance, and doc trace. No production
native/GPU dependency was added.

## Agent Runtime Checks

For agent work, prove:

- agent pane can be created from durable intent
- running, blocked, failed, complete, unknown, and waiting states render
- pending approvals become global attention items
- changed-file summaries are visible
- failed-task investigation launches through the ordinary connector and
  approval seam, then restores as unknown intent rather than a live session;
  adversarial task text cannot forge its evidence framing
- restore keeps agent intent without inventing live runtime state

Future acceptance gate: when the checks surface is implemented, verification
results must attach to the agent actor. It is currently aspirational; see
[agent-runtime.md](agent-runtime.md#not-yet-built-aspirational).

## Distribution Check

The public command is `mandatum`, but a complete installation also needs
`mandatum-approval-bridge` in the same directory so the Claude connector can
resolve its fail-closed approval hook.

Before a release change lands:

```sh
cargo build --locked --release -p mandatum-app --bin mandatum \
  -p mandatum-agent-runtime --bin mandatum-approval-bridge
bash -n install.sh
target/release/mandatum --help
target/release/mandatum --version
cargo test -p mandatum-app --bin mandatum update::tests
```

Both informational flags must print to stdout and exit zero without entering
the TUI. Help must advertise `mandatum update`; the updater tests prove the
embedded installer receives the running executable's directory and propagates
the running version, while propagating a non-zero installer result. An unknown
argument must print a concise error to stderr and exit 2.

For a local install smoke, use a disposable root and prove both installed
names rather than launching the TUI:

```sh
install_root="$(mktemp -d)"
cargo install --locked --path crates/app --bin mandatum --root "$install_root"
cargo install --locked --path crates/agent-runtime \
  --bin mandatum-approval-bridge --root "$install_root"
test -x "$install_root/bin/mandatum"
test -x "$install_root/bin/mandatum-approval-bridge"
```

Tags matching `v*` run the full gate, then build native arm64 and x86-64
archives for macOS and Linux. Each archive must contain exactly `mandatum`,
`mandatum-approval-bridge`, and `LICENSE`, with a sibling `.sha256` file.
After publishing, run `install.sh` against the unauthenticated latest-release
URLs with a temporary `MANDATUM_INSTALL_DIR` and repeat the two executable
assertions. Then run that temporary `mandatum update`, repeat both assertions,
and confirm `mandatum --version` reports the version represented by the tag.
Before publishing a newer version, the same update smoke must refuse the older
latest release without replacing the staged executable.

## Input Latency Regression Check

The standing check for the terminal frontend's key-to-output latency. Run
it after any change to the run loop, input path, PTY event plumbing, or
redraw policy.

Procedure:

```sh
cargo build -p mandatum-app --release
cd spikes/frontend-wgpu && cargo run --release --bin tui_probe
```

The probe spawns the real release binary inside a PTY at 100x30, types 100
characters into its shell, and times each until the echo appears in the
app's output bytes (host-terminal paint excluded). It prints one JSON line
with p50/p95/max. Also sanity-check idle CPU: run the app in a PTY, idle
30 seconds, and compare `ps -o cputime` deltas: the event-driven loop
must idle at ~0% (no busy spin).

Reference numbers (2026-07-09, M-series MacBook, release build):

| Loop | p50 | p95 | max | idle CPU (30 s) |
|------|----:|----:|----:|----------------:|
| 40 ms `event::poll` (before) | 42.62 ms | 44.09 ms | 45.54 ms | 0.13 s (~0.4%) |
| event-driven (after) | 13.30 ms | 15.04 ms | 15.27 ms | 0.03 s (~0.1%) |

Dated live refresh (2026-07-14, after the RuntimeEngine move): p50 11.71 ms /
p95 13.56 ms / max 17.84 ms, 100 samples with zero misses. Idle CPU advanced
0.23 s over a clean 30 s window (~0.8% of one core), with no busy spin. Like
every `tui_probe` result, this stops at app-output bytes and excludes
host-terminal paint; it is not an end-to-end input-to-photon measurement.

Dated Phase 1A refresh (2026-07-21, after the renderer-neutral frontend-effect
seam): p50 11.58 ms / p95 13.35 ms / max 16.14 ms, 100 samples with zero
misses. The endpoint remains key-to-app-output bytes with host-terminal paint
excluded.

Dated Phase 1B/terminal-adoption refresh (2026-07-22, after the shipped loop
moved behind `FrontendHost`): p50 11.14 ms / p95 12.58 ms / max 13.05 ms,
100 samples with zero misses. This was run after a fresh release build. The
endpoint remains key-to-app-output bytes with host-terminal paint excluded; it
is not native presentation or input-to-photon evidence.

Dated Phase 1C refresh (2026-07-22, after all input, PTY, and agent producers
moved behind `AppEventSender`): p50 10.60 ms / p95 12.06 ms / max 13.38 ms,
100 samples with zero misses. This was run after a fresh release build. The
endpoint remains key-to-app-output bytes with host-terminal paint excluded; it
is not native presentation or input-to-photon evidence.

Dated Phase 2 refresh (2026-07-22, after the excluded native adapter moved onto
the real `FrontendHost`): p50 11.39 ms / p95 12.56 ms / max 13.69 ms, 100
samples with zero misses. The endpoint remains key-to-app-output bytes with
host-terminal paint excluded; it is not native presentation, symmetric
input-to-photon evidence, or production-admission evidence.

Regression bar: p50 must stay well under 25 ms. A p50 drifting back toward
40 ms means something reintroduced interval polling into the wake path.
The floor is the shell echo round-trip plus the ~8 ms redraw-cap window,
so numbers meaningfully below ~9 ms require changing the cap, not the
loop.

## Deferred GPU Adapter Maintenance Check

The GPU spike stays outside the product Cargo workspace, build, release
artifacts, and merge gate. After any change to `mandatum-scene`, the spike, its
locked dependencies, or the pinned toolchain, run:

```sh
./ci/gpu-spike.sh
```

This opt-in workflow runs spike-local format, locked tests across all
targets, and a renderer-boundary scan. Green means the warm adapter compiles,
its headless contract tests pass, and the isolated `gpu-renderer` crate's normal
dependency tree contains the neutral scene contract but no PTY/parser package.
It does not ship the adapter, exercise a native window, or satisfy either
production trigger. The merge gate separately
fails closed if a listed GPU frontend dependency enters a production workspace
member before an accepted decision has either a typed pixel-native scene
surface with executable adapter tests, or a sub-20 ms key-to-present product
target with symmetric end-to-end evidence. The list is a tripwire for known
stacks, not an exhaustive taxonomy of GPU libraries.

Selecting the Artifact Preview capability in Phase 0 is not GPU-admission
evidence. Future production admission still requires the named typed artifact
scene contract, executable terminal-fallback and excluded-GPU adapter tests,
and a separate Phase 7 decision accepting that evidence.

Dated maintenance run (2026-07-21): after refreshing the excluded lock's four
workspace path packages from `0.1.0` to `0.2.0`, `./ci/gpu-spike.sh` passed
four tests and the renderer-boundary scan. This run did not open a native
window or collect performance samples.

Dated Phase 2 run (2026-07-22):

- `cargo test --manifest-path spikes/frontend-wgpu/Cargo.toml --test host_wake`
  passed its one controlled test. A real `FrontendHost` started `/bin/cat`
  through `RuntimeEngine`; PTY output invoked the injected wake callback without
  interval polling; draining the host produced a `FrameSnapshot` whose terminal
  surface contained the echoed input.
- `./ci/gpu-spike.sh` passed six tests and the renderer-boundary scan. The
  isolated renderer still has `mandatum-scene` and the GPU stack, but no PTY or
  terminal-parser package in its normal dependency tree.
- `cargo test -p mandatum-app --lib` passed all 248 tests, and the full
  `./ci/gate.sh` was green.
- The displayed macOS smoke built with
  `cargo build --release --manifest-path spikes/frontend-wgpu/Cargo.toml --bin mandatum-frontend-wgpu-spike`
  and ran
  `spikes/frontend-wgpu/target/release/mandatum-frontend-wgpu-spike --exit-after 120`.
  Typing `printf GPU_HOST_OK`, opening the real command palette with Ctrl+P,
  closing it with Escape, and quitting with Ctrl+Q all succeeded. After exit,
  no native-spike or child-shell process remained.

This proves the excluded adapter's one-terminal Phase 2 route through the real
host, runtime, wake callback, neutral input, typed clipboard effects, and real
scene snapshots. Restore and broader scene/input parity remain Phase 3.
Artifact Preview and production GPU admission remain pending.

Dated Phase 3 task/agent increment (2026-07-22):

- Test-first real-host scenes produced the expected initial failures:
  `UnsupportedScene::PaneContent("task")`, followed by
  `UnsupportedScene::PaneContent("agent")` after the task tracer bullet was
  green. Both tests drive only neutral input through `FrontendHost` and keep the
  focused product scene to one pane through the real zoom command.
- The final task test starts `printf TASK_PLAN_OK` through the real
  `RuntimeEngine`, waits on the injected wake callback, drains runtime events,
  and proves both the snapshot's `PaneContent::Task` output surface and the
  prepared GPU plan retain the live output. The agent test proves the configured
  objective and scene-composed detail lines reach the same plan. Renderer tests retain
  terminal/palette support, explicit Empty/multi-pane/overlay rejection,
  tail-preserving one-row task metadata, task-output row budgeting, and wrapped
  agent detail text.
- `./ci/gpu-spike.sh` passed ten tests (two native-shell tests, three real-host
  integration tests, and five isolated-renderer tests) plus the renderer
  dependency-boundary scan. `cargo test -p mandatum-app --lib` passed all 248
  tests.
- The displayed release build ran two fresh macOS smokes. Batched neutral key
  events created and zoomed the pane before redraw because multi-pane paint is
  deliberately still unsupported. The task window showed its command, cwd,
  running status, and live `cargo test` output; the agent window showed its
  objective, draft status, action, approval count, and changed-files summary.
  Both quit through Ctrl+Q with no native-spike, task, or approval-bridge
  process left.
- The spike remains excluded from the product workspace/build/release, and the
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Empty content, multi-pane layouts, remaining overlays,
  broader input, restore, Artifact Preview, and production admission remain.
- The final `./ci/gate.sh` passed formatting, Clippy with warnings denied,
  build, all workspace tests, conformance, and documentation trace checks after
  the synchronized documentation edits.

Dated Phase 3 Empty increment (2026-07-22):

- The required real-host tracer bullet constructed a fresh `FrontendHost` with
  PTY spawning disabled, proved its one pane was `PaneContent::Empty`, and first
  failed in `prepare_scene` with `PaneContent("empty")`.
- The final real-host test proves that same product frame reaches the prepared
  GPU plan. Its renderer test proves the scene-composed cwd, restart generation,
  and no-live-grid detail are retained with word-or-glyph wrapping and no
  terminal surface.
- `./ci/gpu-spike.sh` passed eleven tests (two native-shell tests, four real-host
  integration tests, and five isolated-renderer tests) plus the renderer
  dependency-boundary scan. `cargo test -p mandatum-app --lib` passed all 248
  tests.
- The displayed release build ran on macOS from a disposable project with
  `SHELL` set to a nonexistent absolute path and an empty `XDG_CONFIG_HOME`.
  The failed initial PTY spawn produced the real Empty fallback; the window
  showed its cwd, `restart generation: 0`, and no-live-grid detail with the
  existing header, one-pane geometry, status strip, and theme. Ctrl+Q exited
  cleanly, and no native-spike or attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, remaining overlays, broader input,
  restore, Artifact Preview, and production admission remain separately gated.
- The final `./ci/gate.sh` passed formatting, Clippy with warnings denied,
  build, all workspace tests, conformance, and documentation trace checks after
  the synchronized documentation edits.

Dated Phase 3 context-menu increment (2026-07-22):

- The required real-host tracer bullet built an exact pane-body hit target from
  a fresh `FrontendHost` frame with PTY spawning disabled, sent a neutral
  right-button down event inside it, proved the next product frame contained
  `OverlayScene::ContextMenu`, and first failed in `prepare_scene` with
  `Overlay("context menu")`.
- The final real-host test proves that product menu reaches the prepared GPU
  plan unchanged. Its isolated renderer test proves the resolved area, ordered
  rows, chord hints, selected index, and right-aligned line plan are retained.
- `./ci/gpu-spike.sh` passed thirteen tests (two native-shell tests, five
  real-host integration tests, and six isolated-renderer tests) plus the
  renderer dependency-boundary scan. `cargo test -p mandatum-app --lib` passed
  all 248 tests.
- The displayed release build ran on macOS from a disposable project with an
  intentionally missing shell. The real Empty pane remained visible beneath a
  bordered menu at the right-click anchor; all twelve product labels and chord
  hints painted, the first row was highlighted, Escape closed the menu, and
  Ctrl+Q exited cleanly with no native-spike or attempted-shell process left.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, remaining overlays, broader input,
  restore, Artifact Preview, and production admission remain separately gated.
- The final `./ci/gate.sh` passed formatting, Clippy with warnings denied,
  build, all workspace tests, conformance, and documentation trace checks after
  the synchronized documentation edits.

Dated Phase 3 timeline increment (2026-07-22):

- The required real-host tracer bullet used a writable disposable workspace
  file with PTY spawning disabled, drove the real Show timeline palette route
  through neutral Ctrl+P then `/` input, and proved the next product frame
  contained `OverlayScene::Timeline` with the recorded `show-timeline` dispatch.
  Before implementation, `prepare_scene` failed at runtime with
  `UnsupportedScene::Overlay("timeline")`.
- The focused GREEN proves that exact product timeline reaches the prepared GPU
  plan unchanged, including its resolved area, selected row, and timeline-item
  hit target. The isolated renderer test covers the retained query, ordered
  glyph/time/text rows, selection, footer, outer/inner row alignment, bounded
  overlay text, and the explicit `no matching events` state.
- `./ci/gpu-spike.sh` passed sixteen tests (two native-shell tests, six
  real-host integration tests, and eight isolated-renderer tests) plus the
  renderer dependency-boundary scan. `cargo test -p mandatum-app --lib` passed
  all 248 tests.
- The displayed release build ran on macOS from a writable disposable project
  with an intentionally missing shell. The real Empty pane and product chrome
  remained visible beneath a centered bordered Timeline; the selected recorded
  dispatch, glyph, relative time, filter prompt, live `show` query, and footer
  painted. A second displayed pass confirmed a `zzzz` filter paints
  `no matching events` and keeps query/footer text inside the border. Escape
  closed the overlay, Ctrl+Q exited cleanly, and no native-spike or
  attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, remaining overlays, broader input,
  restore, Artifact Preview, and production admission remain separately gated.
- The post-documentation `./ci/gate.sh` passed formatting, Clippy with warnings
  denied, build, all workspace tests, conformance, and documentation trace
  checks.

Dated Phase 3 session-map increment (2026-07-22):

- The required real-host tracer bullet used a fresh `FrontendHost` with PTY
  spawning disabled, drove the real Show session map route through neutral
  Ctrl+P then `m` input, and proved the next product frame contained
  `OverlayScene::SessionMap` with its real active-session heading, focused pane
  row, resolved area, selected index, and row hit target. Before
  implementation, `prepare_scene` failed at runtime with
  `UnsupportedScene::Overlay("session map")`.
- The focused GREEN proves that exact product session map reaches the prepared
  GPU plan unchanged. The isolated renderer test covers the retained geometry,
  ordered tree rows, depth, glyph, label, live state, focus marker, badges,
  selection, footer, row alignment, and bounded overlay text.
- `./ci/gpu-spike.sh` passed eighteen tests (two native-shell tests, seven
  real-host integration tests, and nine isolated-renderer tests) plus the
  renderer dependency-boundary scan. `cargo test -p mandatum-app --lib` passed
  all 248 tests.
- The displayed release build ran on macOS from a writable disposable project
  with an intentionally missing shell. The real Empty pane and product chrome
  remained visible beneath a centered bordered Sessions map; the active session
  heading, selected focused `pane-1 terminal` row, focus glyph, `idle` state,
  and bounded footer painted. Escape closed the overlay, Ctrl+Q exited with code
  0, and no native-spike or attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, remaining overlays, broader input,
  restore, Artifact Preview, and production admission remain separately gated.
- The post-documentation `./ci/gate.sh` passed formatting, Clippy with warnings
  denied, build, all workspace tests, conformance, and documentation trace
  checks.

Dated Phase 3 objective-prompt increment (2026-07-22):

- The required real-host tracer bullet used a fresh `FrontendHost` with PTY
  spawning disabled and a distinctive configured agent objective, drove the
  neutral Ctrl+P then `a`, Ctrl+P then `z`, and Ctrl+P then `p` routes, and
  proved the next product frame contained `OverlayScene::Prompt` over the real
  focused zoomed agent pane with `layout::prompt_rect`, its pane ID in the
  title, the configured objective input, and the scene-composed footer. Before
  implementation, `prepare_scene` failed at runtime with
  `UnsupportedScene::Overlay("prompt")`.
- The focused GREEN proves that exact product prompt reaches the prepared GPU
  plan unchanged. The isolated renderer test covers retained geometry, title,
  input, block-cursor cell, footer, row alignment, and bounded overlay text.
- `./ci/gpu-spike.sh` passed twenty tests (two native-shell tests, eight
  real-host integration tests, and ten isolated-renderer tests) plus the
  renderer dependency-boundary scan. `cargo test -p mandatum-app --lib` passed
  all 248 tests.
- The displayed release build ran on macOS from a writable disposable project
  with an intentionally missing shell. Process-targeted neutral key events
  queued the create-agent and zoom commands before the next redraw, preserving
  the deliberate multi-pane rejection, then opened Set agent objective. The
  real zoomed agent scene remained visible beneath a centered bordered prompt;
  its focused pane title, configured objective input, visible block cursor, and
  bounded footer painted. Escape closed the prompt, Ctrl+Q exited with code 0,
  and no native-spike or attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, remaining overlays, broader input,
  restore, Artifact Preview, and production admission remain separately gated.
- The post-documentation `./ci/gate.sh` passed formatting, Clippy with warnings
  denied, build, all workspace tests, conformance, and documentation trace
  checks.

Dated Phase 3 session-output Search increment (2026-07-22):

- The required real-host tracer bullet used a fresh `FrontendHost` with PTY
  spawning disabled in a writable disposable project, created and zoomed an
  agent, drove neutral Ctrl+Shift+F, and typed `kind:timeline search`. It proved
  the next product frame contained `OverlayScene::Search` over the real focused
  zoomed agent pane with `layout::search_overlay_rect`, the live query,
  deterministic `search-session` timeline result, nonempty char match indices,
  selected index, overflow/footer state, and aligned `SearchItem(0)` hit target.
  Before implementation, `prepare_scene` failed at runtime with
  `UnsupportedScene::Overlay("search")`.
- The tracer bullet intentionally used the timeline result rather than the
  configured agent objective. Current Search snapshots terminal/task grids,
  agent runtime output tails, and timeline events; a newly created draft
  agent's durable objective is not an output row. Keeping the increment
  scene-only preserved that accepted product behavior.
- The focused GREEN proves that exact product Search reaches the prepared GPU
  plan unchanged. Isolated renderer tests cover retained geometry, query and
  block-cursor cell, grouped source elision, result text and match indices,
  selected row, overflow/footer state, empty-query/no-match states, bounded
  lines, and the pane-text clipping that prevents base glyphs from crossing an
  opaque Search modal.
- `./ci/gpu-spike.sh` passed 24 tests (two native-shell tests, nine real-host
  integration tests, and thirteen isolated-renderer tests) plus the renderer
  dependency-boundary scan. `cargo test -p mandatum-app --lib` passed all 248
  tests.
- The displayed release build ran on macOS from a writable disposable project
  with an intentionally missing shell. Process-targeted events queued
  create-agent and zoom before the next redraw, preserving the deliberate
  multi-pane rejection. Ctrl+Shift+F opened Search and Cmd+V atomically pasted
  `kind:timeline search`. The real zoomed agent remained around a centered
  opaque Search modal; its title, query and block cursor, grouped timeline
  source, selected first result, repeated-source elision, and footer painted
  inside the border with no base-pane glyph leakage visible at that observed
  scale. Escape closed Search, Ctrl+Q exited with code 0, and no native-spike
  or attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, Help/Welcome and remaining overlays,
  broader input, restore, Artifact Preview, and production admission remain
  separately gated.
- The first post-documentation `./ci/gate.sh` passed formatting, Clippy with
  warnings denied, build, all workspace tests, conformance, and documentation
  trace checks. The final gate is rerun after recording this result so the
  committed tree itself is the tree proved green.

Dated Phase 3 generated Help increment (2026-07-22):

- The required real-host tracer bullet used a fresh `FrontendHost` with PTY
  spawning disabled, drove neutral F1 over the supported Empty pane, and typed
  `search session output`. It proved the next product frame contained
  `OverlayScene::Help` with `layout::help_overlay_rect`, the exact live query,
  ordered App heading and Search session output entry, configured
  `ctrl+shift+f` route, selected index, and footer. Before implementation,
  `prepare_scene` failed at runtime with `UnsupportedScene::Overlay("help")`.
- The focused GREEN proves that exact generated Help reaches the prepared GPU
  plan unchanged. The isolated renderer test covers retained geometry, query
  and block-cursor cell, grouped heading/entry indentation, key hints,
  selected/windowed row alignment, pinned footer, the empty-items placeholder,
  and bounded lines. Help joins Search in pane-text clipping so base glyphs
  cannot cross the opaque modal.
- `./ci/gpu-spike.sh` passed 26 tests (two native-shell tests, ten real-host
  integration tests, and fourteen isolated-renderer tests) plus the renderer
  dependency-boundary scan. `cargo test -p mandatum-app --lib` passed all 248
  tests.
- The displayed release build ran on macOS from a writable disposable project
  with an intentionally missing shell. F1 opened Help over the real Empty pane;
  the live filter and block cursor narrowed to the App heading and Search
  session output command with its generated `ctrl+shift+f` route. The selected
  row and footer painted inside the centered border with no base-pane glyph
  leakage visible at that observed scale. Escape closed Help, Ctrl+Q exited
  with code 0, and no native-spike or attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, Welcome, broader input, restore,
  Artifact Preview, and production admission remain
  separately gated.
- The first post-documentation `./ci/gate.sh` passed formatting, Clippy with
  warnings denied, build, all workspace tests, conformance, and documentation
  trace checks. The final gate is rerun after recording this result so the
  committed tree itself is the tree proved green.

Dated Phase 3 generated Welcome increment (2026-07-22):

- The required real-host tracer bullet used a writable disposable project with
  no workspace file, startup restore enabled, and PTY spawning disabled. A
  neutral resize preserved the first-run note and proved the next product frame
  contained `OverlayScene::Welcome` over the real Empty pane with
  `layout::welcome_rect`, the scene-owned introduction, ordered generated
  `ctrl+p`, right-click, F1, and Ctrl+Q route/description rows, and dismissal
  text. Before implementation, `prepare_scene` failed at runtime with
  `UnsupportedScene::Overlay("welcome")`.
- The focused GREEN proves that exact generated Welcome reaches the prepared GPU
  plan unchanged. The isolated renderer test covers retained geometry, title,
  introduction, blank separators, ordered and aligned route/description rows,
  dismissal, and bounded lines. Welcome joins Search and Help in pane-text
  clipping so base glyphs cannot cross the opaque card.
- `./ci/gpu-spike.sh` passed 28 tests (two native-shell tests, eleven real-host
  integration tests, and fifteen isolated-renderer tests) plus the renderer
  dependency-boundary scan. `cargo test -p mandatum-app --lib` passed all 248
  tests.
- The displayed macOS smoke used a disposable harness compiled against the exact
  local `FrontendHost`, scene contract, and GPU renderer because the excluded
  native shell deliberately leaves startup restore disabled. With a writable
  project and missing workspace file, the real Empty pane remained around a
  centered opaque Welcome card; its title, introduction, ordered generated
  routes/descriptions, dismissal, and border painted with no base-pane glyph
  leakage visible at that observed scale. Escape dismissed the non-modal note,
  focused Ctrl+Q exited with code 0, and no smoke or native-spike process
  remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Multiple panes, restore in the excluded native shell,
  broader input, Artifact Preview, and production admission remain separately
  gated.

Dated Phase 3 two-horizontal-Empty-pane increment (2026-07-22):

- The required real-host tracer bullet used `AppConfig { spawn_pty: false,
  .. }`, resized to 80x24, then drove neutral Ctrl+P and `v` input through the
  generated Split pane right route. It proved the next product frame contained
  exactly two tiled Empty panes: `pane-1` at `(0, 1, 40, 22)` titled
  `terminal`, `pane-2` at `(40, 1, 40, 22)` titled `terminal 2`, focus on
  `pane-2`, and the existing no-live-grid detail in both. Before
  implementation, `prepare_scene` failed at runtime with
  `UnsupportedScene::PaneCount(2)`.
- The focused GREEN proves that exact real-host scene reaches the prepared GPU
  plan unchanged. The plan now exposes one prepared record per pane, retains
  both scene pane values and Empty detail, and carries no terminal surface for
  either pane. Unsupported two-pane content and all other multi-pane shapes
  continue to fail explicitly.
- `./ci/gpu-spike.sh` passed 29 tests (two native-shell tests, twelve real-host
  integration tests, and fifteen isolated-renderer tests) plus the renderer
  dependency-boundary scan. `cargo test -p mandatum-app --lib` passed all 248
  tests.
- The displayed release smoke launched the excluded native shell from a
  writable disposable project with an intentionally missing shell, then drove
  Ctrl+P and `v`. The real window header reported `2 pane(s)`; equal left/right
  panes painted the `terminal` and focused `terminal 2` titles plus the Empty
  cwd, restart-generation, and no-live-grid detail. The controlling terminal
  stopped the disposable process after capture, and no native-spike process
  remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Vertical, stacked, floating, dense, three-plus-pane,
  and mixed-content multi-pane scenes, restore, broader input, Artifact
  Preview, and production admission remain separately gated.
- The first post-documentation `./ci/gate.sh` passed formatting, Clippy with
  warnings denied, build, all workspace tests, conformance, and documentation
  trace checks. A parallel cold read then found that title glyph bounds were
  still full-frame and the tracer asserted only the last Empty detail row.
  Title buffers and `TextArea` bounds are now clipped to each pane's usable top
  row, and both panes assert cwd, restart generation, and the no-live-grid row
  in the scene and prepared plan. The full GPU spike check, all 248 app tests,
  and the displayed release smoke passed again after those fixes. The final
  gate is rerun after recording this result so the committed tree itself is the
  tree proved green.

Dated Phase 3 two-vertical-Empty-pane increment (2026-07-22):

- The required real-host tracer bullet used `AppConfig { spawn_pty: false,
  .. }`, resized to 80x24, then drove neutral Ctrl+P and `s` input through the
  generated Split pane down route. It proved the next product frame contained
  exactly two tiled Empty panes: `pane-1` at `(0, 1, 80, 11)` titled
  `terminal`, `pane-2` at `(0, 12, 80, 11)` titled `terminal 2`, focus on
  `pane-2`, all layout flags false, and complete cwd, restart-generation, and
  no-live-grid detail in both panes. Before implementation, `prepare_scene`
  failed at runtime with `UnsupportedScene::Layout("only two horizontal tiled
  Empty panes")`.
- The focused GREEN proves that exact real-host scene reaches the prepared GPU
  plan unchanged. The plan retains both pane records and their complete Empty
  details, carries no terminal surface for either pane, preserves the
  two-horizontal path, and continues to reject unsupported two-pane content
  and shapes.
- `./ci/gpu-spike.sh` passed 32 tests (two native-shell tests, fourteen
  real-host integration tests, and sixteen isolated-renderer tests) plus the
  renderer dependency-boundary scan. `cargo test -p mandatum-app --lib` passed
  all 248 tests.
- The displayed release smoke launched the excluded native shell from a
  writable disposable project with an intentionally missing shell, then drove
  Ctrl+P and `s`. The real window header reported `2 pane(s)`; equal top/bottom
  panes painted the `terminal` and focused `terminal 2` titles plus the Empty
  cwd, restart-generation, and no-live-grid detail. Ctrl+Q exited cleanly, and
  no native-spike or attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Stacked, floating, dense, mixed-content,
  three-plus-pane, and two-pane overlay scenes, restore, broader input,
  Artifact Preview, and production admission remain separately gated.
- A fresh cold read found that a real two-pane stack emits one visible
  `PaneScene` and had bypassed the two-pane predicates. The added real-host
  regression first failed by receiving an accepted `PreparedScene`; the final
  plan rejects it explicitly with `UnsupportedScene::Layout("stacked panes")`
  while preserving covered zoomed paths. An isolated negative matrix now proves
  vertical overlay, per-pane floating/stacked/zoomed flags, gap, overlap,
  off-workspace bounds, and mixed content all fail with the narrow tiled-Empty
  layout error.
- The first post-documentation `./ci/gate.sh` passed formatting, Clippy with
  warnings denied, build, all workspace tests, conformance, and documentation
  trace checks. The final gate is rerun after review and this recorded result so
  the committed tree itself is the tree proved green.

Dated Phase 3 two-pane-floating-Empty increment (2026-07-23):

- The required real-host tracer used `AppConfig { spawn_pty: false, .. }`,
  resized to 80x24, then drove neutral Ctrl+P and `v` followed by Ctrl+P and
  `f`. It proved tiled `pane-1` at `(0, 1, 80, 22)`, focused floating `pane-2`
  at `(8, 5, 72, 18)`, durable titles, exact layout flags, and complete cwd,
  restart-generation, and no-live-grid detail in both panes. Before
  implementation, `prepare_scene` failed with
  `UnsupportedScene::Layout("only two horizontal or vertical tiled Empty
  panes")`.
- The focused GREEN retains both pane records and their complete Empty details,
  carries no terminal surface for either pane, and preserves every completed
  one-pane and tiled two-pane path. The isolated negative matrix proves
  overlays, forbidden flags, altered tiled/floating geometry, and mixed
  content fail with the narrow two-pane layout error.
- The first displayed release attempt exposed the real intermediate
  two-horizontal-Empty plus Palette frame between Split and Float. A second
  real-host tracer first failed with the same narrow layout error; the final
  plan admits only that exact Palette command frame, not broader two-pane
  overlays.
- A fresh cold reviewer found that all pane glyphs were submitted after all
  quads, so long wrapped tiled-pane detail could bleed through the floating
  surface. The review-fixed path paints an opaque float, clips lower-pane
  title/body glyph bounds around it, and has an isolated long-cwd regression.
- `./ci/gpu-spike.sh` passed 36 tests (two native-shell tests, sixteen
  real-host integration tests, and eighteen isolated-renderer tests) plus the
  renderer dependency-boundary scan. `cargo test -p mandatum-app --lib` passed
  all 248 tests.
- The displayed release smoke launched the excluded native shell from a
  writable disposable project with an intentionally missing shell, then drove
  Ctrl+P and `v` followed by Ctrl+P and `f` through macOS System Events. The
  visible 800x632 window header reported `2 pane(s)`; tiled `terminal` filled
  the workspace behind focused floating `terminal 2`, and both panes painted
  complete Empty detail. The smoke was repeated from the review-fixed release
  binary with a long wrapping project path; lower-pane glyphs stayed outside
  the opaque float. Ctrl+Q exited 0, and no native-spike or attempted-shell
  process remained.
- The spike remains excluded from the product workspace/build/release. The
  isolated renderer still consumes only `WorkspaceScene` plus `Theme` with no
  PTY/parser dependency. Stacked, broader floating, dense, mixed-content,
  three-plus-pane, and broader two-pane overlay scenes, restore, broader input,
  Artifact Preview, and production admission remain separately gated.
- The final post-documentation `./ci/gate.sh` is run after cold review and this
  recorded result so the committed tree itself is the tree proved green.

Dated overnight-pilot corrective slice 3 (2026-07-23):

- The canonical default-float tracer first failed to compile because
  `mandatum-scene` exposed no shared resolver. The real two-horizontal-Empty
  Palette tracer then failed to compile because the prepared paint plan exposed
  no Palette-safe pane-text visibility regions.
- Focused GREEN proves the scene resolver produces `(8, 5, 72, 18)` for an
  80x24 frame and clamps the same core `FloatingRect::default()` intent to
  `(5, 1, 1, 1)` for a 6x3 frame. The adapter consumes that result instead of
  copying the default offsets, dimensions, or clamping formula.
- All 17 real-host tests pass. The added regression constructs the exact
  admitted two-horizontal-Empty plus Palette frame from `FrontendHost` with a
  deliberately long project path, proves its Empty detail wraps through the
  Palette rows, and proves every pane-body glyph paint region is outside the
  scene-owned Palette rectangle. All 20 isolated renderer tests and all 35
  scene tests pass.
- `./ci/gpu-spike.sh` passed 39 tests (two native-shell, seventeen real-host,
  and twenty isolated-renderer) plus the renderer dependency-boundary scan.
  `cargo test -p mandatum-app --lib` passed all 248 tests.
- The first post-documentation `./ci/gate.sh` passed formatting, Clippy with
  warnings denied, build, every workspace test, conformance, and doc trace.
  The gate is rerun after cold recheck and this recorded result so the committed
  tree itself is the tree proved green.
- The displayed release smoke ran from the same long-path disposable project
  with an intentionally missing shell in a visible 800x632 macOS window.
  System Events drove Ctrl+P then `v`, reopened the real Palette, and screenshot
  inspection showed no wrapped-base-text leakage at that observed scale.
  Dispatching `f` produced the focused default float with no lower-pane-glyph
  leakage visible at that observed scale. Ctrl+Q exited 0, and no native-spike
  or attempted-shell process remained.
- The spike remains excluded from the product workspace/build/release. Stacked,
  moved/resized or additional floating panes, broader two-pane overlays, dense,
  mixed-content, and three-plus-pane scenes remain fail-closed. Artifact
  Preview and production GPU admission remain pending. A cold-review correction
  also proves an altered Palette rectangle is rejected rather than broadening
  the exact transition admission. The cold recheck found a second
  small-viewport title overlap; a focused RED/GREEN regression now proves
  Palette occlusion applies to underlying pane titles as well as pane bodies.

Dated aggregate-review corrections-only slice (2026-07-23):

- Focused RED first failed because `scene_rect_to_text_bounds`,
  `pane_text_visible_bounds`, the fractional-pixel intersection proof, and the
  usable-interior admission predicates did not exist.
- Pane titles and bodies now begin as complete final pixel `TextBounds`.
  Outward-rounded later-float and every current opaque-overlay bound are
  subtracted in pixel space before glyph submission. Isolated tests at
  fractional cell widths prove every visible body bound is disjoint from each
  surface, and the real-host long-path Palette tracer exercises the same
  final-pixel path. Header and status text use the same overlay subtraction; a
  3x3 full-frame overlay regression proves neither chrome region submits glyphs
  through the opaque surface.
- Every admitted multi-pane rectangle must be at least 3x3 cells. Real-host
  resize tests accept the default horizontal layout at 6x5, the default
  vertical layout at 3x8, and the default float at 11x9, then reject 5x5, 6x4,
  2x8, 3x7, 10x9, and 11x8. Isolated larger-frame cases also prove extreme
  horizontal and vertical splits with sub-3-cell panes are rejected. Separate
  `u16::MAX` cases prove checked right/bottom endpoints reject a pane whose true
  edge would overflow instead of accepting the saturated edge.
- `mandatum-scene` still resolves the core default float to `(5, 1, 1, 1)` at
  6x3. That remains a correct scene-layout clamping fact; `prepare_scene`
  rejects the resulting degenerate multi-pane frame.
- `./ci/gpu-spike.sh` passed 50 tests (two native-shell, twenty real-host,
  and twenty-eight isolated-renderer) plus the renderer dependency-boundary
  scan. `cargo test -p mandatum-scene` passed all 35 tests, and
  `cargo test -p mandatum-app --lib` passed all 248 tests.
- The final post-review `./ci/gate.sh` passed formatting, Clippy with warnings
  denied, build, every workspace test, conformance, and doc trace, proving the
  committed tree itself green.
- A release build drove the long-path missing-shell transition in a visible
  800x632 macOS window. Screenshot inspection showed no leakage through the
  Palette or default float at that observed scale. Ctrl+Q exited cleanly and
  no native-spike or attempted-shell process remained. Fractional-width safety
  is established by the final-pixel regressions, not inferred from screenshots.
- The spike remains excluded from the product workspace/build/release.
  Stacked, moved/resized or additional floating panes, broader two-pane
  overlays, dense, mixed-content, and three-plus-pane scenes remain
  fail-closed. Artifact Preview and production GPU admission remain pending.

Layout/composition capability-family verification (2026-07-23), at that
family's original stop point and superseding the topology limits above. Its
per-pane paint resources were subsequently replaced by the content/style cell
program recorded below:

- Focused RED/GREEN tracers covered one real stack, three real tiled panes, two
  real ordered floats, dynamic pane-buffer growth, and final-pixel subtraction
  of every later opaque pane plus the current overlay. The stack first failed
  with `Layout("stacked panes")`; three panes first failed with `PaneCount(3)`;
  and the buffer test first failed to compile because no dynamic pool existed.
- The completed `prepare_scene` compiler no longer recognizes layout
  topologies. It validates a usable bordered interior, checked pane endpoints,
  workspace containment, and a 256-pane aggregate renderer ceiling. It does
  not reconstruct scene identity, tiled coverage, overlap, flags, focus, or
  draw order.
- Aggregate review found and corrected two high-confidence defects. A
  zero-pane scene could prepare successfully while the public one-pane
  inspection helpers indexed pane zero; zero panes now return
  `SceneCompileError::NoVisiblePane`. Occlusion initially considered only
  `floating` panes, so overlapping non-floating panes could leak lower text;
  every pane now paints an opaque base and every later pane in scene order is
  subtracted. The review also replaced topology-era error strings with typed
  compile failures and bounded the retained pane-buffer high-water mark.
- `./ci/gpu-spike.sh` passed 48 tests: two native-shell tests, twenty-two
  real-host integration tests, and twenty-four isolated-renderer tests, plus
  the renderer dependency-boundary scan.
- A release build ran one visible missing-shell scenario matrix on macOS. It
  progressed from one pane to three tiled panes, stacked the first split while
  retaining the durable three-pane header count, added two overlapping floats,
  and opened Help over the resulting five-pane scene. Screenshot inspection
  showed distinct three-pane buffers, the scene-owned stack representation,
  opaque later-pane composition, and no underlying text through Help. Ctrl+Q
  exited 0 and no native-spike process remained.
- The next family at this stop point was content/style parity. That family is
  now complete below; input/lifecycle remains. The spike remains excluded, and
  production dependency admission and release changes remain blocked.

Content/style capability-family verification (2026-07-23):

- Focused RED/GREEN tracers established the public neutral cell contract first,
  then terminal, task, agent, Empty, header/status chrome, pane borders/titles,
  Palette, context menu, timeline, session map, objective prompt, Search, Help,
  and Welcome semantics. `SceneCell` remains unchanged. `CellProgram` carries
  final topmost `Glyph(char)` or `WideContinuation` occupancy, complete
  `SceneCellStyle`, one optional selection kind, and cursor state in
  deterministic row-major order.
- The ratatui renderer and excluded GPU renderer consume that same final cell
  program. The GPU translation covers ANSI/indexed/RGB colors; built-in and
  custom semantic roles; bold, dim, italic, underline, inverse, hidden, and
  strikethrough; terminal/item selection; cursor; and opaque replacement.
  `PreparedScene` contains no pane/content/overlay shadow plan.
- Real-host tests assert representative final program text and semantic marks
  for Empty, task output, agent detail, copy selection/cursor, and every
  overlay. Focused compiler tests cover huge off-frame rectangles, 128 fully
  overlapping panes, true bordered interiors for one- and two-cell panes, and
  every overlay at one- and two-cell dimensions. GPU tests cover reverse-video
  composition, continuation/hidden cells, final opacity, and checked pane,
  frame-cell, paint-work, and row-buffer ceilings.
- Aggregate review found and corrected the remaining defects before completion:
  obsolete ratatui modules and the GPU shadow plan were removed; duplicate
  selection truth was collapsed; off-frame and degenerate-border paint was
  clipped; final-cell storage replaced the unbounded replacement stream for
  both adapters; GPU work/resource ceilings were checked before compile; and
  warnings-denied all-target clippy joined `./ci/gpu-spike.sh`.
- `cargo test -p mandatum-scene` passed 45 tests (35 unit and 10 integration);
  `cargo test -p mandatum-renderer` passed 28 tests; and
  `./ci/gpu-spike.sh` passed 34 tests (two native-shell, twenty-three real-host,
  and nine isolated GPU tests), warnings-denied clippy, formatting, and the
  renderer dependency-boundary scan.
- The final release build ran a displayed 800x632 macOS matrix with a custom
  `mandatum-light` theme. It showed the real Empty fallback; a successful task
  with 256-color foreground/background plus bold, italic, underline, and
  strikethrough output; a fake agent in the waiting-for-approval attention
  state; and an opaque Palette with custom border/surface/selection roles over
  that mixed scene. Screenshot inspection found no covered-text leakage.
  Escape closed the overlay, Ctrl+Q exited 0, and no native-spike process
  remained.
- Input/lifecycle parity is the exact next Phase 3 family. True
  grapheme/wide-cell production and IME remain Phase 5 boundaries. Artifact
  Preview, production GPU admission, workspace/release dependencies, and
  rollout remain unchanged and blocked.

The same conformance check resolves all Cargo features and keeps release builds,
archive members, and installer binaries on explicit allowlists (`mandatum`, the
approval bridge, and `LICENSE`). Release and install surfaces may not reference
the excluded GPU spike directly.

## The Stranger Test (Workstation Visibility)

The charter bar for the visibility surfaces: a stranger looking at the
screen understands the session state in ten seconds. Procedure:

1. Run the driven demo: `./examples/live-slice/run.sh` (setup, keystroke
   walkthrough, and launch; see `examples/live-slice/README.md`).
2. Drive the printed steps: start the dev-server heartbeat, run the check
   twice (one success, one failure), start the fake agent (it requests an
   approval and waits).
3. Without touching the keyboard further, answer from the screen alone:
   - what session am I in (header; `ctrl+p m` session map)
   - what runs (heartbeat pane; session map state words)
   - what failed and which command produced it (checks pane status,
     "1 task failed" attention segment, timeline entry with command +
     exit status)
   - which agents are active/blocked/waiting approval (agent pane,
     "1 approval waiting" attention segment)
   - what files changed (agent pane changed-files list after approving)
   - what can I rerun/stop/restart/restore/search (right-click menu;
     `ctrl+p /` timeline filter)
   - what survives restart (`ctrl+p w`, quit, relaunch: layout, intents,
     approval history, and the timeline persist; live processes do not,
     and the timeline records that they ran)
4. Automated backing: timeline write/read/rotation/malformed-line tests
   (`crates/app/src/timeline.rs`), overlay filter/jump and session-map
   focus tests (`crates/app/src/app_state.rs`), attention aggregation
   tests (`crates/app/src/scene_builder.rs`), and the header-in-scene
   parity tests (`crates/app/tests/frontend_parity.rs`).

## Phase 3B Native Input/Lifecycle Capability (2026-07-23)

Environment: macOS on Apple M4 Pro, one online LG ULTRAGEAR+ display, release
native adapter launched from disposable project
`/private/tmp/mandatum-phase3b-display.rY9TY2`. The adapter remained excluded
from the product workspace/build/release surface.

Automated and review evidence:

- Focused RED/GREEN coverage exercises configured chord precedence, unbound
  Super suppression, xterm baseline navigation/editing/F1-F24 modifiers, the
  full conventional ASCII control family, Alt-as-Meta, native copy/paste
  boundaries, child any-event motion, focus cancellation, child-capture
  release, float edge/shrink containment, pointer selection/copy/scrollback,
  startup restore of both visible terminal runtimes, resize, quit, idempotent
  shutdown, finite scale-probe arguments, and tiny-frame suspension.
- Three independent aggregate reviewers plus a final cold read found and drove
  fixes for modifier loss, sticky child capture, focus-finalized selection,
  stale/rejected hit targets, geometry-error string control flow, float
  shrink/restore containment, duplicate Alt on BackTab, clipboard error
  visibility, wheel axes, invalid scale values, and partial shutdown. The final
  confidence-70-or-higher review was clean.
- `./ci/gpu-spike.sh` passed formatting, warnings-denied all-target Clippy,
  the renderer dependency-boundary scan, and 39 substantive tests: five native
  shell, twenty-five real-host, and nine isolated renderer tests.
- The post-documentation `./ci/gate.sh` passed root format, warnings-denied
  Clippy, build, all workspace/unit/integration/doc tests (including 262 app
  library and 36 scene library tests), L1/L2 plus GPU-admission conformance,
  the app input-seam scan, and documentation trace.
- The existing reduced-motion proof
  `scene_builder::tests::reduced_motion_kills_the_pulse_and_no_other_motion_exists`
  remains renderer-neutral: reduced motion holds the sole approval pulse steady,
  and the scene is otherwise byte-identical.

Displayed release matrix:

- Ctrl+P and F1 opened the real Palette and generated Help using only keyboard
  input; Escape closed both.
- Cmd+V pasted `CLIPBOARD_PHASE3B_OK` through arboard and the neutral paste
  boundary. A real pointer drag produced a visible 27-character selection;
  Cmd+C cleared it, reported `copied 27 char(s) to clipboard`, and `pbpaste`
  contained the selected shell text.
- The real palette created a second terminal and saved the workspace. Ctrl+Q
  exited with no native or child-shell process left. Relaunch restored the two
  pane layout, recreated both PTYs, and displayed a fresh `Restored session`
  marker in each pane.
- Minimize/focus recovery cleared synthetic stuck modifiers; full-screen
  resize repainted the two-pane workspace from 88x30 to 380x72 and returned
  cleanly through Ctrl+Q.
- Because this Mac exposes only one display, the bounded spike-only
  `--scale-after 2 --scale-factor 1.5` tracer exercised the exact
  `ScaleFactorChanged` transition without changing system settings. The
  displayed grid recomputed from 88x30 to 57x20, both restored PTYs remained
  visible, 16 frames presented, and JSON reported
  `scale_probe_applied=true`; the process exited 0 at its deadline.

Standing terminal regression procedure:

- Fresh root release build plus `cargo run --release --bin tui_probe` produced
  p50 11.77 ms / p95 14.68 ms / max 18.56 ms over 100 key-to-app-output
  samples, zero misses. Host-terminal paint remains excluded.
- Over a clean 30-second idle PTY run, process CPU time advanced from 0.04 s to
  0.14 s: 0.10 s total, about 0.33% of one core, with no busy spin.

Known boundary: the runtime scale transition is proven, but this one-display
environment cannot prove cross-monitor movement. Advanced grapheme/wide-cell
production, IME/dead-key composition, surface/device recovery, production GPU
admission, packaging, and rollout remain later phases.

## Phase 4 Artifact Preview Capability (2026-07-23)

Environment: macOS on Apple M4 Pro. The displayed release adapter ran from the
disposable project `/private/tmp/mandatum-artifact-display.Gvgj4Q` through a
temporary local app wrapper used only so the accessibility driver could target
the otherwise unbundled spike binary. The adapter remained excluded from the
product workspace/build/release surface.

Automated and review evidence:

- Durable core tests prove JSON round-trip of project-relative source, title,
  alt text, and contain fit without pixel, decoder, handle, texture, or revision
  leakage.
- Eight focused app artifact tests prove exact RGBA8 load, explicit and
  metadata-detected reload with increasing revision, APNG/malformed/missing/
  oversized/extension/traversal failures, final-file and ancestor symlink
  rejection, descriptor-swap containment, 4096×4096/64 MiB bounds, four-worker
  scheduling, aggregate decoded-byte admission, the 64-pane/open-descriptor
  cap, stale completion release, and the real palette/prompt/scene path.
- Runtime restore preserves buffered artifact completions alongside input so
  cleared-workspace tokens release their reservations; stale results cannot
  populate the restored workspace.
- Ratatui tests prove deterministic loading/ready/failed fallback. Scene tests
  prove final-topmost raster markers and later-pane/overlay occlusion. The
  isolated GPU tests prove exact surface propagation without byte copies,
  malformed/aggregate rejection before allocation, contain-fit for landscape/
  portrait/square targets, fractional scissor boundaries, all-stale cache
  eviction before replacement, and old-revision removal.
- The real-host tracer drives the fuzzy "Open artifact preview" command and
  prompt, reaches a ready surface, prepares the GPU plan, rewrites the file,
  dispatches Restart Pane, and reaches the new dimensions/revision.
- Three independent architecture, logic/security, and product-quality
  reviewers plus the final cold read found and drove fixes for validation/use
  races and FIFO blocking, unbounded decoded buffers/thread fan-out/file
  descriptors, APNG acceptance, misleading Restart Pane behavior, GPU reload
  high-water overshoot, synchronous per-frame header parsing, and restore-
  stranded reservations. The final confidence-70-or-higher rerun reported no
  remaining defect.
- `./ci/gpu-spike.sh` passed formatting, warnings-denied all-target Clippy,
  the renderer dependency-boundary scan, and 46 substantive tests: five native
  shell, twenty-six real-host, and fifteen isolated renderer tests.
- The final post-documentation `./ci/gate.sh` reported `GATE GREEN`, including
  270 app tests, 36 scene tests, 11 cell-program tests, 29 renderer tests, the
  core and command suites, workspace integration, documentation traceability,
  and conformance checks.

Displayed release matrix:

- The keyboard-only fuzzy palette opened `preview.png`; the pane reported
  `600x300 RGBA8 sRGB` and painted the image contain-fit without distortion.
- Generated Help covered the artifact with an opaque card; pixels remained
  visible only outside the overlay and did not bleed through it.
- Replacing the source with a `300x600` PNG and dispatching Restart Pane
  produced the new dimensions and portrait contain-fit. Full-screen resize
  recomputed the grid from 88×30 to 380×72 and preserved the image fit.
- Opening `missing.png` produced an in-pane red
  `preview: failed · artifact file is missing: missing.png` state without a
  panic or process exit.
- Ctrl+Q closed the native app and the exact process PID was absent afterward.

Remaining boundary: this completes the selected pixel-native capability but
does not admit the winit/wgpu dependency tree to production. Phase 5 advanced
grapheme/wide-cell/IME correctness, multi-display proof, surface/device-loss
hardening, production admission, installer/release work, and rollout remain.

## Phase 5 Advanced Text And IME Capability (2026-07-23)

Environment: macOS on Apple M4 Pro with one attached display. The displayed
adapter used the excluded debug native/GPU binary with Menlo 16 and its bounded
runtime scale tracer. The release terminal probe used the shipped
ratatui/crossterm frontend. No GPU dependency entered the product workspace,
installer, or release.

Contract and automated evidence:

- `mandatum-terminal-vt` preserves bounded extended grapheme clusters,
  combining/ZWJ extension, atomic width-two placement and continuation repair
  across write/erase/edit/resize, and safe replacement at grid edges.
- `mandatum-scene` validates one nonempty grapheme of display width one or two,
  compiles explicit continuations, and aligns wrapping, truncation, selection,
  cursor, attention geometry, and scalar search ranges to grapheme columns.
  Invalid public-scene graphemes fail closed.
- The ratatui adapter emits the full grapheme symbol and clears continuation
  cells. The excluded GPU adapter owns one anchored buffer per visible
  grapheme, retains decorated spaces, bounds frame rows and text buffers
  separately, validates font/scale at the renderer boundary, and clips glyphs
  to adjacent non-overlapping fractional cell spans.
- `InputEvent::Composition` round-trips preedit with a validated UTF-8 cursor
  range, commit, and cancel. App tests cover every eligible text target,
  modal/pointer/key/paste/focus cancellation, target locking, resize
  re-anchoring, exact cancel-before-focus-loss ordering, and one ignored late
  platform commit.
- Winit tests cover neutral IME translation, invalid ranges, multi-scalar
  commit without paste/scalar truncation, named Space, baseline modifiers,
  pointer state, runtime scale, font bounds, and fatal/clean exit status.
- `./ci/gpu-spike.sh` passed formatting, warnings-denied all-target Clippy,
  the renderer dependency-boundary scan, and 53 substantive tests: six native
  shell, twenty-seven real-host, and twenty isolated renderer tests.
- The post-documentation `./ci/gate.sh` reported `GATE GREEN`, including 280
  app library tests, 38 scene library tests, 14 cell-program integration tests,
  30 renderer tests, 23 terminal-engine tests, the remaining workspace/unit/
  integration/doc suites, L1/L2 and GPU-admission conformance, the app input
  seam, and documentation traceability.
- Three independent correctness, boundary/security, and acceptance review
  tracks drove fixes for late-commit ordering, unfocused IME re-enable,
  public-scene validation, GPU buffer admission, copy/search/wrap/scrollback
  wide edges, Command Palette placeholder clearing, decorated spaces, glyph
  overhang, attention geometry, and fractional span overlap. All three final
  reruns returned no finding.

Displayed macOS matrix:

- Left Option+E showed underlined preedit at the real terminal caret; E
  committed one `é`. The Command Palette showed the same preedit at its input
  row without its placeholder leaking underneath.
- Starting a dead-key preedit, moving focus to Terminal, and returning canceled
  the composition. The next E inserted plain `e`, proving no stale preedit or
  late commit leaked across focus loss.
- The real shell displayed mixed `A界é👩‍💻Z` output with the CJK glyph,
  decomposed combining grapheme, emoji ZWJ sequence, following ASCII, cursor,
  and cell backgrounds remaining aligned.
- Menlo 16 rendered through the native-only font settings. Runtime scale 1.25
  and a window resize recomputed the grid from 66×23 to 99×29 while preserving
  the active scene and composition geometry.
- Ctrl+Q exited the native app with status 0 and the exact process absent.
- This one-display, current-input-source run does not claim cross-monitor
  movement, a visible candidate popup, or every installed locale/input source.

Standing terminal regression procedure:

- `cargo build -p mandatum-app --release` succeeded.
- `cd spikes/frontend-wgpu && cargo run --release --bin tui_probe` measured
  p50 14.58 ms / p95 16.67 ms / max 18.28 ms over 100 key-to-app-output
  samples with zero misses. Host-terminal paint is excluded by design.
- In a clean release PTY idle window, process CPU advanced from 0.55 s to
  0.83 s over exactly 30 seconds: 0.28 s, about 0.93% of one core, with no busy
  spin.

Remaining boundary: Phase 5 is complete, but this does not admit production
GPU dependencies. Phase 6 still owes surface/device recovery, explicit
out-of-memory/no-adapter/no-display outcomes, multi-display and resize/scale
storms, structured symmetric measurement, and soak evidence before any
admission decision. Packaging, release changes, and rollout remain later.

## Completion Rule

Do not claim a task is complete until:

- relevant files are updated
- source-of-truth docs agree
- required commands pass or are explicitly scoped out
- remaining risks are named
