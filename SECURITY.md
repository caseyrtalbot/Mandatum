# Security Policy

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub's security advisory
flow: [Report a vulnerability](https://github.com/caseyrtalbot/Mandatum/security/advisories/new).
Do not open a public issue for anything exploitable.

You can expect an acknowledgment within a week. Mandatum is pre-release
software; there is no embargo program, but reports are triaged ahead of all
other work.

## Scope notes

Areas of particular interest:

- **The approval bridge** (`mandatum-approval-bridge`): it is designed to
  fail closed; any input, socket, or timing condition that makes it emit an
  allow decision on a failure path is a vulnerability.
- **The agent runtime boundary**: any way for a replaced or dead runtime's
  events to mutate durable state, or for live runtime state (sockets,
  tokens, handles) to reach a persisted file.
- **Workspace persistence**: symlink or path tricks against
  `.mandatum/workspace.json` and `.mandatum/timeline.jsonl` (both writers
  reject symlinks and cap sizes; bypasses are vulnerabilities).
- **VT parsing**: crashes or state corruption from hostile terminal output
  (the parser is fuzz-hardened by test fixtures; new hostile inputs
  welcome).

## Supported versions

Pre-release: only the tip of `main` is supported.
