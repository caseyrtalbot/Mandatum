# Security Policy

## Reporting a vulnerability

Please report potential vulnerabilities privately through
[GitHub Security Advisories](https://github.com/caseyrtalbot/Mandatum/security/advisories/new).
Do not open a public issue for an exploitable problem or include sensitive
details in a discussion.

Reports should include the affected version, operating system, reproduction
steps, impact, and any suggested mitigation. You can expect an acknowledgment
within seven days.

## Supported versions

Mandatum is pre-release software. Security fixes are applied to the latest
published release and the current `main` branch; older releases may not receive
backports.

## Security-sensitive boundaries

Reports are especially useful for:

- approval requests that allow an action when the approval bridge encounters
  malformed input, a timeout, or another failure;
- stale or replaced runtime events that mutate current durable state;
- live process handles, sockets, credentials, or transient output appearing in
  persisted workspace data;
- path traversal, symlink, archive, or replacement attacks in persistence,
  artifact loading, installation, or updating; and
- crashes or state corruption caused by hostile terminal output.

The default Claude connector gates shell commands and auto-allows file reads
and writes. This policy is connector-dependent. The command-risk label shown
with an approval is a heuristic for human review, not a sandbox or a security
guarantee. Users remain responsible for reviewing commands before approval.
