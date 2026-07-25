# Contributing to Mandatum

Mandatum welcomes focused bug reports, design discussions, documentation
improvements, and code contributions. The project is pre-release, so opening an
issue before a large change is the best way to confirm that the work fits the
current direction.

Please report security issues privately as described in
[SECURITY.md](SECURITY.md).

## Set up a development checkout

Mandatum uses the Rust toolchain pinned in `rust-toolchain.toml`.

```sh
git clone https://github.com/caseyrtalbot/Mandatum.git
cd Mandatum
./ci/gate.sh
```

Run the native macOS application from source:

```sh
cargo run -p mandatum-native --bin mandatum-native
```

Run the terminal frontend:

```sh
cargo run -p mandatum-app --bin mandatum
```

## Before opening a pull request

Run the same gate used by CI:

```sh
./ci/gate.sh
```

It checks formatting, Clippy with warnings denied, the workspace build and test
suite, the public distribution contract, native-frontend maintenance,
architectural conformance, and documentation traceability.

Keep changes narrow and explain the behavior they change. Bug fixes should add
a regression test at the lowest useful seam. Tests for agent behavior must use
the deterministic `FakeConnector`; the live Claude CLI tests are intentionally
excluded from CI.

The five architectural invariants in
[docs/constitution.md](docs/constitution.md) are enforced by the gate. If a
proposal appears to conflict with one, open an issue to discuss the boundary
before investing in an implementation.

## Pull requests

A useful pull request includes:

- the problem and intended user outcome;
- the implementation approach and important tradeoffs;
- the verification performed;
- screenshots or a short recording when visible behavior changes; and
- updated documentation when behavior, paths, or configuration change.

Use a clear commit subject. Conventional prefixes such as `feat:`, `fix:`,
`docs:`, `test:`, and `chore:` are welcome but not required.

## Releases

Releases are maintainer-operated and tag-driven. Pushing an ordinary commit to
`main` does not update user installations. The release workflow validates the
tagged source, builds the supported artifacts, verifies checksums, and publishes
a GitHub Release.
