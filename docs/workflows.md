# Developer Workflows

## Primary Workflow: Open Project

1. User opens command palette.
2. User selects project.
3. Workspace restores known layout or creates a default session.
4. Focus lands in primary terminal pane.
5. Status surface shows repo, branch, running tasks, and agent state.

## Primary Workflow: Build And Test

1. User invokes `run build` or `run tests`.
2. Product opens or reuses a task pane.
3. Command runs in project context.
4. Output streams live.
5. Exit status is visible.
6. Failure can be rerun, copied, or sent to an agent.

## Primary Workflow: Agent Thread

1. User starts an agent from command palette or project context.
2. Agent pane appears with objective, state, and recent output.
3. Pending approvals are visible.
4. Changed files and verification results are summarized.
5. User can open the full external thread when needed.

## Primary Workflow: Layout Control

1. User splits pane.
2. User runs different roles in each pane: editor, shell, tests, server, agent.
3. User stacks or zooms panes as context shifts.
4. Layout persists as durable intent.

## Recovery Workflow

1. App restarts.
2. Workspace restores project, layout, pane specs, and recent tasks.
3. Dead processes are marked as exited/restartable.
4. User can restart panes or reset workspace.

