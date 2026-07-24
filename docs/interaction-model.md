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
- session output search
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

## Help

"Help" (default chord `f1`, palette `?`, the status strip's permanent hint,
and the last row of every pane context menu) opens a filterable overlay
generated at open time — never hand-maintained text that can drift:

- every command grouped by category, each with its CURRENT key route from
  the live keymap: global chord (rebinds included), palette letter spelled
  as `ctrl+p <letter>`, or the honest "palette (type to search)" fallback
  for commands with no binding
- the palette fast-path rules and the direct approval keys (y/n)
- the mouse gestures, each naming its keyboard equivalent, including the L5
  note: when a child app captures the mouse, alt+click / alt+drag reaches
  the workspace
- the glyph legends for the session map and timeline, generated from the
  same tables those overlays draw from

The filter input is the palette pattern (type to filter, Up/Down or
Ctrl+N/P scroll, Esc closes). Pressing the help chord again toggles it
closed. The status strip hint and the first-run note both derive the help
route from the live keymap, so a rebind is never contradicted on screen.

## First Run

When launched with no saved workspace — and only then — the state-only status
`new workspace` composes with the permanent live-keymap hint into `new
workspace — ctrl+p commands · right-click menu · f1 help`; each route appears
once. A calm eight-line note names the four doors: the palette chord, the
right-click menu, the help key, and the quit chord. It is not modal: any key,
paste, or click dismisses it and the action itself still lands. Once a
workspace has been saved the launch path restores instead, so the note never
returns. No splash theater.

## Pane Interaction

Current pane actions:

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
- inspect status

