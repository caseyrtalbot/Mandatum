# Technology Direction

## Accepted Constraint

This project is a terminal/Codex build.

Do not use:

- Xcode.app
- `.xcodeproj`
- SwiftUI
- AppKit
- Metal
- MetalKit
- CoreText-dependent renderer work
- Apple-native GUI surfaces
- notarization-first packaging

The project must be buildable, testable, and runnable from terminal commands that Codex can execute and verify.

## Recommended Stack

Default to a Rust workspace unless `/plan` finds a concrete blocker.

Recommended early shape:

```text
crates/core          pure workspace/session/layout/action model
crates/pty           PTY/process lifecycle
crates/terminal-vt   terminal parser adapter boundary
crates/renderer      terminal renderer and pane chrome
crates/app           terminal application runtime
crates/commands      command palette, keymaps, action registry
crates/workflows     builds, tests, tasks, agent surfaces
```

Recommended command surface:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run
```

## Why Rust First

Rust gives the best outcome for a terminal/Codex build:

- mature terminal and PTY ecosystem
- strong test tooling
- simple command-line verification
- good serialization/config support
- strong enough systems control for PTY and event loops
- easier for Codex to build incrementally than a mixed native stack
- no Xcode or Apple GUI dependency

Rust is not chosen for cross-platform ambition. It is chosen because this repo should be terminal-first, testable, and maintainable through command-line workflows.

## Where Zig Fits

Zig remains a possible later component for:

- libghostty-vt integration
- terminal parser experiments
- low-level renderer experiments
- C ABI adapter modules

Do not make Zig the whole app unless a spike proves Rust blocks terminal correctness or libghostty integration.

If Zig is added, keep it behind a narrow boundary:

```text
Rust app/core -> C ABI -> Zig/libghostty adapter
```

## Where Ghostty Fits

Ghostty remains a quality reference and possible terminal-substrate source.

Do not fork Ghostty.

Do not adopt Ghostty's macOS app architecture.

Evaluate `libghostty-vt` only behind `terminal-vt` after the Rust core and fake parser adapter exist.

## What This Means For `/plan`

Plan should assume:

- Rust workspace
- terminal app runtime
- terminal renderer first
- fake terminal parser first
- no Apple-native app stack
- no Xcode
- all verification through shell commands

Plan may challenge Rust only if it names a concrete terminal, PTY, parser, or rendering blocker.
