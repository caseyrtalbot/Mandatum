# Developer Workflows

## Primary Workflow: Open Project

1. User opens command palette.
2. User selects project.
3. Workspace restores known layout or creates a default session.
4. Focus lands in primary terminal pane.
5. Status surface shows repo, branch, running tasks, and agent state.

## Primary Workflow: Build And Test

Current task-runtime slice:

1. User opens the command palette.
2. User invokes `Run Task` (`Ctrl-P`, then `b`).
3. Product opens a task pane with durable command intent.
4. The configured shell command runs in project context through app-owned PTY
   runtime.
5. Output streams live in the task pane.
6. Running, succeeded, and failed status is visible.
7. With the task pane focused, user invokes `Rerun Task` (`Ctrl-P`, then `r`) to
   replace the app-owned runtime for the same pane and durable command intent.
8. With the task pane focused, user invokes `Stop Task` (`Ctrl-P`, then `c`) to
   cancel a pending launch or terminate a running task without serializing live
   process state.

Later workflow work:

- named build/test/dev-server recipes
- task pane reuse
- command history
- forwarding a failure to an agent

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
2. Workspace restores project, layout, and pane specs from durable state.
3. Visible terminal panes launch fresh live PTYs for the restored layout.
4. Task pane command intent restores, but old task processes do not auto-relaunch.
5. Task/agent runtime recovery remains later workflow work.
6. User can restart panes or reset workspace.
