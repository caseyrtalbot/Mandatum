# Contributing to Mandatum

Thanks for looking under the hood. This repo runs on two documents and one
script; internalize those and the rest follows.

## The gate is the review

```sh
./ci/gate.sh
```

Runs fmt, clippy with `-D warnings`, build, the full test suite, the
dependency-conformance scans, and doc-trace. CI executes exactly this
script on the pinned toolchain (`rust-toolchain.toml`). A change that
reddens the gate does not land; there are no exceptions, including for
documentation (doc-trace is part of the gate).

## The Constitution is not up for debate in a PR

[docs/constitution.md](docs/constitution.md) defines five laws (engine and
frontend separation; core as a runtime-free leaf; durable intent separate
from live runtime; terminal quality behind `TerminalAdapter`; terminal
soul). Each is enforced by a gate. If your feature seems to need a law
broken, the boundary of your feature is wrong, not the law; open an issue
to discuss the design instead.

## How to work

- **Bugs get a failing test first**, then the fix. Live-PTY behavior has a
  harness pattern in `crates/app/tests/terminal_smoke.rs`.
- **Agents use the FakeConnector in tests**, always. The two live Claude
  CLI tests are `#[ignore]` and run explicitly.
- **Every changed line traces to the change's purpose.** No drive-by
  refactors or restyling.
- **Prefer focused modules under 800 lines.** Existing oversized modules
  should shrink when touched; do not grow one without documenting why a
  split would make the design worse.
- **New judgment calls go to `docs/decisions.md`** (status, decision,
  context, rationale, consequences). Docs are reconciled to code in the
  same change that alters behavior.
- **Latency-sensitive changes** re-run the standing probe:
  `cargo run --release --bin tui_probe` from `spikes/frontend-wgpu`
  (procedure in [docs/verification.md](docs/verification.md)).

## Commit style

`<type>: <description>` where type is one of
feat, fix, refactor, docs, test, chore, perf, ci.

## Publishing a release

A normal push to `main` runs CI but does not replace the version users receive.
User downloads are deliberately tag-driven so every shipped build has a stable
version and rollback point.

1. Change the single `version` in `[workspace.package]` in `Cargo.toml`.
2. Run `./ci/gate.sh`, commit the versioned release state, and ensure `main` is
   current on `origin`.
3. Create an annotated tag with the same version and push it:

```sh
git tag -a v0.2.0 -m "Mandatum v0.2.0"
git push origin main v0.2.0
```

The tag starts `.github/workflows/release.yml`, which reruns the full gate,
builds four native archives, verifies their checksums, and publishes the GitHub
Release consumed by `mandatum update`. Do not expose this maintainer operation
as `mandatum push`: the public command updates an installation and requires no
repository permissions.

## Setup

```sh
git clone https://github.com/caseyrtalbot/Mandatum.git
cd Mandatum
./ci/gate.sh          # rustup installs the pinned toolchain automatically
cargo run -p mandatum-app --bin mandatum
```
