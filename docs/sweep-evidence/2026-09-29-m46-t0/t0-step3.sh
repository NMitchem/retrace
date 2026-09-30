#!/bin/bash
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
export RETRACE_TRACE=1
cargo build -p retrace > $L/t0-m1-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0/after_dyn -o /private/tmp/claude-501/m46-clock.bin -- clock > $L/t0/m1a-clock.out 2> $L/t0/m1a-clock.err; echo "clock record=$?"
cat $L/t0/m1a-clock.out
grep -a -E '^\[trap\] num=116 ' $L/t0/m1a-clock.err
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0/after_dyn -o /private/tmp/claude-501/m46-after0.bin > $L/t0/m1a-after.out 2> $L/t0/m1a-after.err; echo "after record=$?"
grep -a -E '^\[trap\] num=116 ' $L/t0/m1a-after.err | awk '{print $NF}' | sort | uniq -c
grep -a 'panicked at' $L/t0/m1a-after.err | head -2
