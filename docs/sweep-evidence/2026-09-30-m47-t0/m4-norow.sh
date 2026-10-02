#!/bin/zsh
# M47 t0 M4: per command, which of the rowcheck's NOROW numbers (and 2, 138) it dispatched.
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
for f in $L/t0/m4/*.rec.err $L/t0/m4-retry/*.rec.err $L/t0/m3b/git-commit.err $L/t0/m3b/forkfail.err; do
  echo "${f#$L/t0/}: $(grep -a -o -E '^\[trap\] num=(2|9|12|83|136|138|333) ' $f | sed 's/.*=//' | sort -un | tr '\n' ' ')"
done
