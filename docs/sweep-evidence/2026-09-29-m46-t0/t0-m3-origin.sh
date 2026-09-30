#!/bin/bash
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
L=.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
for m in "" wall; do lldb -b -s $L/t0/m3-origin.lldb -- $L/t0/after_dyn $m > $L/t0/m3-origin-${m:-default}.log 2>&1; echo "after '$m' exit=$?"; done
