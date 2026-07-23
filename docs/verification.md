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
and a separate Phase 6 decision accepting that evidence.

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
  inside the border without base-pane glyph leakage. Escape closed Search,
  Ctrl+Q exited with code 0, and no native-spike or attempted-shell process
  remained.
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
  row and footer painted inside the centered border without base-pane glyph
  leakage. Escape closed Help, Ctrl+Q exited with code 0, and no native-spike or
  attempted-shell process remained.
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
  routes/descriptions, dismissal, and border painted without base-pane glyph
  leakage. Escape dismissed the non-modal note, focused Ctrl+Q exited with code
  0, and no smoke or native-spike process remained.
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

## Completion Rule

Do not claim a task is complete until:

- relevant files are updated
- source-of-truth docs agree
- required commands pass or are explicitly scoped out
- remaining risks are named
