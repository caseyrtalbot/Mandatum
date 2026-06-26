# Ghostty And libghostty Evaluation

## Recommendation

Do not fork Ghostty as the first move.

Evaluate `libghostty-vt` or related libghostty surfaces behind a narrow terminal parser adapter. Treat Ghostty as a terminal substrate inspiration and possible dependency, not as the product architecture.

Current spike result: `docs/libghostty-vt-feasibility-spike.md` found
`libghostty-vt` feasible as a future optional backend, but not ready to bind in
this repo until the Zig/CMake toolchain and upstream API pinning are explicit.
The compiled default backend is now the local Rust `vte` parser behind
`TerminalAdapter`; the fake adapter remains for fixtures only.

## Why Not Fork First

Forking Ghostty would inherit:

- Zig core decisions
- macOS app decisions
- GTK/Linux app decisions
- existing terminal-emulator product scope
- upstream maintenance burden
- release and compatibility pressure
- a codebase optimized for Ghostty's product, not this workspace

This product's differentiator is the developer workspace above the terminal substrate. Forking too early risks spending the project budget maintaining a terminal emulator instead of building the workspace.

## What To Learn From Ghostty

Use these as architectural reference points:

- terminal quality and platform fit matter, even though this repo will not adopt Ghostty's macOS app stack
- terminal correctness matters
- rendering performance matters
- parser performance matters
- GUI and terminal core should be separable
- app features and terminal features are different layers
- terminal apps need modern protocol support

## Adapter Boundary

Create a `terminal-vt` interface that can support:

- feed bytes
- resize
- read visible grid
- read cursor state
- read style attributes
- read scrollback metadata
- read mouse protocol mode
- encode input if supported
- expose feature/capability flags
- reset
- snapshot for testing

This keeps the project free to try:

- libghostty-vt
- another parser
- a temporary fake parser
- a platform-native terminal view

## Evaluation Criteria

Evaluate terminal substrates against:

- correctness
- API stability
- licensing
- language interop
- platform support
- Unicode behavior
- modern terminal protocol support
- scrollback model
- mouse and key encoding support
- performance under heavy output
- memory behavior
- embedding complexity
- release cadence
- testability

## Spike Plan

1. Build a fake parser adapter for tests. Done.
2. Build simple stream fixtures. Done.
3. Feed recorded command output into the adapter. Done.
4. Evaluate `libghostty-vt` against the adapter boundary. Done; see
   `docs/libghostty-vt-feasibility-spike.md`.
5. If a future binding is approved, add it as an optional backend behind
   `terminal-vt` only.

## Decision Rule

Use libghostty-vt if it materially reduces terminal correctness risk without forcing the app architecture into Ghostty's product shape.

Defer or avoid it if API volatility, language interop, rendering coupling, or build complexity dominates the milestone.
