#!/bin/bash
# M46 Task 6 Step 1 — the brief's command lines, unchanged.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers || exit 2
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
mkdir -p $E
export RETRACE_TRACE=1
cargo build -p retrace > $L/t6-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /usr/bin/automationmodetool -o /private/tmp/claude-501/m46-amt.bin > $E/automationmodetool.rec.out 2> $E/automationmodetool.rec.err; echo "record exit=$?"
grep -a -c '^\[trap\] ' $E/automationmodetool.rec.err
tail -5 $E/automationmodetool.rec.err
