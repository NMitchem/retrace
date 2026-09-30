#!/bin/bash
# M46 Task 6 Step 3: how far each watchdog-killed run had got. Reads each kept trace's own landmarks
# (tracedump.rs, Reader::open_checked): the sweep's two timeouts (`[` recording, `kill` replaying)
# and the controls' two (t6 round 1 `dddiagnose` recording, base round 2 `ps` recording).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
T=$S/tracedump/target/release/tracedump
echo "# t6-timeouts.sh: the last landmarks of each watchdog-killed run's kept trace (#0 is the Snapshot)."
echo "# A recording that completes ends Exit/Crash then a final Snapshot; a trace ending at Exit/Crash"
echo "# with no Snapshot after it was killed after the guest's own terminal event."
for f in "$E/sweep/[.bin" $E/sweep/kill.bin $S/ctl/moved-t6-1/dddiagnose.bin $S/ctl/moved-base-2/ps.bin; do
  echo "== $f ($(wc -c < "$f" | tr -d ' ') bytes)"
  $T "$f" | grep -a '^events=\|^gettimeofday'
  $T "$f" | grep -a '^#' | tail -3
done
