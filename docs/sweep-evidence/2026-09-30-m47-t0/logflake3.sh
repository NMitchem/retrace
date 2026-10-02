#!/bin/zsh
# M47 t0: logflake2 saw `log -1` abort 1/10 on the BASE binary too. Identify that abort: record
# `log -1` (cwd = repo, no -C) on base, one trace per run, until a run exits 134 (at most 25), then
# read x22 and the frame chain the probe's way (commit-abort.txt), symbolicated as m1b-why was.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=/private/tmp/claude-501/m47-base-retrace
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
O=$L/t0/logflake3
T=/private/tmp/claude-501/m47-logflake3
mkdir -p $O $T
cd /private/tmp/claude-501/m47-logflake-repo
for i in $(seq 1 25); do
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $T/run.bin -- log -1 > $O/run.out 2> $O/run.err; rc=$?
  echo "logflake3 run=$i rc=$rc"
  if [ $rc = 134 ]; then
    cp $O/run.err $O/abort-run-$i.err
    echo "--- terminal stop"
    $R debug $T/run.bin --script 'continue; where; regs' 2>&1 | tail -12
    $R debug $T/run.bin --script 'continue; x 0x180328e07 64; x 0x27fc000 16384' > $O/abort-stack.txt 2>&1
    break
  fi
done
