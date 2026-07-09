# Product Principles

## Thesis

Mandatum is a development workstation for terminal-centered builders. It brings
shells, editors, builds, tests, servers, agents, diffs, approvals, logs, and
recovery into one spatial session surface.

The product should feel like a terminal environment expanded into a complete
work loop: fast, precise, commandable, inspectable, and calm under load.

## User

The first user is an experienced developer who already moves between terminal
emulators, shell tools, editors, build systems, local servers, repository
commands, and agent sessions.

They do not need a simplified terminal. They need fewer blind spots while many
pieces of work run at once.

## Core Promise

The user can always answer:

- What project and session am I in?
- What panes and processes are running?
- What failed, and what command produced the failure?
- Which agents are active, blocked, or waiting for approval?
- Which files changed?
- What can I rerun, stop, restart, restore, copy, search, or inspect?
- What will survive app restart, machine sleep, or process failure?

## Product Pillars

### 1. Terminal Soul

Terminals remain first-class. Shells, editors, REPLs, test runners, servers, and
agent CLIs should run naturally without the workspace stealing their input.

### 2. Workstation Visibility

The product should reveal running work, failures, approvals, changed files,
ports, commands, task status, and agent state without forcing the user to hunt
through disconnected panes.

### 3. Spatial Control

Panes, stacks, floating surfaces, zoom, status overlays, and session maps should
make work physically legible. Layout is part of memory.

### 4. Commandable Everything

Every meaningful action should be reachable from the command palette and
bindable to keyboard input. Direct manipulation should exist where it is faster:
click, drag, resize, select, scroll, and inspect.

### 5. Recoverable By Design

The workspace persists durable intent and clearly explains live state that could
not be restored. Restarting the app should not destroy the user's mental map.

### 6. Agents As Session Actors

Agents are visible workers in the session. Their panes show objective, status,
current action, approvals, changed files, commands, checks, blockers, and
handoff state.

### 7. Renderer Optionality

Product behavior belongs in the engine and scene model, not in a frontend. A
terminal frontend, native frontend, GPU-backed frontend, or platform-specific
frontend should all be possible without rewriting the workstation model.

## Quality Bar

The product must feel:

- fast under output
- visually crisp
- quiet by default
- dense without clutter
- responsive to keyboard and pointer input
- safe around child terminal apps
- explicit during failures
- recoverable after interruption
- useful before it is feature-rich

## Non-Goals

Do not build these before the workstation loop is strong:

- a general source editor
- a standalone project-management board
- a chat-first agent product
- a decorative dashboard that hides raw output
- a marketplace or extension ecosystem
- a cloud collaboration layer
- an onboarding-first landing surface

Editor, language, debugger, and review integrations are valid later surfaces
when they strengthen session visibility and do not swallow the terminal work
loop.
