# Mandatum

[![CI](https://github.com/caseyrtalbot/Mandatum/actions/workflows/ci.yml/badge.svg)](https://github.com/caseyrtalbot/Mandatum/actions/workflows/ci.yml)

Mandatum is a development workstation for terminal-centered builders: shells,
tasks, dev servers, agents, diffs-in-progress, approvals, and recovery live in
one spatial session surface. It is a terminal environment expanded into a
session operating system, not a chat app, dashboard, or editor clone.

Five immutable laws govern the architecture, each enforced by an executable CI
gate: see [docs/constitution.md](docs/constitution.md).

## Quickstart

```sh
cargo run -p mandatum-app
```

Three doors in:

- `ctrl+p` opens the fuzzy command palette (every action lives there)
- right-click opens a context menu on any pane
- `?` in the palette (or `F1`) opens help, generated from the live keymap

Configuration is read from `~/.config/mandatum/config.toml` (honoring
`XDG_CONFIG_HOME`), overlaid by `<project>/.mandatum/config.toml` (project
wins). A broken config never blocks launch; each bad key produces a status-line
warning and keeps its default. Sections: `[keymap]`, `[keymap.palette]`,
`[theme]`, `[ui]`, `[shell]`, `[task]`, `[agent]`.

### The live-slice demo

```sh
./examples/live-slice/run.sh
```

Sets up a demo project (live shell, a flaky check task, a dev-server
heartbeat, a floating agent pane on the deterministic fake connector), prints
a keystroke walkthrough, and launches Mandatum in it. See
[examples/live-slice/README.md](examples/live-slice/README.md).

## Crate map

```text
crates/core           durable workspace intent: sessions, panes, layouts,
                      actions, persistence (runtime-free leaf; serde only)
crates/commands       command table, palette routing, fuzzy matcher, keys
crates/pty            PTY process lifecycle, I/O, resize, exit, byte events
crates/terminal-vt    terminal parser adapter, grid, scrollback, capabilities
                      (parser stays behind TerminalAdapter)
crates/scene          renderer-neutral scene contract: WorkspaceScene output
                      model, pane layout math, neutral input types
crates/agent-runtime  agent connector contract, approval events, FakeConnector,
                      Claude CLI connector + the approval-bridge hook binary
crates/workflows      task recipes and agent launch intent
crates/renderer       the ratatui frontend adapter: render(frame, &scene, &theme)
crates/app            the workstation: event loop, runtime registries, scene
                      builder, timeline, search, config, save/restore
spikes/               experiments outside the Cargo workspace; they may depend
                      on engine crates, but their heavy dependency trees never
                      join the product build or the CI gate
```

CI runs `./ci/gate.sh` (fmt, clippy `-D warnings`, build, test, the L1/L2
conformance scans, doc-trace). Local runs and CI execute the same script.

## Status

What works today, all behind a green gate:

- multi-pane sessions: live shells, task panes (launch/rerun/stop, exit status
  visible), floating panes, split-drag resize, save/restore of durable intent
- agents as session actors: objective, state, output tail, changed files, and
  a real approval gate. The Claude CLI connector runs `claude -p` headless
  with a PreToolUse hook that blocks on a Unix socket until you approve or
  reject in the workstation; the FakeConnector scripts deterministic flows
- visibility: header attention strip (approvals waiting, failed tasks, blocked
  agents), session map, append-only execution timeline
  (`.mandatum/timeline.jsonl`), session-wide output search with a query
  grammar (`pane:`, `kind:`)
- pointer support honoring terminal soul: wheel scrollback, drag selection,
  and passthrough to children that request mouse reporting; alt+click is the
  explicit workspace override
- event-driven main loop: key-to-bytes-out p50 13.3 ms measured by the
  external `tui_probe` (was 42.6 ms on the old 40 ms poll loop); PTY floods
  are backpressured (a `yes` flood stays at ~12 MB RSS and quits in under a
  second, measured on the release binary)
- generated help, first-run note, and glyph legends with drift-failing tests

Deferred, deliberately:

- **GPU frontend.** The winit+wgpu spike (`spikes/frontend-wgpu`) renders
  purely from the scene contract and measured key-to-GPU-present p50 21.6 ms
  against the TUI's pre-rework 42.9 ms bytes-out. The terminal frontend stays
  v1; the adapter stays warm behind the scene contract. Evidence and the full
  verdict: [spikes/frontend-wgpu/RESULTS.md](spikes/frontend-wgpu/RESULTS.md).
- **Rewrap on resize.** Lines wrapped at a narrow width stay wrapped after
  growing the terminal (xterm behavior). If wanted, it belongs in the
  terminal-vt grid.
- **Dependency bump.** The open Dependabot update for `lru` cannot apply
  until ratatui bumps: `lru` enters the tree only through the ratatui 0.29
  pin.

PLAN.md holds the forward horizon; docs/decisions.md holds every judgment
call; docs/verification.md holds the standing verification procedures.

## License

Apache-2.0. See [LICENSE](LICENSE).
