#!/usr/bin/env bash
# One high-effort LHTB-46 cell, smoke-gated. One cell per box, so the two cells
# of this pair run concurrently on separate hardware instead of serially.
#
# Usage: lhtb-high.sh <label> <manifest-stem> <jobs-subdir> <required-seat>...
#
# n=8, NOT the previous LHTB wave's 4. `override_cpus` is 4 per trial container,
# so n=8 is exactly the 32 vCPU box, 1:1 -- the ceiling, not oversubscription.
# BOTH cells of this pair must run at 8 or the comparison is void: mismatched
# -n compressed the between-condition spread by 0.09 in the TB wave.
#
# The smoke gate is not optional here. Three of the four things it checks
# (seat addressing, the phase loop firing, per-phase receipts) fail silently --
# they produce a plausible-looking bad number rather than an error, and you find
# out two days and several hundred dollars later.
set -uo pipefail

LABEL="${1:?usage: lhtb-high.sh <label> <manifest> <jobs> <seat>...}"
MANIFEST="${2:?usage: lhtb-high.sh <label> <manifest> <jobs> <seat>...}"
JOBS="${3:?usage: lhtb-high.sh <label> <manifest> <jobs> <seat>...}"
shift 3
SEATS=("$@")
N=8

# Resolve siblings relative to this script, so it works both from the repo
# checkout and from a copy dropped in $HOME on a benchmark box.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd "$HOME/buzz/benchmarks/harbor-buzz-orchestra" || exit 1

# Leaked containers from an earlier run keep holding CPU, which makes the real
# concurrency higher than -n claims and quietly breaks the -n match.
docker ps --format '{{.Names}}' | grep -v '^buzz-benchmark-' \
  | xargs -r docker rm -f > /dev/null 2>&1 || true

echo "=== $(date -u +%H:%M:%SZ) [$LABEL] SMOKE $MANIFEST (great-expectations-audit, 0.15x)"
"$HERE/lhtb-smoke.sh" "$MANIFEST" "${JOBS}-smoke" > "$HOME/${JOBS}-smoke.log" 2>&1
echo "=== $(date -u +%H:%M:%SZ) [$LABEL] smoke exited rc=$?"

python3 "$HERE/gatecheck.py" "jobs/${JOBS}-smoke" "${SEATS[@]}"
gate=$?

if [ "$gate" -ne 0 ]; then
  echo "=== $(date -u +%H:%M:%SZ) [$LABEL] gate FAILED -- NOT starting the full run."
  echo "===        inspect ~/${JOBS}-smoke.log and"
  echo "===        jobs/${JOBS}-smoke/*/*/agent/buzz/*.stdout.log"
  exit 1
fi

echo "=== $(date -u +%H:%M:%SZ) [$LABEL] gate PASSED, starting full 46-task run (n=$N)"
"$HERE/lhtb46.sh" "$MANIFEST" "$JOBS" "$N" > "$HOME/${JOBS}.log" 2>&1
echo "=== $(date -u +%H:%M:%SZ) [$LABEL] full run exited rc=$?"
