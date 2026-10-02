#!/bin/zsh
# M47 t0 M1(b) follow-up: mode B (advice 7/8 no-op'd) ALSO aborted 3 of 5 (m1b.txt), against the
# probe's g36. Record mode-B commits with RETRACE_TRACE=1, each to its own trace, until one aborts,
# then read the abort the probe's way (commit-abort.txt): x22 at the terminal stop, and the frames.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=/private/tmp/claude-501/m47-census-retrace
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
O=$L/t0/m1b-why
T=/private/tmp/claude-501/m47-m1bwhy
mkdir -p $O $T
ID=(-c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false)
export PROBE_NOREUSABLE=1
for i in 1 2 3 4 5 6 7 8; do
  d=$T/repo-$i; rm -rf $d; mkdir -p $d
  env -i $G -C $d init -q -b main; print one > $d/a.txt; env -i $G -C $d add a.txt
  export RETRACE_TRACE=1
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $T/b-$i.bin -- -C $d $ID commit -q -m first > $O/b-$i.out 2> $T/b-$i.err; rc=$?
  unset RETRACE_TRACE
  echo "why run=$i rc=$rc traps=$(grep -a -c '^\[trap\]' $T/b-$i.err) head=$(env -i $G -C $d rev-parse -q --verify HEAD >/dev/null && echo committed || echo none)"
  if [ $rc = 134 ]; then
    cp $T/b-$i.err $O/b-$i.rec.err
    echo "--- last 40 trap/console lines before the abort"
    grep -a -E '^\[trap\]|^\[fd|^\[probe\]|^\[m47\] num=75' $T/b-$i.err | tail -40 | cut -c1-200
    echo "--- terminal stop: regs"
    $R debug $T/b-$i.bin --script 'continue; where; regs' 2>&1 | tail -14
    break
  fi
done
