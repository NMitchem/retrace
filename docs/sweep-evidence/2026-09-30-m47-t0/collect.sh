#!/bin/zsh
# M47 t0 Step 13: copy the evidence (never a .bin, a built binary or a repository directory) from
# the ledger into docs/sweep-evidence/2026-09-30-m47-t0/.
setopt null_glob
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
L=$W/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47-t0
rm -rf $E; mkdir -p $E
cd $L/t0
cp *.txt *.log *.out *.err *.tsv *.lldb *.sh *.py *.patch *.c $E/
cp $L/t0-base-build.log $L/t0-census-build.log $E/
for d in m1b m4 m4-retry m3b m1b-why logflake logflake2 logflake3; do
  mkdir -p $E/$d
  cp $d/*.out $d/*.err $d/*.state $d/*.txt $E/$d/ 2>/dev/null
done
mkdir -p $E/m4-run1
cp m4-run1/log.rec.err m4-run1/log.native.out m4-run1/log.rec.out $E/m4-run1/
find $E -name '*.bin' -print
find $E -type f | wc -l
du -sh $E
