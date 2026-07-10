## What

<!-- One or two sentences: what changes and why. -->

## Checklist

- [ ] `./ci/gate.sh` is green locally (fmt, clippy, build, test, conformance, doc-trace)
- [ ] Bugs: a failing test reproduced the issue before the fix
- [ ] Behavior changes: docs reconciled in this PR (and `docs/decisions.md` for judgment calls)
- [ ] No Constitution law weakened (see `docs/constitution.md`); conformance gates untouched or strengthened
- [ ] Agent-related changes tested through the FakeConnector
