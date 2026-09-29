#!/bin/bash
# M45 Task 3 Step 3 measurement: each candidate's recorded kevent_qos landmarks (throwaway trace
# reader in the session scratchpad, retrace-trace by path), and the refused second call's change-list
# entry read at its svc (pc 0x1804afa44) by the debugger. Measurement only.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
TD=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/tracedump/target/release/tracedump
echo "== automationmodetool"
$TD /private/tmp/claude-501/m45-amt2.bin | tail -3
for c in timer after; do
  echo "== $c"
  $TD /private/tmp/claude-501/m45-$c-2.bin | tail -3
done
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m45-timer-2.bin --script 'break 0x1804afa44; continue; continue; where; regs; x 0x27ff028 72' > $L/t3-timer-entry.log 2>&1; echo "timer debug exit=$?"
cat $L/t3-timer-entry.log
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m45-after-2.bin --script 'break 0x1804afa44; continue; continue; where; regs; x 0x27ff048 72' > $L/t3-after-entry.log 2>&1; echo "after debug exit=$?"
cat $L/t3-after-entry.log
