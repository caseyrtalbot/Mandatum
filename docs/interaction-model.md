# Interaction Model

## Control Philosophy

The workspace should be keyboard fluent, pointer precise, and safe around child
terminal applications.

Normal terminal input passes through unless the user explicitly invokes
workspace control.

## Primary Controls

- direct typing into focused terminal/editor pane
- command palette
- leader/keymap actions
- pointer focus and resizing
- pane context menu
- session map navigation
- execution timeline search
- status strip actions

## Command Palette

The command palette is the universal control surface.

It must support:

- fuzzy search
- recent commands
- context-aware commands
- task commands
- agent commands
- pane commands
- project/session commands
- settings and keymap commands
- approval commands

Command labels should be short, verb-first, and stable.

## Pane Interaction

Required pane actions:

- focus
- split right/down
- stack
- float
- dock
- zoom
- close
- rename
- restart terminal runtime
- rerun task runtime
- stop task runtime
- pin agent pane
- inspect status

Pointer support should include:

- click to focus
- drag split separators
- drag floating panes
- double-click or command to zoom
- select text
- open context menu

If a child terminal app requests mouse capture, the workspace must respect that
until the user invokes workspace-level control.

## Session Map

The session map shows:

- panes
- tasks
- agents
- running servers
- failed actors
- waiting approvals
- hidden/stacked/floating surfaces

It should support jump-to-pane, focus waiting approval, focus failed task, and
restore layout actions.

## Execution Timeline

The timeline records:

- shell commands
- task launches
- task exits
- agent state changes
- approvals
- file-change summaries
- verification results
- restore events

The timeline should be searchable and scoped by project, pane, task, or agent.

## Copy, Search, And Scrollback

Terminal panes need:

- bounded scrollback
- keyboard copy mode
- pointer selection
- semantic selection where possible
- search within pane output
- copy command output
- copy failure block
- copy changed-file list

Copy and search are presentation/runtime concerns, not durable core state.

## Status And Attention

Attention should be explicit and restrained:

- failed task
- blocked agent
- pending approval
- crashed pane
- restore failure
- dirty repo
- server health warning

The user should be able to jump directly from an attention indicator to the
surface that needs action.

## Accessibility

Plan for:

- keyboard-only operation
- configurable keymaps
- readable contrast
- font scaling
- reduced motion
- visible focus
- descriptive labels for non-terminal surfaces
- platform accessibility hooks in native frontends
