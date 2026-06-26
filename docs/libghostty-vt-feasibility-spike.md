# libghostty-vt Feasibility Spike

Date: 2026-06-25

Upstream snapshot inspected:

- Repository: `https://github.com/ghostty-org/ghostty`
- HEAD: `f9194f93deeec82670771fc3909132b37356b155`
- Local inspection copy: `/private/tmp/ghostty-libghostty-spike`
- Context7 library used: `/websites/libghostty_tip_ghostty`
- Rust binding repository inspected: `https://github.com/Uzaaft/libghostty-rs`
- Rust binding HEAD: `81b776c64fa96d8be0b243e0ebc383887c51eb38`

## Result

`libghostty-vt` is feasible as a future optional `terminal-vt` backend, but do
not bind it directly in this repo yet.

The public C API has the core pieces this project needs:

- terminal allocation and teardown
- raw VT byte feeding
- resize
- cursor and terminal metadata reads
- screen/grid/cell/style access
- render-state snapshots for renderer-oriented traversal
- key, mouse, focus, paste, SGR, OSC, and formatter helper APIs

The blocker is not capability. The blocker is integration stability and local
verification. Upstream explicitly marks the API as work in progress, and this
machine currently has neither `zig` nor `cmake` on `PATH`, so a Rust FFI binding
or linked adapter cannot be verified in this phase.

## Evidence

Upstream describes `libghostty` as a cross-platform, zero-dependency C and Zig
library for building or embedding terminal functionality. It says
`libghostty-vt` is available and usable for Zig and C across macOS, Linux,
Windows, and WebAssembly, while also saying API signatures are still in flux and
`libghostty` has not been tagged with a version yet.

Primary source locations inspected:

- Ghostty README:
  - `README.md:31-37`
  - `README.md:145-170`
- C API umbrella header:
  - `include/ghostty/vt.h:4-12`
  - `include/ghostty/vt.h:15-27`
  - `include/ghostty/vt.h:124-148`
- Terminal API:
  - `include/ghostty/vt/terminal.h:45-84`
  - `include/ghostty/vt/terminal.h:160-178`
  - `include/ghostty/vt/terminal.h:711-795`
  - `include/ghostty/vt/terminal.h:1002-1115`
- Grid/cell/style APIs:
  - `include/ghostty/vt/grid_ref.h:21-31`
  - `include/ghostty/vt/grid_ref.h:81-95`
  - `include/ghostty/vt/grid_ref.h:124-204`
  - `include/ghostty/vt/screen.h:117-208`
  - `include/ghostty/vt/screen.h:300-392`
  - `include/ghostty/vt/style.h:82-131`
- Render-state API:
  - `include/ghostty/vt/render.h:303-426`
  - `include/ghostty/vt/render.h:428-559`
  - `include/ghostty/vt/render.h:561-721`
- Build integration:
  - `build.zig.zon:1-6`
  - `build.zig:116-140`
  - `src/build/Config.zig:298-357`
  - `src/build/GhosttyLibVt.zig:195-313`
  - `src/build/GhosttyLibVt.zig:329-380`
  - `CMakeLists.txt:1-9`
  - `CMakeLists.txt:92-148`
  - `CMakeLists.txt:178-188`
- Examples:
  - `example/c-vt-stream/src/main.c:7-23`
  - `example/c-vt-grid-traverse/src/main.c:7-83`
  - `example/c-vt-render/src/main.c:24-180`
  - `dist/cmake/README.md:1-28`
- Existing Rust binding:
  - `Cargo.toml:1-9`
  - `crates/libghostty-vt-sys/Cargo.toml:1-30`
  - `crates/libghostty-vt-sys/build.rs:5-8`
  - `crates/libghostty-vt-sys/build.rs:319-357`
  - `crates/libghostty-vt/src/lib.rs:50-72`

Local toolchain check:

```sh
zig version
# zsh:1: command not found: zig

cmake --version
# zsh:1: command not found: cmake
```

