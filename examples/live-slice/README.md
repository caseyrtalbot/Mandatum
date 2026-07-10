# The Live Slice

The scene a stranger is shown: one session whose state is readable in ten
seconds from the attention strip, the panes, the session map, and the
execution timeline.

## Run it

```sh
./examples/live-slice/run.sh            # defaults to examples/live-slice/demo-workspace
./examples/live-slice/run.sh /tmp/demo  # or any directory
```

The script writes the demo project (a flaky check script, a project
`.mandatum/config.toml` selecting the deterministic fake agent connector),
generates `.mandatum/workspace.json` through the real core API
(`crates/app/examples/make_live_slice.rs`), prints the keystroke
walkthrough, and launches Mandatum in that directory.

`workspace.json` in this directory is a committed copy of the generated
file for inspection; regenerate it with:

```sh
cargo run -p mandatum-app --example make_live_slice -- /tmp/live-slice \
  && cp /tmp/live-slice/.mandatum/workspace.json examples/live-slice/workspace.json
```

## What is in the workspace

- **pane-1 “terminal”** — a live shell.
- **pane-2 “checks”** — a task pane running `sh ./flaky-check.sh`, which
  alternates success and failure (exit 3), so two reruns (`ctrl+p r`)
  produce one of each. The failing command and its exit status are
  readable in the pane, the header attention strip, and the timeline.
- **pane-3 “dev server”** — a task pane with a heartbeat loop (a
  long-running dev-server stand-in). Rerun it once to start it.
- **pane-4 “agent”** — a floating agent pane. `ctrl+p g` starts the fake
  connector's built-in script: it runs, requests approval to `rm .flip`,
  and waits. `y` approves (the agent completes, with a changed file);
  `n` rejects (the agent fails). Either verdict is a durable timeline fact.

## The stranger test

After driving the five steps the script prints, a person who has never
seen Mandatum should be able to answer, from the screen alone:

- what session am I in (header, session map)
- what runs (dev server heartbeats; session map state words)
- what failed and which command produced it (checks pane, attention strip,
  timeline entry `task pane-2 failed: exit 3: sh ./flaky-check.sh`)
- which agents are active/blocked/waiting approval (agent pane, attention
  strip, session map)
- what files changed (agent pane changed-files list after approval)
- what can I rerun/stop/restart/restore/search (context menu on any pane;
  `ctrl+p /` searches the timeline)
- what survives restart (quit and relaunch: layout, intents, approval
  history, and the timeline all persist; live processes deliberately do
  not — the timeline records that they ran)
