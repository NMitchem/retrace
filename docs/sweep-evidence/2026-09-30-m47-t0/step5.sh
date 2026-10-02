#!/bin/zsh
# M47 t0 Step 5 (M1(c)): the brief's loop verbatim, as a script (session guard).
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
for i in 1 2 3 4 5; do /private/tmp/claude-501/m47-base-retrace record-dyn $L/t0/madv_dyn -o /private/tmp/claude-501/m47-m1c.bin -- reuse > $L/t0/m1c-$i.out 2> $L/t0/m1c-$i.err; echo "run $i rc=$?"; cat $L/t0/m1c-$i.out; done
