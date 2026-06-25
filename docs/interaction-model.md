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
- start time
- exit code
- failure summary
- rerun action
- stop action

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
