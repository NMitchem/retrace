#!/bin/bash
# M2 with a per-run watchdog (lldb killed after 300 s).
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
run1() { # $1 = script, $2 = log, rest = program + args
  local S=$1 LOG=$2; shift 2
  lldb -b -s $L/t0/$S -- "$@" > $L/t0/$LOG 2>&1 &
  local P=$!
  ( sleep 300; kill $P 2>/dev/null && echo "watchdog killed lldb ($LOG)" ) &
  local W=$!
  wait $P; local rc=$?
  kill $W 2>/dev/null
  return $rc
}
for m in "" two; do run1 m2.lldb m2-after-${m:-default}.log $L/t0/after_dyn $m; echo "after '$m' exit=$?"; done
run1 m2.lldb m2-timer.log $L/t0/timer_dyn; echo "timer exit=$?"
grep -a -E ' x4 = ' $L/t0/m2-*.log | sort | uniq -c
