#!/bin/bash
# M47 Task 6 Step 3 (spec §3g; M45 T3-a; ledger Ruling T6-a): the full corpus on a signed scratchpad copy
# of this task's binary, detached, with no cargo running in this worktree (every build ran first:
# t6-build.sh). The M38/M39/M44/M45/M46 wrapper shape — pidstart, the binary's hash/commit/date, the
# sweep, SWEEP_EXIT — with the load average before and after (`uptime` and `sysctl -n vm.loadavg`),
# a 30 s load sampler for the sweep's duration (sweep-load.txt), and the xcrun cache's state at the
# start (the trio's path depends on /var/tmp/xcrun_db, M44 t0 Ruling T0-e; this sweep does not touch it).
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
B=$S/retrace-t6
cd "$W" || exit 2
mkdir -p "$E" "$S"
cp target/aarch64-apple-darwin/debug/retrace "$B" || exit 2
codesign -f -s - --entitlements retrace.entitlements "$B" 2>/dev/null || exit 2
export RETRACE_SWEEP_KEEP=$E/sweep
( while :; do echo "$(date '+%H:%M:%S') $(sysctl -n vm.loadavg)"; sleep 30; done ) > $E/sweep-load.txt 2>&1 &
sampler=$!
{
  echo "pidstart=$$"
  echo "binary=$B sha256=$(shasum -a 256 "$B" | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo "load-start: $(uptime) vm.loadavg=$(sysctl -n vm.loadavg)"
  echo "xcrun_db-start: $(ls -la /var/tmp/xcrun_db 2>&1)"
  tools/apple-sweep.sh "$B"
  echo "SWEEP_EXIT=$?"
  echo "load-end: $(uptime) vm.loadavg=$(sysctl -n vm.loadavg) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
} > "$E/sweep.log" 2>&1
kill $sampler 2>/dev/null
echo "wrapper done"
