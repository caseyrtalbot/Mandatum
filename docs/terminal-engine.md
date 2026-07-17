# Terminal Engine

## Purpose

The terminal engine converts PTY byte streams into renderer-neutral terminal
state. It should provide terminal correctness without dictating product
architecture or frontend choice.

## Engine Interface

The engine interface is the `TerminalAdapter` trait in
`crates/terminal-vt/src/lib.rs`. It supports:

- create terminal state with a size
- feed raw PTY bytes
- resize
- read visible grid and bounded scrollback (default 2000 rows,
  `DEFAULT_SCROLLBACK_LIMIT`)
- read cursor state and cell style
- read terminal capabilities
- expose the child's mouse-tracking request (`mouse_mode`: DECSET
  9/1000/1002/1003, with SGR 1006 encoding; this is how the app honors L5)
- expose parser errors
- provide deterministic snapshots for tests

## Current Backend

The default backend (`vte_backend.rs`) wraps the `vte` parser crate behind
`TerminalAdapter`; a deterministic fake backend (`fake.rs`) exists for
tests. `TerminalParser` owns one boxed adapter per pane so the app and
renderer never name a concrete backend. The `[L4-GATE]`
adapter-conformance suite in `crates/terminal-vt/tests/` (fixture streams,
mouse-mode exposure, DECSTR release) exercises both the fake backend and
the default parser path; backend swaps land only if fixture parity holds.
The L1 dependency scan additionally forbids `vte` from reaching any
engine-side crate, so parser types cannot leak past this interface.

## Input Encoding Boundary

Frontend adapters emit neutral `scene::input::Key` values. After explicit
workspace chords are resolved, `crates/app/src/input.rs` encodes the remaining
keys for the focused child under the `TERM=xterm-256color` contract used by
terminal and task runtimes. This includes plain Tab (`HT`) and Shift+Tab /
BackTab (`CSI Z`). Frontend differences are normalized at this seam: crossterm
reports Shift+Tab as BackTab with the Shift modifier, while another adapter may
emit Tab with Shift.

Mode-dependent or negotiated keyboard extensions such as modifyOtherKeys or
CSI-u require an explicit terminal capability before they can be claimed; the
baseline encoder must not invent them. Explicit workspace-control chords are
resolved before byte encoding, preserving L5 without shadowing configured
commands.

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
- baseline special-key input encoding, including Shift+Tab / BackTab
- output stress
