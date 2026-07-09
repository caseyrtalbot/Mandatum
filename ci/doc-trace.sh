#!/usr/bin/env bash
# Doc-trace gate: every Constitution law (L1-L5) must be traceable to
#   (a) documentation that states the law, and
#   (b) an executable gate — a `[Lx-GATE]` tag in a CI script or a test file.
# A law with docs but no executable gate is an intention, not a law.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
for law in L1 L2 L3 L4 L5; do
  if ! grep -rqE "\b${law}\b" docs/; then
    echo "doc-trace: ${law} is not documented under docs/"
    fail=1
  fi
  if ! grep -rq "\[${law}-GATE\]" ci/ crates/; then
    echo "doc-trace: ${law} has no executable gate ([${law}-GATE] tag in ci/ or crates/)"
    fail=1
  fi
done

[ "$fail" -eq 0 ] && echo "doc-trace: all laws documented and gated"
exit "$fail"
