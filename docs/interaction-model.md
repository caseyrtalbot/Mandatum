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

"Show session map" (palette `m`) opens a modal tree of every session and
its panes. Each pane row carries a kind glyph (terminal/task/agent/status),
its title, a one-word live state (`running`, `exited:N`,
`waiting-approval`, `blocked`, `failed`, `complete`, `idle`), a focus
marker on the active session's focused pane, and `zoom`/`float` badges.
Panes outside the active session show their durable-intent state (only the
active session has live runtimes).

Up/Down (or Ctrl+N/P, or the wheel) move the selection; Enter — or a click
on any row — focuses the selected pane, switching the active session when
needed (a session row switches without changing that session's focus).
Esc closes. The footer names these keys.

## Execution Timeline

Durable facts append to `<project>/.mandatum/timeline.jsonl` as they
happen: command dispatches (with the focused pane), task starts and exits
(with the command string and exit status), agent status transitions,
approval requests (command, scope, risk) and decisions (verdict, decided
by user), agent objective edits, refused agent launches (with the
reason), workspace saves/restores, pane creation/closure, and config
reloads. See docs/decisions.md ("Execution Timeline") for the format and
rotation rules.

"Show timeline" (palette `/`) reads the last ~500 events and lists them
newest first with kind glyphs and relative timestamps ("2m ago");
malformed lines are skipped and counted in the footer, never a crash. The
filter input is the palette input pattern: plain text fuzzy-matches the
event description, and the prefixes `pane:<id>`, `kind:<family>`
(command/task/agent/approval/workspace/pane/config), and `since:<30s|5m|2h|1d>`
filter structurally; tokens AND together. Enter (or a click) on an entry
that names a pane jumps focus to it and closes the overlay. Esc closes.

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

The header is the attention strip, scene-carried (`HeaderScene` holds its
area, composed text, and segments — a frontend paints it without deriving
anything). When something needs eyes it shows, in severity order:

- approvals waiting (count + first pane)
- failed tasks (count + first pane)
- blocked/failed agents (count)

Each segment is styled with the theme's attention color and is a hit
target: clicking it jumps to the pane in need ("Focus next waiting agent",
palette `j`, is the keyboard cycle). When nothing needs attention the
strip shows calm session facts — workspace name, session name, pane count,
agent connector kind — never blank, never noisy.

The status strip below stays the app's own voice: the last status message
plus the permanent control hint (palette chord, right-click menu).

Still open for attention: crashed panes, restore failures, dirty repo,
server health.

## Set Agent Objective

"Set agent objective" (palette `p`, and the agent pane's context menu)
opens a one-line prompt pre-filled with the pane's current objective.
Enter writes it into the durable `AgentPaneIntent` (a timeline fact) —
the next Start agent/relaunch uses it. Esc cancels; an empty objective is
rejected. This closes the "objective only editable by hand-editing JSON"
gap.

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
