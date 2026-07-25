# Mandatum Roadmap

Mandatum is a GPU-native development workstation for macOS. It keeps shells,
tasks, long-running commands, agent activity, approvals, artifacts, and
recovery in one spatial session.

This file points forward. Stable product behavior belongs in the public docs;
accepted technical decisions and dated verification evidence remain in
`docs/decisions.md` and `docs/verification.md`.

## Current Product

The native macOS frontend and terminal frontend share one renderer-neutral
`WorkspaceScene` and one application/runtime engine. Current capabilities
include:

- tiled, stacked, floating, zoomed, and restorable workspace layouts;
- live PTY terminals with bounded scrollback and explicit input ownership;
- task panes with rerun behavior and visible outcomes;
- agent panes with summaries, changed-file reporting, and a fail-closed
  shell-command approval path;
- a command palette, context menu, session map, timeline, help, output search,
  prompts, and first-run guidance;
- bounded project-relative PNG artifact previews;
- dark, light, and high-contrast native themes with bundled fonts; and
- native surface/device recovery, scale handling, IME input, clipboard, and
  event-driven redraw.

The public limitations are equally explicit:

- durable intent restores, but live processes do not;
- named task and dev-server recipe catalogs are not implemented;
- the default agent policy gates shell commands, while reads and writes are
  auto-allowed;
- native macOS accessibility projection and VoiceOver are not implemented;
  and
- artifact preview is intentionally limited to bounded project-relative PNGs.

## Distribution

Version `0.2.0` was the last split-archive, terminal-only release. The
current contract ships the desktop application:

- tagged releases publish one universal `Mandatum.app.zip` (Apple Silicon
  and Intel) with a SHA-256 sidecar, assembled by
  `packaging/package-app.sh` from per-architecture CI builds;
- the approval bridge rides inside the bundle beside the app executable,
  and the command-line launcher rides at `Contents/Resources/mandatum`;
- `install.sh` verifies the checksum, installs the app to `/Applications`
  (or `~/Applications`), and installs the `mandatum` launcher last;
- `mandatum` opens the app in the current directory, and `mandatum update`
  re-runs the hosted installer, which refuses downgrades and rolls back a
  failed swap; and
- binaries are ad-hoc signed, so publishing requires no Apple credentials
  or repository secrets.

The terminal frontend remains a development surface and is not distributed.
See the "Distribution Ships One App Bundle And One Launcher" decision.

## Ordered Work

1. **Publish the first signed native macOS release.**
   Configure the documented GitHub Actions secrets, run the authoritative
   repository gate, push a matching semantic-version tag, then verify both
   architectures through a real fresh install and update.
2. **Named task and dev-server recipes.**
   Add project-local named recipes, lifecycle state, command-palette routes,
   and truthful restart/restore behavior behind one catalog boundary.
3. **Pane title-rail geometry.**
   Introduce an explicit layout/PTY contract before increasing title rails
   beyond the current terminal-row geometry.
4. **Native accessibility projection.**
   Map the existing typed accessibility nodes and actions into the macOS
   accessibility tree, then qualify keyboard and VoiceOver behavior.

An editor surface, arbitrary document preview, non-macOS distribution, and
broad agent-provider expansion remain outside the current milestone.

## Release Standard

A capability is complete only when:

- its real user path works;
- focused tests cover the boundary that can regress;
- active documentation states only verified behavior;
- `./ci/gate.sh` ends exactly `GATE GREEN`; and
- the change is committed with a clear next product step.

For installation and user-facing behavior, start with [README.md](README.md).
For implementation boundaries, see [docs/architecture.md](docs/architecture.md)
and [docs/constitution.md](docs/constitution.md).
