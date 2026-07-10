#!/usr/bin/env bash
# The live slice: one command sets up and launches the demo workspace a
# stranger is shown — a rerunnable check that passes then fails, a
# heartbeat "dev server", and a fake agent that requests an approval and
# waits. See examples/live-slice/README.md for the walkthrough.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
DIR="${1:-$REPO/examples/live-slice/demo-workspace}"

mkdir -p "$DIR/.mandatum"

# The rerunnable check: passes on the first run, fails (exit 3) on the
# second, alternating — so two reruns show one success and one failure in
# the pane, the attention strip, and the timeline.
cat >"$DIR/flaky-check.sh" <<'SH'
#!/bin/sh
if [ -f .flip ]; then
  rm .flip
  echo "FAIL: simulated flaky check (marker file .flip present)"
  exit 3
fi
touch .flip
echo "OK: checks passed"
SH
chmod +x "$DIR/flaky-check.sh"

# Project config: the deterministic fake connector (no network, scripted
# approval flow) and the check as the default task command.
cat >"$DIR/.mandatum/config.toml" <<'TOML'
[agent]
connector = "fake"

[task]
default_command = "sh ./flaky-check.sh"
TOML

# The durable workspace file, generated through the real core API.
cargo run -q -p mandatum-app --manifest-path "$REPO/Cargo.toml" \
  --example make_live_slice -- "$DIR"

cat <<'TXT'

live slice ready. Drive it (each step is one or two keys):

  1. focus "dev server" (click it, or ctrl+p l)   ctrl+p r  -> heartbeats
  2. focus "checks"                               ctrl+p r  -> OK (exit 0)
  3. rerun it                                     ctrl+p r  -> FAIL (exit 3)
     - the header attention strip now shows "1 task failed"
  4. focus the floating agent pane                ctrl+p g  -> agent runs,
     requests approval, waits; header shows "1 approval waiting"
     - y approves (agent completes) / n rejects (agent fails)
  5. ctrl+p /  -> execution timeline (filter, enter jumps to a pane)
  6. ctrl+p m  -> session map
  7. ctrl+p w saves; ctrl+q quits; relaunching restores the workspace and
     the timeline still holds every fact above.

launching Mandatum in the demo workspace...
TXT

cd "$DIR"
exec cargo run -q -p mandatum-app --manifest-path "$REPO/Cargo.toml"
