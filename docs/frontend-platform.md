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

Tagged releases publish one universal macOS artifact, `Mandatum.app.zip`,
with a SHA-256 sidecar. The bundle layout is the shipping contract:

```text
Mandatum.app/
  Contents/Info.plist
  Contents/MacOS/Mandatum                  (universal native binary)
  Contents/MacOS/mandatum-approval-bridge  (resolved as a sibling)
  Contents/Resources/mandatum              (command-line launcher)
  Contents/Resources/Mandatum.icns
  Contents/Resources/LICENSE
```

`install.sh` verifies the checksum, installs the app to `/Applications` or
`~/Applications`, and copies the launcher to `~/.local/bin/mandatum` after
the app swap succeeds, so a failed swap leaves the previous updater in
place. `mandatum` opens the app in the current directory; `mandatum update`
re-runs the hosted installer, which refuses downgrades and restores the
prior app when a replacement fails.

Binaries are ad-hoc signed. The project claims checksum verification and CI
provenance, not Apple notarization, so a browser-downloaded zip triggers a
Gatekeeper warning while the installer path avoids quarantine entirely.

Public binary releases currently target Apple Silicon and Intel macOS. The
terminal frontend is a development surface and is not distributed. Other
platforms are outside the supported release path.

## Verification

`./ci/gate.sh` is the authoritative repository gate. It runs formatting,
Clippy, builds, tests, native maintenance checks, architecture conformance, and
documentation traceability on the pinned Rust toolchain.

Additional native measurement, displayed-capture, resize, recovery, and fault
procedures live in [verification.md](verification.md). Historical measurement
records remain under `spikes/frontend-wgpu`; they are not product guarantees.
