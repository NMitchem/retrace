#!/bin/bash
# M47 Task 6: isolate the mask fix for jq's 300k abort. t6-jq300k.sh compared the base binary with the swept
# one, which carries ALL of M47 (rows, madvise, __mac_syscall, fork, the mask fix). This repeats the run on
# the Task 3b diagnosis's pair, which differ ONLY by the mask fix (docs/sweep-evidence/2026-09-30-m47-abort/
# README.md's binary table):
#   rt0    = 090bf5e (t0's measurements commit; aa16f01's code), unpatched            sha256 9301e771…
#   rt-fix = 090bf5e + fix.diff (the mach_vm_map mask fix alone, no magic bump)       sha256 43f00caf…
# Three rounds alternating, each binary replaying its own trace, the same 900 s watchdog per phase.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
D=$W/docs/sweep-evidence/2026-09-30-m47/jq300k
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
A=/private/tmp/claude-501/m47-abort
JQ=/opt/homebrew/bin/jq
PROG='[range(0;300000)] | add'
mkdir -p "$D" "$S/jq300k"
TMO=900
bounded() {
  rm -f "$1"; local m=$1; shift
  "$@" &
  local c=$!
  ( sleep $TMO; kill -9 $c 2>/dev/null && touch "$m" ) &
  local w=$!
  wait $c; local st=$?
  kill $w 2>/dev/null; wait $w 2>/dev/null
  return $st
}
echo "rt0    sha256=$(shasum -a 256 $A/rt0 | cut -d' ' -f1)"
echo "rt-fix sha256=$(shasum -a 256 $A/rt-fix | cut -d' ' -f1)"
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)"
for r in 1 2 3; do
  for b in rt0 rt-fix; do
    B=$A/$b
    t=$S/jq300k/i$r-$b.bin; rm -f $t
    bounded $S/jq300k/.tmo $B record-dyn $JQ -o $t -- -n "$PROG" < /dev/null > $D/i$r-$b.rec.out 2> $D/i$r-$b.rec.err
    rc=$?; [ -e $S/jq300k/.tmo ] && rc="$rc(TIMED OUT)"
    bounded $S/jq300k/.tmo $B replay $t < /dev/null > $D/i$r-$b.rp.out 2> $D/i$r-$b.rp.err
    rp=$?; [ -e $S/jq300k/.tmo ] && rp="$rp(TIMED OUT)"
    cmp -s $D/i$r-$b.rec.out $D/i$r-$b.rp.out; c=$?
    echo "i$r $b record rc=$rc stdout=[$(head -c 80 $D/i$r-$b.rec.out)] | replay rc=$rp stdout=[$(head -c 80 $D/i$r-$b.rp.out)] | cmp=$c | load=$(sysctl -n vm.loadavg)"
    echo "   rec.err last: $(tail -1 $D/i$r-$b.rec.err | cut -c1-200)"
    echo "   rp.err first DIVERGENCE: $(grep -a -m1 DIVERGENCE $D/i$r-$b.rp.err | cut -c1-200)"
    rm -f $t
  done
done
echo "end $(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)"
