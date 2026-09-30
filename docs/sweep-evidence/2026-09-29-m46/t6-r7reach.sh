#!/bin/bash
# M46 Task 6 Step 3: which kept traces reach R7. R7 rewrites gettimeofday's (116) mach-time
# out-parameter only when the call passes one (x2 != 0) and succeeds; a call with x2 == 0 is
# recorded exactly as before M46. Counts both, per kept trace (tracedump.rs), for the sweep's kept
# rows, the controls' kept rows (base and t6), and Step 1's automationmodetool trace.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
T=$S/tracedump/target/release/tracedump
echo "# t6-r7reach.sh: per kept trace, gettimeofday (116) events, and those with x2 != 0 (R7's only case)."
for f in /private/tmp/claude-501/m46-amt.bin "$E"/sweep/*.bin "$S"/ctl/*/*.bin; do
  echo "$(echo "$f" | sed "s#^$E/##; s#^$S/##") $($T "$f" | grep -a '^gettimeofday')"
done
