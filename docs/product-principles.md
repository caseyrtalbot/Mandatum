# Product Principles

## Product Thesis

Mandatum is a terminal-native workspace for developers. It borrows the mental model of tmux and zellij, the surface-quality ambition of Ghostty, and the workflow needs of modern coding with build systems, tests, logs, and agents.

It is not an IDE. It is a coding session control surface.

The user should feel that the product understands a real development loop:

1. choose a project
2. open a workspace
3. create terminal panes
4. run editor, shell, build, tests, servers, and agents
5. watch output and health
6. rearrange context quickly
7. preserve session state
8. resume without rebuilding the mental map

## Product Category

Use this framing:

```text
Terminal-native workspace
```

Avoid these as primary labels:

- IDE
- code editor
- terminal emulator clone
- dashboard
- AI IDE
- project manager

## Intended User

The initial user is a developer who is comfortable in terminals and already uses tools such as tmux, zellij, Ghostty, iTerm, Neovim, Helix, Zed, Cursor, Claude Code, Codex, cargo, npm, uv, make, docker, and shell scripts.

They want less friction coordinating work, not a simplified terminal.

## Core Promise

Make the development session feel native, spatial, recoverable, and commandable.

The user should always know:

- what project they are in
- what panes are running
- what commands are active
- what failed
- what needs attention
- what agents are doing
- how to jump, split, run, review, and recover

## Product Pillars

### 1. Terminal First

The terminal pane is the atomic unit. Shells, editors, REPLs, tests, servers, logs, and agents all run as terminal-backed surfaces unless there is a strong reason to create a native non-terminal surface.

### 2. Workspace Native

A workspace is not just a window. It is durable intent: project, layout, panes, commands, running tasks, agent threads, status, history, and user-defined recipes.

### 3. Commandable

Every meaningful action should be reachable through a command palette and bindable to a key.

### 4. Recoverable

The product should gracefully recover from app restart, machine sleep, process exit, parser failure, pane crash, and corrupted session files.

### 5. Beautiful Under Load

The product should look best during real work, not during an empty welcome screen. Dense output, multiple panes, failures, long-running commands, and agent logs must remain readable.

### 6. Terminal-Native Where It Matters

Use terminal-native affordances for panes, keyboard flow, mouse support where available, copy/paste, command discovery, status, and recovery. Do not depend on Apple-native GUI frameworks for the product surface.

### 7. Renderer-Neutral Core

The workspace model should outlive the first terminal renderer and runtime. Keep core state portable and testable.

## Non-Goals

Do not build these in early milestones:

- built-in general-purpose code editor
- language server platform
- full debugger
- extension marketplace
- file explorer as primary navigation
- chat-first UI
- task-management board
- cloud sync
- team collaboration
- visual notebook
- web dashboard

## Product Anti-Patterns

Avoid:

- replacing shell conventions with proprietary equivalents
- hiding raw command output behind summaries
- turning every task into a card
- overusing sidebars
- making the command palette the only way to discover state
- fighting child TUI applications for input and mouse control
- building a fragile abstraction over terminals before terminal correctness is established

## Quality Bar

The product should feel:

- fast
- quiet
- stable
- precise
- terminal-native
- keyboard fluent
- visually restrained
- trustworthy during failures