Pin Agent Pane remains aspirational; see
[agent-runtime.md](agent-runtime.md#not-yet-built-aspirational).

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
the float letter toggles between them. Floating panes move from the
keyboard too: Move float left/right/up/down step the durable float rect
(2 columns / 1 row per step, clamped like a drag), so float placement
never requires a pointer.

"New session" creates and focuses a fresh session under the current project.
It is deliberately not named Open project: choosing another project is not
built. Existing configs using the historical `open-project` command name keep
working as a compatibility alias. A session switch retires all live runtimes
before same-id panes in the destination session are reconciled.

If a child terminal app requests mouse capture, the workspace must respect that
until the user invokes workspace-level control.

## Session Map

"Show session map" (palette `m`) opens a modal tree of every session and
its panes. Each pane row carries a kind glyph (terminal/task/agent/status),
its title, a live state (`open`, `running`, `succeeded: exit 0`,
`failed: exit 3`, `waiting-approval`, `blocked`, `failed`, `complete`,
`idle`) — exit facts use the same vocabulary as the pane body and the
status line, so one fact never reads two ways — a focus
marker on the active session's focused pane, and `zoom`/`float` badges.
Panes outside the active session show their durable-intent state (only the
active session has live runtimes).

Up/Down (or Ctrl+N/P, or the wheel) move the selection; Enter — or a click
on any row — focuses the selected pane, switching the active session when
needed (a session row switches without changing that session's focus).
Esc closes. The footer names these keys and carries a legend for the
glyphs actually on screen (`▸ session · ❯ terminal · ▶ task · ◆ agent · ≡
status · ● focused`), generated from the same table the rows draw from so
it cannot drift; the full legend also lives in the help overlay.

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
The footer ends with a legend for the kind glyphs currently listed
(`» command · ▶ started · ✓ ok · ✗ failed · …`), generated from the same
table `glyph()` is tested against; the full legend also lives in the help
overlay.

## Session Search

"Search session output" (chord `ctrl+shift+f`, the fuzzy palette, every
pane's context menu; deliberately no palette letter — search took the
last free one, which would have ended bare-letter filter seeding) is
honest text search — exact/fuzzy subsequence matching, never embeddings.
Opening it snapshots the searchable text once:

- every live terminal pane's scrollback+screen text (the grid bounds
  scrollback at 2000 rows per pane, so older output is gone)
- every running task pane's output grid
- every live agent pane's output tail (last ~200 lines)
- the execution-timeline tail (last ~500 events)

Scope and limits: the active session only (other sessions have no live
runtimes), and results reflect the moment the overlay opened — a flooding
pane cannot reshuffle the list mid-read; reopen to search newer output.

Typing filters the snapshot with the palette input pattern. Plain tokens
fuzzy-subsequence-match a line (matched chars highlighted); the prefixes
`pane:<title-or-id-substring>` and
`kind:<terminal|task|agent|timeline>` (prefix match) filter structurally;
tokens AND together. Results group by source in pane order (timeline
last), most recent first within each group, capped at 200 with an honest
"+N beyond cap" count in the footer — `pane:`/`kind:` are the escape
hatch when one noisy pane buries the rest. An empty query lists nothing
(calm over noisy).

Enter (or a click) on a pane hit focuses that pane; for terminal panes it
also scrolls the viewport to the matched row and selects the matched span
— the pointer-view mechanics, so plain typing keeps flowing to the shell
(L5). Task output follows its content tail and agent tails render
bottom-anchored, neither with a scrollable viewport, so focus is the
whole jump there. If the match was
evicted or moved by new output since the snapshot, the status says so
instead of pretending. Enter on a timeline hit opens the timeline overlay
positioned at that entry. Esc returns. The footer names the keys.

## Copy, Search, And Scrollback

Terminal panes need:

- bounded scrollback
- keyboard copy mode
- pointer selection
- semantic selection where possible
- search within pane output (shipped session-wide: see Session Search)
- copy command output
- copy failure block
- copy changed-file list

Copy and search are presentation/runtime concerns, not durable core state.

## Status And Attention

The header is the attention strip, scene-carried (`HeaderScene` holds its
area, composed text, and segments — a frontend paints it without deriving
anything). When something needs eyes it shows, in severity order:

- approvals waiting (count + the first pane's title)
- failed tasks (count + the first pane's title)
- blocked/failed agents (count)

Segments name panes by their user-facing titles ("1 task failed ·
checks"), so a glance says WHICH pane needs eyes; pane ids stay in the
timeline and session map, where audit needs them. Status messages lead
with the title too and keep the id as trailing detail ("checks failed:
exit 3 · pane-5").

Each segment is styled with the theme's attention color and is a hit
target: clicking it jumps to the pane in need ("Focus next waiting agent",
palette `j`, is the keyboard cycle). When nothing needs attention the
strip shows calm session facts — workspace name, session name, pane count,
agent connector kind — never blank, never noisy.

The status strip below stays the app's own voice: the last status message
plus the permanent control hint (palette chord, right-click menu).
Byte-level PTY diagnostics ("read N byte(s)") never overwrite it — a
failure status persists until something meaningful supersedes it, not
until the next read. `[ui] debug_status = true` restores the diagnostics
for debugging sessions.

Still open for attention: crashed panes, restore failures, dirty repo,
server health.

## Set Agent Objective

"Set agent objective" (palette `p`, and the agent pane's context menu)
opens a one-line prompt pre-filled with the pane's current objective.
Enter writes it into the durable `AgentPaneIntent` (a timeline fact) —
the next Start agent/relaunch uses it. Esc cancels; an empty objective is
rejected. This closes the "objective only editable by hand-editing JSON"
gap.

## Investigate A Failed Task

"Investigate task failure with agent" is discoverable in the fuzzy palette
and the context menu of a failed task. It is disabled, with an explicit
reason, when focus is not a task, the task has no known failure, or no agent
connector is configured. Running it creates a focused agent pane whose durable
objective contains the task command, resolved cwd, failure status, and a
bounded output tail. Every fact is bounded, JSON-escaped, line-prefixed, and
labeled as untrusted evidence. Transient runtime diagnostics do not enable the
command without a typed process-exit, launch-failure, or rerun-failure fact.
The agent then uses the same connector, approval requests, and stop/relaunch
controls as every other agent. Restore keeps the objective but folds away the
live-session claim.

## Accessibility

What holds today, each backed by a test where one is possible:

**Keyboard-only operation is complete.** Every pointer behavior has a
keyboard route: click-to-focus (Tab/BackTab, session map), separator drags
(Grow/Shrink pane), floating-pane drags (Move float left/right/up/down),
wheel scrollback and drag selection (copy mode), double-click zoom (Zoom
pane), the context menu and status-strip click (the palette lists every
command), attention-segment clicks (Focus next waiting agent for
approvals; the session map's state column for failed panes), and overlay
row clicks (Enter). No known gaps: the pointer is a convenience layer,
never the only door.

**Reduced motion.** `[ui] reduced_motion = true` disables the
waiting-approval pulse — the only motion in the product — by holding its
emphasis steady instead of alternating it. Nothing else in the scene is
time-driven; a scene-equality test pins that adding unguarded motion
fails the build.

**Visible focus.** The focused pane title has its own theme color in all three
built-in themes (bright blue in mandatum-dark; bright yellow in
mandatum-high-contrast), reinforced bold, while the full perimeter stays on
the calm pane-border role. The `focused` word remains in the title, so focus
never rides on color alone. Theme- and renderer-level tests assert the title
distinction and neutral border per theme. At degenerate one-to-three-column
widths, where no title text can survive, one accented corner cell preserves a
visible focus signal without restoring a loud perimeter.

**Layered surfaces.** Every overlay paints an explicit foreground/background
surface inside its accent border, so palettes, maps, prompts, help, and the
first-run card read as a layer above the workspace rather than another pane.
The first-run card keeps live keys, descriptions, and dismissal guidance as
separate scene fields: keys are accented and bold, descriptions are normal,
and the dismissal line is dim.

**Configurable keymaps.** Every command is rebindable (`[keymap]`), all
surfaces (palette hints, context menu, help, first-run note, status strip)
derive key text from the live keymap, and bare-key chords are rejected at
the config boundary (L5).

**Terminal key forwarding.** The focused child receives the baseline
`xterm-256color` character, control, navigation/editing, and F1-F24 families.
Shift+Tab reaches it as BackTab (`ESC [ Z`) from either neutral representation;
additional Alt/Control modifiers remain encoded. Alt acts as Meta, conventional
control aliases are accepted, and configured workspace chords still win before
terminal fallback or native clipboard conventions. Enhanced negotiated
protocols such as CSI-u are not implied by this baseline.

**Artifact Preview.** Search the fuzzy palette for "Open artifact preview",
enter a project-relative PNG path, and Mandatum opens a focused floating
artifact pane. The pane always states source, useful alt text (filename by
default), and loading/ready/failed state. The terminal frontend keeps that
deterministic labeled fallback; the native adapter paints the same
ready scene surface contain-fit without stretching. "Restart pane" on an
artifact is an explicit reload and does not mutate terminal restart intent.
Missing, malformed, animated, oversized, escaping, symlinked, or over-budget
sources remain visible in the pane and never panic. URLs, SVG, HTML, PDF,
video, and animation are outside this slice.

**Font scaling.** The maintained terminal tool inherits its host terminal's
font, size, and zoom. The native product owns its font, scale, DPI, and IME
contract. Renderer-neutral product behavior remains authoritative. The current
native shell handles live
scale changes through a single transition that cancels stale pointer ownership,
updates glyph metrics, recomputes pointer cells, resizes the host/PTYs, and
requires a successful new present before interaction resumes. It also validates
native-only font family/size settings, keeps left Option for dead-key
composition and right Option for terminal Meta, and routes renderer-neutral
preedit/commit/cancel to the active text surface. The next quality decision is a
side-by-side typography comparison with Ghostty, followed by the bounded shaping
cache described in the
[native GPU implementation plan](native-gpu-implementation-plan.md).

Descriptive labels and platform accessibility hooks remain valid native
enhancements, but they are not prerequisites for Casey's daily use.
