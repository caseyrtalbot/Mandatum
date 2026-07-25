# Frontend Platform Strategy

## Direction

The GPU-native macOS frontend is Mandatum's primary product surface. A
maintained terminal frontend serves remote, headless, recovery, and
low-dependency use.

Both frontends consume one product model. `AppState`, `RuntimeEngine`,
`FrontendHost`, and `WorkspaceScene` own terminal, task, agent, approval,
persistence, and recovery behavior. Frontends translate platform input and
render scenes; they do not create a second product state machine.

## Native macOS frontend

The native application owns:

- the window and platform event loop;
- GPU surface and device lifecycle;
- font, scale, glyph, and texture resources;
- clipboard, pointer, selection, and IME translation; and
- presentation scheduling and animation.

Its production shell is `crates/native` (`mandatum-native`), built on winit.
Scene-only GPU presentation lives in `crates/native-renderer`, built on wgpu
and glyphon. Measurement, stress, and fault tools stay outside the application
under `spikes/frontend-wgpu`.

The native renderer receives product state only through `FrontendHost` and
paints only `WorkspaceScene`. It supports terminal, task, agent, Empty, and
artifact pane content; application chrome; status and attention surfaces; and
the complete overlay family.

## Terminal frontend

The terminal application (`mandatum`) is intended for:

- SSH and remote operation;
- headless or low-dependency environments;
- recovery when native startup is unavailable;
- deterministic frontend checks; and
- users who prefer an in-terminal surface.

It preserves the same command, layout, runtime, persistence, and input-routing
behavior through the shared scene model. Native-only materials and raster
presentation have explicit terminal fallbacks.

## Shared contract

Every frontend must:

- consume neutral input and typed effects through `FrontendHost`;
- paint from `WorkspaceScene`;
- leave layout, command routing, persistence, runtime identity, and recovery
  policy in shared modules;
- expose platform failures clearly;
- keep live frontend resources out of durable state; and
- support deterministic checks at the deepest practical seam.

`CellProgram` is the complete terminal-parity representation. The native
frontend may consume richer typed scene data, but only through
`mandatum-scene`. Artifact Preview's bounded `RasterSurface` is the reference
pattern: durable intent in core, safe live loading in app, typed pixels in the
scene, native presentation, and an honest terminal fallback.

## Native presentation

The native application currently provides:

- dark, light, and high-contrast theme palettes;
- bundled JetBrains Mono with strict, observable font overrides;
- cell-exact terminal text and separate typography roles for application UI;
- native materials for workspace chrome, panes, overlays, approvals, and
  artifact surfaces;
- reduced-motion behavior;
- resize and display-scale handling;
- typed surface/device recovery and bounded event draining; and
- GPU preflight before restore state or live PTYs are created.

Platform-independent accessibility roles, labels, states, bounds, and actions
exist in the scene model. Native macOS accessibility projection, including
VoiceOver support, is not yet implemented and is not claimed.

Pane title rails currently occupy one terminal row. Making those rails taller
without consuming child-terminal content requires a future layout and PTY
contract change.

## Distribution

Tagged releases are prepared to publish separate Apple Silicon and Intel macOS
artifacts. Each architecture has a common archive:

```text
mandatum
mandatum-approval-bridge
LICENSE
```

and a native archive:

```text
mandatum-native
LICENSE
```

Keeping the common archive's membership stable lets pre-native installations
upgrade their terminal command and embedded installer without rejecting an
unexpected file. The installer treats an equal version as current only when
the complete selected platform set is present, so a terminal-only installation
can acquire the native archive without a version bump or forced downgrade.

The release workflow is configured to sign all three macOS binaries with a
Developer ID Application certificate, enable the hardened runtime, and submit
them to Apple for notarization. Missing credentials or an unsuccessful
signing/notarization step fails publication.

The installer detects the Mac architecture, verifies both release checksums,
validates each binary's Developer ID signature against the pinned Apple Team
ID, and installs all three commands. Newer `mandatum update` versions apply
later published releases and restore the previous command set if replacement
fails. The archives contain command-line binaries rather than a `.app` bundle,
so the project does not claim stapled or offline Gatekeeper tickets.

Linux release archives contain the terminal frontend and approval bridge for
arm64 and x86-64 glibc systems. There is no native Linux GUI release.

## Verification

`./ci/gate.sh` is the authoritative repository gate. It runs formatting,
Clippy, builds, tests, native maintenance checks, architecture conformance, and
documentation traceability on the pinned Rust toolchain.

Additional native measurement, displayed-capture, resize, recovery, and fault
procedures live in [verification.md](verification.md). Historical measurement
records remain under `spikes/frontend-wgpu`; they are not product guarantees.
