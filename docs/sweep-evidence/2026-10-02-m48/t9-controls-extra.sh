#!/bin/bash
# M48 Task 9, a t9 addition beyond the brief: controls.txt's d5-t9 round met a third dddiagnose face
# (RECORD ERROR: data abort at far 0x10, UNMAPPED, pc 0x193bc20ec = libswiftCore
# swift::RefCounts<…>::incrementSlow + 88), on the t9 binary once in six, on base never. Ten more
# rounds of dddiagnose alone, alternating base and t9 exactly as t9-controls.sh's d-rounds do (its
# run function, the sweep's own labels and watchdog), so each binary has sixteen samples.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
S=/private/tmp/claude-501/m48-t9-ctl
BASE=/private/tmp/claude-501/m48-base-retrace
T9=/private/tmp/claude-501/m48-t9-debug-retrace
cd "$W" || exit 2
echo "base sha256=$(shasum -a 256 $BASE | cut -d' ' -f1)"
echo "t9   sha256=$(shasum -a 256 $T9 | cut -d' ' -f1)"
echo /usr/bin/dddiagnose > $S/ctl-ddd.txt
run() { # run <label> <binary> <list>
  local d=$S/ctl/$1
  rm -rf "$d"; mkdir -p "$d"
  export RETRACE_SWEEP_LIST=$3
  export RETRACE_SWEEP_KEEP=$d
  echo "== $1 start $(date '+%H:%M:%S') load=$(sysctl -n vm.loadavg)"
  tools/apple-sweep.sh "$2" 2>&1 | grep -a -E '^(PASS|FAIL|SKIP|TALLY)' | cut -c1-230
  for f in "$d"/*.rec.err; do
    [ -e "$f" ] || continue
    c=$(grep -a -m1 'guest crashed:' "$f")
    [ -n "$c" ] && echo "   $(basename "$f" .rec.err): $c"
  done
  unset RETRACE_SWEEP_LIST RETRACE_SWEEP_KEEP
}
for i in 7 8 9 10 11 12 13 14 15 16; do
  run x$i-base $BASE $S/ctl-ddd.txt
  run x$i-t9 $T9 $S/ctl-ddd.txt
done
echo "end $(date '+%H:%M:%S') load=$(sysctl -n vm.loadavg)"
