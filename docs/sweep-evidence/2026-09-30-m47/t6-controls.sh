#!/bin/bash
# M47 Task 6 Step 3 controls: each moved row (moved-rows.txt) re-swept by tools/apple-sweep.sh itself
# (RETRACE_SWEEP_LIST, the sweep's own labels and 30 s watchdog), on the base binary (427fa0a's code,
# t0's m47-base-retrace) and on the swept copy (retrace-t6, a8a1ecd), alternating: base, t6, base, t6.
# Then dddiagnose alone, four more rounds alternating (its two faces are a coin flip, M45/M46), so each
# binary has six dddiagnose samples. Every run records AND replays with one binary (TRACE_MAGIC differs:
# RT\x00\x0a on base, RT\x00\x0b on t6). Each non-clean row's evidence is kept in the scratchpad per
# run, and its crash line (if any) printed. No cargo runs meanwhile.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
BASE=/private/tmp/claude-501/m47-base-retrace
T6=$S/retrace-t6
cd "$W" || exit 2
echo "base sha256=$(shasum -a 256 $BASE | cut -d' ' -f1)"
echo "t6   sha256=$(shasum -a 256 $T6 | cut -d' ' -f1)"
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
run r1-t6 $T6 $S/ctl-rows.txt
run r2-base $BASE $S/ctl-rows.txt
run r2-t6 $T6 $S/ctl-rows.txt
for i in 3 4 5 6; do
  run d$i-base $BASE $S/ctl-ddd.txt
  run d$i-t6 $T6 $S/ctl-ddd.txt
done
echo "end $(date '+%H:%M:%S') load=$(sysctl -n vm.loadavg)"
