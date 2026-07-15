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

## Agent Runtime Checks

For agent work, prove:

- agent pane can be created from durable intent
- running, blocked, failed, complete, unknown, and waiting states render
- pending approvals become global attention items
- changed-file summaries are visible
- failed-task investigation launches through the ordinary connector and
  approval seam, then restores as unknown intent rather than a live session;
  adversarial task text cannot forge its evidence framing
- verification results attach to the agent actor (not yet built: the
  checks surface is aspirational; see docs/agent-runtime.md "Not Yet Built")
- restore keeps agent intent without inventing live runtime state

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
```

Both informational flags must print to stdout and exit zero without entering
the TUI. An unknown argument must print a concise error to stderr and exit 2.

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
assertions.

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

Dated live refresh (2026-07-14): p50 11.30 ms / p95 13.08 ms. Like every
`tui_probe` result, this stops at app-output bytes and excludes host-terminal
paint; it is not an end-to-end input-to-photon measurement.

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
