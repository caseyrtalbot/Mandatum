# Interaction Model

## Core Shape

The workspace is a terminal application containing one or more project workspaces. Each workspace is made of panes. Panes can be shell panes, task output panes, agent panes, status/log panes, or future specialized terminal surfaces.

The user controls the workspace through:

- direct terminal input
- command palette
- keybindings
- mouse/pointer actions
- terminal command palette
- contextual pane actions

## Principles

### Shell Input Is Sacred

Normal shell/editor/TUI input must pass through unless the user explicitly invokes workspace control.

Avoid global shortcuts that steal common shell/editor keys.

### Command Palette Is The Primary Control Surface

The command palette should support:

- fuzzy command search
- workspace actions
- pane actions
- build/test/task actions
- agent actions
- project actions
- recent commands
- contextual commands for focused pane

Command labels must be short and action-oriented.

### Spatial Control Should Be Direct

Users should be able to:

- click pane to focus
- drag split separators
- drag floating panes
- double-click or command to zoom
- use keyboard to focus next/previous
- use keyboard to split, stack, float, close, restart, and rename

### Discoverability Without Onboarding Bloat

Use:

- command palette
- help overlay
- status hints
- native menu labels
- concise empty states

Avoid:

- tutorial landing pages
- long instruction panels
- decorative cards
- marketing copy

## Pane Types

### Terminal Pane

Runs a shell or command under PTY.

Required actions:

- focus
- split
- close
- restart
- rename
- zoom
- float/dock
- copy selection
- paste
- scrollback
- clear
- open command palette scoped to pane

### Task Pane

Runs a build, test, dev server, script, or recipe.

It may be backed by a terminal process but should expose task metadata:

- command
- cwd
- status
- start time (deferred)
- exit code (current runtime surfaces exit status text)
- failure summary (deferred beyond raw exit/status text)
- rerun action
- stop action

Current task runtime: `Run Task` from the command palette (`Ctrl-P`, then `b`)
opens a task pane and runs one configured shell command. When a task pane is
focused, `Ctrl-P` then `r` reruns the same durable task intent in the same pane,
and `Ctrl-P` then `c` stops a pending or running task. The task pane stores
durable command intent only; live running/succeeded/failed/stopped status and
output are app runtime state.

### Agent Pane

Tracks an agent or Codex-like thread.

It should not become a chat-first product. Treat it as a work surface with:

- status
- active objective
- pending approvals
- changed files
- test results
- logs
- latest summary
- open thread action

### Status/Log Pane

Shows structured project/workspace state:

- running processes
- failed tasks
- dirty repos
- active agents
- ports
- health probes

## Layout Model

Support:

- split horizontal/vertical
- stack/tab group
- floating pane
- zoom focused pane
- project/workspace tabs inside the terminal
- saved layouts

Layout is durable intent, not renderer state.

## Default Commands

Early command vocabulary:

- open project
- new terminal
- split right
- split down
- focus next
- focus previous
- close pane
- restart pane
- zoom pane
- float pane
- stack panes
- run build
- run tests
- rerun last task
- stop task
- show agents
- start agent
- show command history
- open settings
- save workspace
- restore workspace

## Keybinding Philosophy

Use a leader or command-mode approach for workspace-level controls.

Recommended early defaults:

- command palette: platform-native primary shortcut plus a terminal-safe fallback
- leader key: configurable
- focus next/previous: configurable
- split/close/zoom/float: leader-based defaults

Do not make F-keys the primary path.

## Copy Mode and Scrollback (Milestone 4 baseline)

Terminal panes keep a bounded scrollback history and expose a keyboard-first
copy mode. Copy mode is presentation state owned by the app runtime; it never
mutates core layout or the parser grid.

Enter copy mode from the command palette (`Ctrl-P` then `[`, or the "Copy Mode"
command). Inside copy mode:

- `h`/`j`/`k`/`l` or arrow keys move the copy cursor through the visible grid and scrollback.
- `PageUp`/`PageDown` scroll a page; `g`/`G` jump to the top/bottom of history.
- `0`/`$` move to the start/end of the line.
- `v` or `Space` starts a selection at the cursor; `c` clears it.
- `y` or `Enter` copies the selection (or the cursor's line when nothing is selected) and exits.
- `q` or `Esc` exits without copying.

Copy uses the OSC 52 escape sequence to set the host terminal's clipboard, so it
works over SSH and needs no platform clipboard dependency. The host terminal
must support OSC 52 for the copy to reach the system clipboard. Normal shell
input is never intercepted unless copy mode is explicitly active, and a terminal
resize exits copy mode rather than tracking moved coordinates.

This is the minimal documented baseline. Native OS mouse selection, semantic
selection, and rich clipboard history are out of scope for this milestone. Copy
mode reads the live grid, so a pane that keeps producing output while you select
can shift the buffer under the selection; the baseline does not freeze output.
For stable selection, copy from a quiescent pane (or scroll into settled
scrollback). Output-freezing copy mode is a later refinement.

## Mouse Philosophy

Mouse support should feel native and precise:

- click focus
- drag resize
- drag floating panes
- select text
- right-click context menu
- hover affordances only when useful

If a child terminal application requests mouse capture, respect it.

## Visual Density

Pane chrome should be useful but minimal.

Each pane may show:

- short title
- cwd/project chip
- status icon
- task/agent status
- dirty/error indicator

Avoid permanent heavy toolbars.

## Accessibility

Plan for:

- keyboard-only operation
- keyboard-only operation for all workspace controls
- readable contrast
- font size control
- reduced motion
- screen reader labels for non-terminal UI
