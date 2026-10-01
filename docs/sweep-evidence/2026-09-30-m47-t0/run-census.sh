#!/bin/zsh
# M47 t0 Step 3 wrapper: the brief's census invocation, as a script (session guard).
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
L=$PWD/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
GOUT=$(ls -td target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)
echo "GOUT=$GOUT"
date
sh $L/t0/census.sh /private/tmp/claude-501/m47-census-retrace $GOUT $L/t0 $PWD $L/t0/census.tsv > $L/t0/census.progress 2>&1; echo "exit=$?"
date
tail -3 $L/t0/census.progress
