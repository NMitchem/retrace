#!/bin/bash
# M48 Task 9 (spec §3i): the full Apple corpus on a signed scratch copy of this task's debug binary,
# detached, with no cargo running in this worktree (t9-build.sh ran every build first). M47's
# t6-sweep.sh shape, gated on an idle host (Ruling T9-a): it refuses to start at a 1-minute load
# of 3 or more.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
B=/private/tmp/claude-501/m48-t9-debug-retrace
cd "$W" || exit 2
load1=$(sysctl -n vm.loadavg | awk '{print $2}')
if awk -v l="$load1" 'BEGIN { exit !(l >= 3) }'; then echo "REFUSED: 1-minute load $load1 >= 3 at $(date '+%Y-%m-%d %H:%M:%S %Z')"; exit 3; fi
export RETRACE_SWEEP_KEEP=$E/sweep
( while :; do echo "$(date '+%H:%M:%S') $(sysctl -n vm.loadavg)"; sleep 30; done ) > $E/sweep-load.txt 2>&1 &
sampler=$!
{
  echo "pidstart=$$"
  echo "binary=$B sha256=$(shasum -a 256 "$B" | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo "load-start: $(uptime) vm.loadavg=$(sysctl -n vm.loadavg)"
  tools/apple-sweep.sh "$B"
  echo "SWEEP_EXIT=$?"
  echo "load-end: $(uptime) vm.loadavg=$(sysctl -n vm.loadavg) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
} > "$E/sweep.log" 2>&1
kill $sampler 2>/dev/null
echo "wrapper done"
