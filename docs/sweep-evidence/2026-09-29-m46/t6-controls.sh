#!/bin/bash
# M46 Task 6 Step 3: every moved row, re-swept on the base binary (f907c33, the M45 merge) and the
# swept binary (d369fcc), alternating, two rounds each, through tools/apple-sweep.sh itself with a
# list of just the moved rows (RETRACE_SWEEP_LIST), so each control row is judged by the sweep's
# own labels and watchdog. Usage: t6-controls.sh <list> <tag> [timeout-seconds, default 30].
# No cargo runs while this runs.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
LIST=$1; TAG=$2; export RETRACE_SWEEP_TIMEOUT=${3:-30}
cd "$W" || exit 2
export RETRACE_SWEEP_LIST=$LIST
echo "# t6-controls.sh $TAG: list=$(tr '\n' ' ' < $LIST) timeout=${RETRACE_SWEEP_TIMEOUT}s"
echo "# base sha256=$(shasum -a 256 $S/retrace-base | cut -d' ' -f1) (f907c33)"
echo "# t6   sha256=$(shasum -a 256 $S/retrace-t6 | cut -d' ' -f1) (d369fcc, the swept binary)"
for r in 1 2; do
  for b in base t6; do
    echo "## round $r binary $b start $(date '+%H:%M:%S') $(uptime | sed 's/.*load/load/')"
    export RETRACE_SWEEP_KEEP=$S/ctl/$TAG-$b-$r
    tools/apple-sweep.sh $S/retrace-$b 2>&1 | grep -a '^ROW\|^TALLY' | sed "s#^#$r $b #"
  done
done
echo "## end $(date '+%H:%M:%S') $(uptime | sed 's/.*load/load/')"
