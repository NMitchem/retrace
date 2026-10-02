#!/bin/bash
# M47 Task 6: read node's refused kevent's (363) arguments out of the partial recording, by stopping the
# debugger at the stub svc (pc 0x1804b3fc0, one before the trap pc 0x1804b3fc4, the return address) on
# its first hit — the trace has no earlier 363 — and dumping the two-entry change list at x1 and the
# timespec at x5. Then the trace's own landmarks (the throwaway reader `td`, source tracedump.rs here).
# Measurement only (the M46 t6-wall-entry.sh shape).
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
cd "$W" || exit 2
start=$(date +%s)
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m47-node.bin --script 'break 0x1804b3fc0; continue; where; regs; x 0x27ff368 64; x 0x27ff358 16' > $E/node.entry.txt 2>&1
echo "debug exit=$? secs=$(( $(date +%s) - start ))"
{
  echo "# The recorded trace's own landmarks (Reader::open_checked; #0 is the Snapshot): every kqueue (362),"
  echo "# kevent (363), bsdthread_create (360) and __mac_syscall (381) event, then the last 10. The trace ends"
  echo "# at the second kqueue: the kevent (363) panicked before its event was appended."
  echo "== node (/private/tmp/claude-501/m47-node.bin)"
  $S/td/target/release/td /private/tmp/claude-501/m47-node.bin 10 362 363 360 381
} > $E/node.landmarks.txt 2>&1
echo "td exit=$?"
