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

Ctrl+P opens it with an empty filter input. The interaction contract
(implemented in `crates/app/src/palette.rs`, which documents it in full):

- Typing filters every command by case-insensitive fuzzy subsequence match,
  with word-boundary, prefix, and contiguous-run bonuses. Best match first.
- Commands relevant to the focused pane kind rank first: agent commands on
  agent panes, task commands on task panes, pane commands on terminals.
- Commands that are currently impossible appear greyed with the reason in
  the detail text, never hidden.
- Every entry shows its verb-first label, a detail line, and its current
  key(s) from the live keymap. The selected entry previews the pane it will
  affect where that is cheap.
- Single-letter fast paths are preserved on the first keystroke: while the
  input is empty, a bare bound key runs its command (with the task-pane
  and float/dock substitutions), `q` runs the listed Quit command, and
  Tab/BackTab cycle pane focus. An unbound key — or any Shift+letter —
  starts the filter instead, so every command stays reachable by typing.
  The empty input's placeholder states this rule and the Shift escape.
- Up/Down or Ctrl+N/Ctrl+P move the selection (while open, Ctrl+P
  navigates rather than toggling), and the wheel scrolls it. Enter runs
  the selection; on a greyed entry it reports the reason and stays open.
  Esc closes. The footer names these keys and counts entries hidden
  outside the visible window.
- The palette itself is reachable without a chord: clicking the status
  strip opens it (the strip's permanent hint names the chord), and the
  pane context menu leads with a "Command palette" row.

Command labels should be short, verb-first, and stable.

Still open for the palette: recent commands, and settings/keymap commands.

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
- click the status strip to open the palette

Split ratios also move from the keyboard: Grow pane / Shrink pane adjust
the focused pane's nearest enclosing split in 5% steps, the same durable
intent separator drags write. Dock pane is the inverse of Float pane, and
the float letter toggles between them.

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
