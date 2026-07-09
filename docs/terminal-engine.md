# Terminal Engine

## Purpose

The terminal engine converts PTY byte streams into renderer-neutral terminal
state. It should provide terminal correctness without dictating product
architecture or frontend choice.

## Engine Interface

The terminal parser interface must support:

- create terminal state with a size
- feed raw PTY bytes
- resize
- read visible grid
- read scrollback
- read cursor state
- read cell style
- read terminal capabilities
- expose parser errors
- reset
- provide deterministic snapshots for tests

## Current Backend

The current backend uses a Rust VT parser path and exposes terminal grid value
types through `terminal-vt`. That backend is valid as long as it preserves the
engine interface and passes terminal conformance tests.

## Optional Backends

A future backend can be introduced when it materially improves:

- terminal correctness
- parser performance
- protocol support
- Unicode behavior
- image/graphics protocol support
- mouse/key encoding
- scrollback model
- frontend interoperability

Candidate backends must stay behind the terminal engine interface.

## Ghostty-Class Criteria

Use Ghostty as a quality reference for:

- correctness under real shell and TUI workloads
- smooth visual output
- modern terminal protocols
- crisp text and color behavior
- responsive input
- careful separation between terminal state and product UI

Do not copy another terminal emulator's product shape into Mandatum. The
workstation experience lives above the terminal substrate.

## Backend Acceptance Gate

A new terminal backend is acceptable only when it proves:

- byte-feed compatibility with the engine interface
- equivalent or better fixture coverage
- no parser/runtime leakage into `core`
- controlled build requirements
- deterministic tests
- clear error behavior
- measurable quality or capability improvement

## Terminal Conformance Scope

Conformance tests should cover:

- plain text
- CR/LF behavior
- wrapping
- SGR styles
- true color
- cursor addressing
- erase display and line
- scroll regions
- alternate screen
- resize
- invalid UTF-8 handling
- bounded scrollback
- child TUI behavior
- output stress
