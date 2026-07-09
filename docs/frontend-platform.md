# Frontend Platform Strategy

## Purpose

Mandatum's product is not defined by a single frontend. The engine and scene
model should support multiple adapters so the product can serve local, remote,
terminal, native, and high-polish use cases.

## Frontend Roles

### Terminal Frontend

Use for:

- fast development verification
- remote sessions
- SSH-friendly operation
- automation-friendly smoke tests
- fallback when no native frontend is available

Requirements:

- preserve child terminal input
- render terminal grids and pane chrome clearly
- support command palette
- support copy/scrollback baseline
- support task and agent panes
- remain lightweight and testable

### Native/GPU Frontend

Use for:

- smooth resizing
- high-quality text rendering
- precise pointer interaction
- low-latency scrollback
- richer selection behavior
- animation and visual polish
- accessibility integration
- platform-native menus and window behavior

Requirements:

- consume the same scene layer as the terminal frontend
- keep product behavior in engine modules
- expose input and hit-test events to runtime/command routing
- measure frame pacing, latency, memory, and CPU
- support automated smoke verification

### Platform-Specific Frontend

Use when platform fit materially improves the product.

Examples:

- macOS-native windowing and text rendering
- platform clipboard, accessibility, and menu integration
- platform GPU APIs where they improve quality and performance

Requirements:

- product state remains in the shared engine
- platform code remains behind adapter interfaces
- build and verification steps are explicit
- cross-platform assumptions are not smuggled into `core`

## Decision Criteria

Evaluate frontend options against:

- startup time
- input latency
- resize latency
- scroll latency
- frame pacing under output
- text crispness and shaping
- glyph fallback
- color fidelity
- selection precision
- mouse/pointer support
- accessibility hooks
- crash recovery
- test automation
- packaging complexity

## Required Spike

Before committing to a native/GPU frontend, build one vertical slice:

1. open a window
2. render one live PTY-backed terminal grid
3. support typing and paste
4. support resize
5. support scrollback and selection
6. render one task/agent status strip
7. measure latency and frame pacing
8. run an automated smoke check

The spike succeeds only if it proves a user-visible quality gain over the
terminal frontend without duplicating workstation behavior.
