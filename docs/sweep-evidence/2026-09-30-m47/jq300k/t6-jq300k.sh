#!/bin/bash
# M47 Task 6 (added by the controller): does Task 3b's mach_vm_map mask fix clear jq's 300k abort?
# docs/current-state.md records `jq -n '[range(0;N)] | add'` exiting 134 at N=300000 under retrace, on
# record and replay alike, where native exits 0; its last traps are a mach_vm_map (−15) with a 4 MiB
# alignment mask and the malloc-large tag — the class Task 3b fixed.
# Native once, then three rounds alternating the base binary (427fa0a's code) and the swept copy
# (retrace-t6, a8a1ecd). Each binary records AND replays its own trace (TRACE_MAGIC differs). Each phase
# is bounded by a 900 s watchdog (the apple-sweep.sh shape: kill -9 and a marker). Run after the sweep
# and its controls, with no cargo running. Traces live in the scratchpad and are removed at the end.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
D=$W/docs/sweep-evidence/2026-09-30-m47/jq300k
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
BASE=/private/tmp/claude-501/m47-base-retrace
T6=$S/retrace-t6
JQ=/opt/homebrew/bin/jq
PROG='[range(0;300000)] | add'
mkdir -p "$D" "$S/jq300k"
TMO=900
bounded() { # bounded <marker> cmd... ; returns the command's exit status, touches <marker> if killed
  rm -f "$1"; local m=$1; shift
  "$@" &
  local c=$!
  ( sleep $TMO; kill -9 $c 2>/dev/null && touch "$m" ) &
  local w=$!
  wait $c; local st=$?
  kill $w 2>/dev/null; wait $w 2>/dev/null
  return $st
}
echo "jq=$JQ ($($JQ --version)) realpath=$(python3 -c "import os; print(os.path.realpath('$JQ'))")"
echo "base sha256=$(shasum -a 256 $BASE | cut -d' ' -f1)"
echo "t6   sha256=$(shasum -a 256 $T6 | cut -d' ' -f1)"
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)"
$JQ -n "$PROG" < /dev/null > $D/native.out 2> $D/native.err
echo "native rc=$? stdout=[$(cat $D/native.out)] stderr-bytes=$(wc -c < $D/native.err | tr -d ' ')"
for r in 1 2 3; do
  for b in base t6; do
    if [ $b = base ]; then B=$BASE; else B=$T6; fi
    t=$S/jq300k/r$r-$b.bin; rm -f $t
    s=$(date +%s)
    bounded $S/jq300k/.tmo $B record-dyn $JQ -o $t -- -n "$PROG" < /dev/null > $D/r$r-$b.rec.out 2> $D/r$r-$b.rec.err
    rc=$?; [ -e $S/jq300k/.tmo ] && rc="$rc(TIMED OUT)"
    rs=$(( $(date +%s) - s ))
    s=$(date +%s)
    bounded $S/jq300k/.tmo $B replay $t < /dev/null > $D/r$r-$b.rp.out 2> $D/r$r-$b.rp.err
    rp=$?; [ -e $S/jq300k/.tmo ] && rp="$rp(TIMED OUT)"
    ps_=$(( $(date +%s) - s ))
    cmp -s $D/r$r-$b.rec.out $D/r$r-$b.rp.out; c=$?
    echo "r$r $b record rc=$rc ${rs}s stdout=[$(head -c 80 $D/r$r-$b.rec.out)] | replay rc=$rp ${ps_}s stdout=[$(head -c 80 $D/r$r-$b.rp.out)] | cmp=$c | trace-bytes=$(stat -f %z $t 2>/dev/null) | load=$(sysctl -n vm.loadavg)"
    echo "   rec.err last: $(tail -1 $D/r$r-$b.rec.err | cut -c1-200)"
    echo "   rp.err first DIVERGENCE: $(grep -a -m1 DIVERGENCE $D/r$r-$b.rp.err | cut -c1-200)"
    rm -f $t
  done
done
echo "end $(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)"
