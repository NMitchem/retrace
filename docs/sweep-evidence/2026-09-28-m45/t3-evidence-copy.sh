#!/bin/bash
# M45 Task 3: put the walk's measurements into the committed evidence directory.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
TD=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/tracedump/target/release/tracedump
cp $L/t3-wall-entry.log $E/automationmodetool.entry.txt
cp $L/t3-timer-entry.log $E/gcd-timer.entry.txt
cp $L/t3-after-entry.log $E/gcd-after.entry.txt
{
  echo "# The recorded traces' own landmarks (Reader::open_checked; #0 is the Snapshot). Each trace ends"
  echo "# at the emulated init: the refused second kevent_qos panicked before its event was appended."
  echo "== automationmodetool (/private/tmp/claude-501/m45-amt2.bin)"
  $TD /private/tmp/claude-501/m45-amt2.bin
  for c in timer after; do
    echo "== gcd-$c (/private/tmp/claude-501/m45-$c-2.bin)"
    $TD /private/tmp/claude-501/m45-$c-2.bin
  done
} > $E/landmarks.txt
cat $E/landmarks.txt
ls -la $E
