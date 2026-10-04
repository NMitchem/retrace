#!/bin/bash
# M48 Task 9 Step 6 controls: each moved row (moved-rows.txt) re-swept by tools/apple-sweep.sh itself
# (RETRACE_SWEEP_LIST, the sweep's own labels and 30 s watchdog), on the base binary (t0's
# m48-base-retrace, 50e716f = M47's code) and on the swept copy (m48-t9-debug-retrace, 4e37b88),
# alternating: base, t9, base, t9. Then dddiagnose alone, four more rounds alternating (its two faces
# are a coin flip, M45/M46), so each binary has six dddiagnose samples. Every run records AND replays
# with one binary (the same TRACE_MAGIC, RT\x00\x0b, is on both binaries). Each non-clean row's
# evidence is kept per run, and its crash line (if any) printed. No cargo runs meanwhile.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
S=/private/tmp/claude-501/m48-t9-ctl
BASE=/private/tmp/claude-501/m48-base-retrace
T9=/private/tmp/claude-501/m48-t9-debug-retrace
cd "$W" || exit 2
echo "base sha256=$(shasum -a 256 $BASE | cut -d' ' -f1)"
echo "t9   sha256=$(shasum -a 256 $T9 | cut -d' ' -f1)"
grep -v '^#' $E/moved-rows.txt > $S/ctl-rows.txt
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
run r1-base $BASE $S/ctl-rows.txt
run r1-t9 $T9 $S/ctl-rows.txt
run r2-base $BASE $S/ctl-rows.txt
run r2-t9 $T9 $S/ctl-rows.txt
for i in 3 4 5 6; do
  run d$i-base $BASE $S/ctl-ddd.txt
  run d$i-t9 $T9 $S/ctl-ddd.txt
done
echo "end $(date '+%H:%M:%S') load=$(sysctl -n vm.loadavg)"
