# Architecture

## Goal

Design a greenfield terminal-native workspace with strict boundaries between durable workspace state, PTY/process handling, terminal parsing, terminal rendering, application runtime, and developer workflow orchestration.

The architecture must support a polished terminal user experience without trapping core behavior inside a terminal UI framework or any Apple-native app stack.

## High-Level Layers

```text
app
  terminal application runtime, lifecycle, config loading, persistence, orchestration

renderer
  terminal drawing, pane chrome, overlays, frame scheduling, themes

commands
  command palette, keymap resolution, action registry, help/discovery surfaces

workflows
  build/test/task recipes, agent threads, logs, status, process groups

terminal-vt
  terminal parser adapter boundary, screen/grid model, capabilities, input encoding

pty
  process spawn, PTY read/write, resize, exit, stream backpressure

core
  workspace/session/layout/pane/action domain model and persistence
```

Dependency direction should move downward only where possible:

```text
app -> renderer -> commands/workflows -> core
app -> pty -> terminal-vt -> renderer
```

The exact dependency graph may vary by implementation detail, but the domain rule is fixed: `core` cannot depend on terminal UI, PTY, parser, render, or platform types.

## Modules

### core

Owns product state and pure behavior:

- workspace identity
- projects
- sessions
- panes
- pane kinds
- split/stack/floating/tab layout
- focus
- zoom
- durable command recipes
- actions
- session serialization schema
- migrations and recovery rules

Core should be heavily unit-tested and runnable without a terminal UI.

Core should not own:

- PTY handles
- threads
- terminal parser objects
- renderer resources
- GPU resources
- platform event types

### pty

Owns OS-facing process mechanics:

- spawn shell/process
- attach PTY
- read stream
- write input bytes
- resize
- child exit
- kill/restart
- backpressure
- process groups

PTY must expose events to the runtime without knowing UI details.

### terminal-vt

Owns the terminal parser adapter boundary:

- process byte streams into terminal state
- expose screen/grid/cursor/style state
- expose mouse protocol state
- encode key and mouse input when appropriate
- track terminal capabilities

The first implementation may use a fake parser for tests. A later spike should evaluate `libghostty-vt` behind this boundary.

### renderer

Owns presentation:

- text grid drawing
- cursor rendering
- selection rendering
- pane chrome
- split separators
- stack strips
- overlays
- command palette drawing
- frame timing
- smooth resize/scroll behavior

Renderer should not decide product actions. It receives scene/state and emits input/hit-test events.

### app

Owns terminal application orchestration:

- terminal initialization/restoration
- lifecycle
- config loading
- persistence timing
- status/error routing
- top-level event loop
- clipboard
- crash/panic restoration policy

### commands

Owns command discovery and dispatch metadata:

- command palette entries
- command search/filtering
- keymap loading
- conflict detection
- action labels
- help overlay content
- command availability rules

Command dispatch should call core/workflow/app services through explicit actions.

### workflows

Owns developer-session workflows:

- build recipes
- test recipes
- dev server recipes
- logs
- command history
- agent threads
- approval surfaces
- diff/review references
- health/status probes

Workflow code should not create layout mutations directly. It should request core actions.

## Runtime Event Model

Prefer a central event loop with typed events:

- app events
- key events
- pointer events
- PTY output
- child exit
- terminal parser updates
- workflow/task status
- agent status
- render tick
- resize
- persistence timer
- command invocation

Events should be explicit and testable.

## Durable State

Persist intent, not live handles.

Persist:

- workspace id/name
- project paths
- pane specs
- layout tree
- focus
- command recipes
- task definitions
- user preferences
- selected theme/keymap
- last known working directory per pane

Do not persist:

- PTY handles
- thread handles
- parser objects
- GPU resources
- process ids as durable truth
- raw unbounded scrollback in the first milestone

## Failure Model

Plan for:

- child process exits
- PTY spawn failure
- terminal parser error
- config parse failure
- session schema mismatch
- corrupted session file
- renderer initialization failure
- app restart
- machine sleep/wake
- stuck agent/task

Failures should be visible but not catastrophic. A failed pane should remain inspectable and restartable.

## Greenfield Technology Posture

Use the accepted terminal/Codex constraint as the technology starting point.

Current recommendation:

- Rust workspace first
- terminal application runtime first
- no Xcode, SwiftUI, AppKit, Metal, or Apple-native GUI dependency
- renderer-neutral core
- parser adapter boundary suitable for `libghostty-vt`
- no Ghostty fork in the initial path

## Architecture Validation Checklist

Before accepting implementation:

- Can `core` tests run without terminal UI?
- Can a fake terminal parser be used in tests?
- Can renderer be swapped without changing session state?
- Can workspace layout be serialized without runtime handles?
- Can commands be discovered without launching the terminal app?
- Can PTY failure be represented in domain state?
- Can agent/task status be represented without becoming a chat UI?
