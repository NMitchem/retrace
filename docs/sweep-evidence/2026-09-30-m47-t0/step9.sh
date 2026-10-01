#!/bin/zsh
# M47 t0 Step 9 (M3(c)): the brief's commands verbatim, as a script (session guard).
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
D=$L/t0/m3c-repo
rm -rf $D && mkdir -p $D && env -i $G -C $D init -q -b main && echo one > $D/a.txt && env -i $G -C $D add a.txt
env -i $G -C $D -c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false commit -q -m first; echo "setup=$?"
echo two >> $D/a.txt && env -i $G -C $D add a.txt
sh -c "ulimit -u 1; exec env -i $G -C $D -c user.name=retrace -c user.email=retrace@example.invalid commit -q -m second" > $L/t0/m3c.out 2> $L/t0/m3c.err; echo "rc=$?"
echo "--- m3c.out"; cat $L/t0/m3c.out; echo "--- m3c.err"; cat $L/t0/m3c.err; echo "--- log -1"
env -i $G -C $D log -1 --format=%s
