# Developer Workflows

Each workflow lists what works today and what remains aspirational. "Built"
claims trace to the driven demo (`examples/live-slice/run.sh`, the stranger
test in docs/verification.md) and the test suite; "Not yet built" items are
targets, not descriptions of the product.

## Open Project And Sessions

Built:

1. User opens Mandatum in a project directory.
2. A saved workspace restores; with no saved workspace, a first-run note
   orients (palette chord, right-click menu, help key, quit chord).
3. Layout, focus, and pane intent restore; the focused pane receives input.
4. The header attention strip shows what needs eyes (approvals, failed
   tasks, blocked agents); when calm it shows workspace, session, pane
   count, and agent connector kind.
5. New session creates and focuses a fresh session under the active project;
   it does not duplicate the project, reuse a same-id pane's prior runtime, or
   imply a project chooser exists.

Not yet built: project chooser (the project is the launch directory),
branch display in the header.

## Build And Test

Built:

1. User runs the configured task (`[task] default_command`; palette `b`)
   or reruns the focused task pane.
2. A task pane opens (or the focused one reruns); output streams live with
   status.
3. Failure is visible in the pane, the header attention strip ("1 task
   failed · <pane>"), the session map state column, and the timeline.
4. User can rerun, stop, copy output (copy mode), and search output
   (session search).
5. The execution timeline records task starts and exits with the command
   string and exit status (`crates/app/src/timeline.rs`).
6. On a known failure, Investigate task failure with agent creates a durable
   agent mandate with the task command, resolved cwd, failure status, and the
   last 24 nonblank output lines (each capped at 240 characters). The output
   and every other task fact are bounded, JSON-escaped, line-prefixed, and
   explicitly labeled untrusted evidence. Only typed process-exit,
   launch-failure, and rerun-failure facts enable the handoff; transient
   parser/reader/resize/wait errors do not. The agent launches through the
   normal connector and approval gate.

Not yet built: named multi-recipe catalog (one configured task command
exists today; `TaskRecipe` in `crates/workflows` shapes durable intent
only), task history with cwd/duration/start-time fields.

## Dev Server Loop

Built: a long-running command runs as an ordinary task pane (the live-slice
demo's heartbeat pane) with rerun/stop and live output.

Not yet built: server-specific surface (port state, health probes, open
URL, restart-vs-rerun semantics).

## Agent Supervision

Built:

1. User starts an agent with an objective (Set agent objective, palette
   `p`; Start agent, palette `g`).
2. The agent pane shows objective, status, current action, latest summary,
   changed files, and output tail.
3. Pending approvals surface in the pane (command, scope, risk band) and
   as a header attention segment ("1 approval waiting · <pane>").
4. User can approve (`y`) or reject (`n`) directly, stop the agent, and
   cycle to the next waiting agent (palette `j`).
5. Approval decisions persist in durable intent (`approval_history`) and
   the timeline.

Not yet built: open the full agent thread, checks/verification surface on
the pane, agent handoff documents (see docs/agent-runtime.md, "Not Yet
Built").

## Failure Triage

Built:

1. A failed task or agent shows near the actor (pane status, title flag)
   and globally (attention strip, session map, timeline).
2. Clicking an attention segment (or Focus next waiting agent) jumps to
   the pane in need.
3. User can copy output, rerun the task, or relaunch the agent (failed
   agent panes show the relaunch hint).
4. From a failed task, Investigate task failure with agent creates and starts
   a separate agent pane. Save/restore keeps the mandate but never invents a
   live agent session or replays the launch.

Not yet built: mark-as-acknowledged.

## Review Changes

Built: agent panes list the files the agent reported changing (count plus
the last ~10 paths), durable across restart.

Not yet built: workspace-level change detection, a changed-file summary
overlay, jump to editor/diff, verification state attached to changed-file
context.

## Resume Workstation

Built:

1. App start loads durable workspace intent (layout, panes, focus, task
   and agent intent, approval history).
2. Terminal panes launch fresh runtimes; task and agent panes restore as
   intent and require explicit rerun/relaunch (side effects never replay
   silently).
3. Live-session claims are detached on restore: running/waiting agent
   statuses fold to `unknown`, pending approval ids clear (history stays).
4. The execution timeline persists and records that prior work ran.

Not yet built: a restore report that itemizes what resumed versus what
needs action (today the folding rules guarantee honesty; the summary
surface does not exist).
