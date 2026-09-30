#!/bin/bash
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
L=.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
for f in after-default after-two after-wall timer; do echo "== m3-$f"; /usr/bin/python3 /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad/m3parse.py $L/t0/m3-$f.log; done
