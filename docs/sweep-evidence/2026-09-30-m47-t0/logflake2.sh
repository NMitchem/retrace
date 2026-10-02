#!/bin/zsh
# M47 t0: logflake.sh's base runs all stopped at the M33 panic for chdir (12), because `-C` is a
# chdir and base has no row for 12. Re-run the comparison the probe's git12 way: cwd = the repo, no
# `-C` (git then issues no chdir), on BOTH binaries, N each, and count rc 134.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
O=$L/t0/logflake2
N=${1:-10}
mkdir -p $O
d=/private/tmp/claude-501/m47-logflake-repo
cd $d
env -i $G log -1 > $O/native.out 2>&1; echo "native rc=$?"
for bin in base census; do
  R=/private/tmp/claude-501/m47-$bin-retrace
  for i in $(seq 1 $N); do
    perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o /private/tmp/claude-501/m47-logflake.bin -- log -1 > $O/$bin-$i.out 2> $O/$bin-$i.err; rc=$?
    cmp -s $O/native.out $O/$bin-$i.out && same=yes || same=no
    echo "logflake2 bin=$bin run=$i rc=$rc stdout==native:$same $(grep -a -m1 -E 'guest terminated|panicked at|RECORD ERROR' $O/$bin-$i.err | cut -c1-120)"
  done
done
