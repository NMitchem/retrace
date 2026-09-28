#!/bin/bash
# M45 Task 3: read the refused second kevent_qos's change-list entry out of the partial recording,
# by stopping the debugger at the stub svc (pc 0x1804afa44, one before the trap pc 0x1804afa48, which is the return address) on its second hit. Measurement only.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m45-amt2.bin --script 'break 0x1804afa44; continue; where; regs; x 0x27ff348 72; continue; where; regs; x 0x27ff298 72; x 0x27fedb8 64' > $L/t3-wall-entry.log 2>&1; echo "debug exit=$?"
cat $L/t3-wall-entry.log
