#!/bin/zsh
# M47 t0 follow-up to M1(b)/M4: `git log -1` (read-only; no M47 row, no madvise call) aborted once
# under the census build with libmalloc's "pointer being freed was not allocated" (m4-run1/log.rec.err,
# zero num=75 traps). Is that abort already on main? Record `log -1` N times on the BASE binary (the
# unpatched aa16f01 CLI, which the probe's git12 showed passing once) and on the census binary, in
# one repo, and count rc 134.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
O=$L/t0/logflake
N=${1:-10}
mkdir -p $O
d=/private/tmp/claude-501/m47-logflake-repo
rm -rf $d; mkdir -p $d
env -i $G -C $d init -q -b main; print one > $d/a.txt; env -i $G -C $d add a.txt
env -i $G -C $d -c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false commit -q -m first
env -i $G -C $d log -1 > $O/native.out 2>&1; echo "native rc=$?"
for bin in base census; do
  R=/private/tmp/claude-501/m47-$bin-retrace
  for i in $(seq 1 $N); do
    perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o /private/tmp/claude-501/m47-logflake.bin -- -C $d log -1 > $O/$bin-$i.out 2> $O/$bin-$i.err; rc=$?
    cmp -s $O/native.out $O/$bin-$i.out && same=yes || same=no
    echo "logflake bin=$bin run=$i rc=$rc stdout==native:$same $(grep -a -m1 -E 'guest terminated|panicked at|RECORD ERROR' $O/$bin-$i.err | cut -c1-120)"
  done
done
