#!/usr/bin/env sh
# ─────────────────────────────────────────────────────────────────────────────
# CI audit-diff check — the regression floor for the project's automated checks.
#
# Auto-discovers every tools/*-audit/audit.js, runs it, then ratchets each tool's violation count
# against a committed baseline (tools/ci-audit/baseline.json). Exits 0 when nothing got worse;
# exits 1 with a markdown table of regressions when any tool has MORE violations than its baseline.
# Tools at or below baseline pass (fixed / improved / unchanged are wins). A new audit folder is
# picked up the day it's added — no edit here.
#
# Usage:
#   sh tools/ci-audit/check.sh                    run audits + ratchet vs baseline
#   sh tools/ci-audit/check.sh --no-run           skip the run; ratchet existing audit.json outputs
#   sh tools/ci-audit/check.sh --update-baseline  accept current counts as the new baseline
# ─────────────────────────────────────────────────────────────────────────────
set -e

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"
HERE="tools/ci-audit"

if ! command -v node >/dev/null 2>&1; then
  export NVM_DIR="$HOME/.nvm"
  [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh" >/dev/null 2>&1
fi
if ! command -v node >/dev/null 2>&1; then
  echo "ci-audit: node not found (needed to run the audit suite). Aborting."
  exit 2
fi

MODE="run"
case "${1:-}" in
  --no-run)          MODE="no-run" ;;
  --update-baseline) MODE="update" ;;
  "")                MODE="run" ;;
  *) echo "ci-audit: unknown arg '$1' (expected --no-run | --update-baseline)"; exit 2 ;;
esac

# ── 1. run the suite (auto-discovered audit.js per tools/*-audit/) ───────────
# Each audit.js exits with its violation count. Tracked-only: untracked tools/*-audit/ dirs are
# ignored so the suite is reproducible from a fresh clone. A non-zero audit exit is data for the
# ratchet, not a hard error here (set -e is scoped around the run).
if [ "$MODE" != "no-run" ]; then
  echo "========================  ci-audit | running suite  ========================"
  for audit in tools/*-audit/audit.js; do
    [ -f "$audit" ] || continue
    git ls-files --error-unmatch "$audit" >/dev/null 2>&1 || continue
    tool="$(basename "$(dirname "$audit")" -audit)"
    echo "--------  $tool  --------"
    set +e
    node "$audit"
    set -e
    echo
  done
fi

# ── 2. ratchet violation counts against the committed baseline ───────────────
echo "========================  ci-audit | checking regressions  ========================"
node "$HERE/ratchet.mjs" "$MODE"
