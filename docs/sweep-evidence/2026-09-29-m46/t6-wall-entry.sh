#!/bin/bash
# M46 Task 6: read the refused kevent_id's (375) change-list entry out of the partial recording, by
# stopping the debugger at the stub svc (pc 0x1804afa70, one before the trap pc 0x1804afa74, which
# is the return address) on its first hit. Also dumps the id (x0, a guest heap address) and the
# landmark trace. Measurement only (the M45 t3-wall-entry.sh shape).
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers || exit 2
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m46-amt.bin --script 'break 0x1804afa70; continue; where; regs; x 0x27ff458 72; x 0x6bc90 160' > $E/automationmodetool.entry.txt 2>&1; echo "debug exit=$?"
cat $E/automationmodetool.entry.txt
{
  echo "# The recorded trace's own landmarks (Reader::open_checked; #0 is the Snapshot). The trace ends"
  echo "# at the 3409 reply: the kevent_id (375) panicked before its event was appended."
  echo "== automationmodetool (/private/tmp/claude-501/m46-amt.bin)"
  $S/tracedump/target/release/tracedump /private/tmp/claude-501/m46-amt.bin
} > $E/landmarks.txt 2>&1; echo "tracedump exit=$?"
