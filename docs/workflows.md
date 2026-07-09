# Developer Workflows

## Open Project

1. User opens Mandatum.
2. User chooses or restores a project.
3. Workspace opens the last useful session or a default session.
4. Primary terminal/editor pane receives focus.
5. Status strip shows project, branch, running tasks, agents, and attention
   items.

## Build And Test

1. User runs a build/test recipe from the palette or keymap.
2. A task pane opens or reuses an existing recipe pane.
3. Output streams live with status.
4. Failure state is visible in the pane and session map.
5. User can rerun, stop, copy failure output, search output, or send the failure
   to an agent.
6. Task history records command, cwd, start, exit, duration, and result.

## Dev Server Loop

1. User starts a dev-server recipe.
2. Server pane shows command, cwd, port, status, and recent output.
3. Health probes and port state are visible.
4. User can restart, stop, open URL, or inspect logs.

## Agent Supervision

1. User starts an agent with an objective and scope.
2. Agent pane appears with state and latest action.
3. Pending approvals become global attention items.
4. Changed files and verification results are summarized.
5. User can inspect, approve, reject, stop, or open the full thread.
6. Agent handoff remains attached to the session.

## Failure Triage

1. A task, shell, server, or agent fails.
2. Failure indicator appears near the actor and in global status.
3. User jumps to the failing surface.
4. User can copy output, rerun, start an agent, open changed files, or mark as
   acknowledged.

## Review Changes

1. Workspace detects changed files.
2. Session map and agent panes show change counts.
3. User opens changed-file summary.
4. User jumps to editor, diff tool, or agent result.
5. Verification state stays attached to the changed-file context.

## Resume Workstation

1. App starts and loads durable workspace intent.
2. Layout, panes, task intent, and agent intent restore.
3. Live terminal panes launch fresh runtimes when appropriate.
4. Side-effecting tasks and agents stay visible as intent until explicitly
   relaunched.
5. Restore status explains what resumed and what needs action.
