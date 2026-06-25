# Ghostty And libghostty Evaluation

## Recommendation

Do not fork Ghostty as the first move.

Evaluate `libghostty-vt` or related libghostty surfaces behind a narrow terminal parser adapter. Treat Ghostty as a terminal substrate inspiration and possible dependency, not as the product architecture.

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

1. Build a fake parser adapter for tests.
2. Build a simple PTY stream fixture.
3. Feed recorded command output into the adapter.
4. Render a simple terminal grid through the renderer contract.
5. Replace fake adapter with libghostty-vt spike.
6. Compare behavior and integration cost.
7. Decide whether libghostty-vt is a dependency, optional backend, or deferred.

## Decision Rule

Use libghostty-vt if it materially reduces terminal correctness risk without forcing the app architecture into Ghostty's product shape.

Defer or avoid it if API volatility, language interop, rendering coupling, or build complexity dominates the milestone.