## Adapter Mapping

The current `TerminalAdapter` seam maps cleanly enough for a future backend:

- `TerminalAdapter::feed(&[u8])`
  - maps to `ghostty_terminal_vt_write(terminal, data, len)`
  - upstream treats input as untrusted bytes and does not fail on malformed
    input, so a real adapter should return `Ok(...)` for invalid UTF-8 rather
    than mirroring the fake adapter's UTF-8 error
- `TerminalAdapter::resize(TerminalSize)`
  - maps to `ghostty_terminal_resize(terminal, cols, rows, cell_width_px,
    cell_height_px)`
  - this repo will need a policy for cell pixel size, probably owned by
    renderer/app rather than `core` or `pty`
- `TerminalAdapter::size()`
  - maps to `ghostty_terminal_get(..., GHOSTTY_TERMINAL_DATA_COLS/ROWS, ...)`
- `TerminalAdapter::grid()`
  - for tests, can be built from `ghostty_terminal_grid_ref` and
    `ghostty_grid_ref_cell`
  - for renderer work, should use `ghostty_render_state_update` and row/cell
    iteration because upstream warns grid refs are not meant to be the core
    render-loop path
- `TerminalCursor`
  - maps to `GHOSTTY_TERMINAL_DATA_CURSOR_X`,
    `GHOSTTY_TERMINAL_DATA_CURSOR_Y`, and
    `GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE`
- `CellStyle`
  - can start with `GhosttyStyle.bold` and `GhosttyStyle.inverse`, but the
    local style model is currently much smaller than upstream's style surface

## Risks

- API stability: high. Upstream headers explicitly say the C API is incomplete,
  work in progress, and expected to change.
- Build complexity: medium-high. Downstream builds require Zig. CMake can wrap
  Zig, but this environment currently lacks both `zig` and `cmake`.
- Rust FFI maintenance: medium. The C surface is broad, uses opaque handles,
  sized structs, tagged unions, callbacks, borrowed lifetimes, and result-code
  conventions.
- Existing Rust bindings: medium-high. `libghostty-vt` / `libghostty-vt-sys`
  exist, but the inspected sys crate defaults to vendored native builds, can
  fetch Ghostty from git when `GHOSTTY_SOURCE_DIR` is unset, and pins a
  different Ghostty commit (`fdbf9ff3...`) than the upstream head inspected in
  this spike.
- Renderer coupling: medium. Grid refs are useful for correctness tests, but
  a performant renderer should use render-state snapshots.
- Boundary risk: low if the binding stays inside `crates/terminal-vt` and is
  optional. There is no reason for `core`, `pty`, `renderer`, or `app` to depend
  on Ghostty directly.
- Licensing: low. The inspected upstream license is MIT.

## Decision

Treat `libghostty-vt` as a promising optional backend, not the default backend
yet.

Do not add a Cargo dependency, vendored source, bindgen output, build script, or
checked-in generated headers in this phase.

Current status update: Milestone 4 later added a local Rust `vte` backend as the
compiled default behind `TerminalAdapter`; the fake adapter is now fixture-only.

## Next Binding Gate

A future binding slice may proceed only after:

1. Pin an upstream commit or released artifact.
2. Install or vendor an explicit Zig toolchain compatible with upstream
   `minimum_zig_version`.
3. Choose dynamic, static, or generated prebuilt integration.
4. Decide whether to use direct FFI, a C/Zig shim, or third-party Rust bindings;
   do not accept a build that fetches Ghostty from the network during normal
   Cargo builds.
5. Keep all FFI and unsafe code inside `crates/terminal-vt`.
6. Add a feature-gated adapter so normal `cargo test` remains green without
   Zig/CMake.
7. Run parity tests against existing stream fixtures for text, wrapping,
   carriage returns, resize, cursor, styles, invalid bytes, and scrollback.
8. Document how Ghostty callbacks that write responses back to the PTY route
   through the future app runtime without making `terminal-vt` depend on
   `pty`.
