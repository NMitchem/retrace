#!/bin/bash
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
lldb -b -o "disassemble -n mach_absolute_time" -o "disassemble -n __commpage_gettimeofday_internal" -- $L/t0/after_dyn > $L/t0/m1c-disasm.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m46-clock.bin --script "x 0xfffffc080 0x60" > $L/t0/m1c-commpage.out 2>&1; echo "exit=$?"
cat $L/t0/m1c-commpage.out
