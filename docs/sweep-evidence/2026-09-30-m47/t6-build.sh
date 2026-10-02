#!/bin/bash
# M47 Task 6: every cargo build this task needs, run FIRST (before the walk's records and long before
# the sweep), so no cargo runs while anything is being recorded (M45 T3-a).
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
L=$W/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cd "$W" || exit 2
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') commit=$(git rev-parse --short HEAD)"
cargo build -p retrace > $L/t6-build.log 2>&1
echo "build exit=$?"
echo "end $(date '+%Y-%m-%d %H:%M:%S %Z')"
