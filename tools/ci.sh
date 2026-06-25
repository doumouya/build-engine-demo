#!/usr/bin/env sh
# The CI gate, runnable from a fresh clone. Exit code = failure count (health-check style).
# Grows with the project — today it runs the audit ratchet; add the project's test/build gates
# (e.g. `cargo test`, a node:test runner, a bundle self-check) as `run` lines below.
set -u
fails=0
run() { echo "== $*"; "$@" || fails=$((fails + 1)); }

cd "$(dirname "$0")/.."

# node may live under nvm in dev shells; resolve it for non-interactive runs.
if ! command -v node >/dev/null 2>&1; then
  export NVM_DIR="$HOME/.nvm"
  [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh" >/dev/null 2>&1
fi

# The audit ratchet runs the whole tools/*-audit suite and fails only on REGRESSIONS vs
# tools/ci-audit/baseline.json — existing findings are the floored backlog, not a hard block.
run sh tools/ci-audit/check.sh

echo "ci: $fails failure(s)"
exit "$fails"
