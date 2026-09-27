#!/bin/bash
# M44 Task 4 Step 1 (controller-run): re-measure the seven targets on the COMMITTED rows, in a
# `git archive` copy of the given commit so no running implementer's edits reach the build.
# usage: t4-sweep.sh <commit-sha> <worktree>
# The xcrun trio's 464/128 reach depends on xcrun's host cache (Ruling T0-e), so each trio member
# is swept alone after removing /var/tmp/xcrun_db (a cache xcrun rebuilds itself): every trio row
# is measured from a cold cache, which is the path that reaches the M44 rows.
set -u
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/1996e432-75ea-4d65-a52b-ed70d24a05b8/scratchpad
T=$S/t4tree
E=$S/t4-evidence
rm -rf "$T" "$E"
mkdir -p "$T" "$E"
( cd "$2" && git archive "$1" ) > $S/t4tree.tar || { echo "archive failed"; exit 2; }
tar -xf $S/t4tree.tar -C "$T" || exit 2
cd "$T" || exit 2
cargo build -p retrace > $S/t4-build.log 2>&1; echo "build exit=$?"
export RETRACE_SWEEP_KEEP=$E
export RETRACE_SWEEP_KEEP_ALL=1
printf '/bin/ed\n/bin/ls\n/usr/bin/dddiagnose\n/usr/bin/automationmodetool\n' > $S/t4-list-a.txt
export RETRACE_SWEEP_LIST=$S/t4-list-a.txt
tools/apple-sweep.sh > $S/t4-sweep-a.log 2>&1; echo "sweep a exit=$?"
for b in desdp dyld_info flex; do
  rm -f /var/tmp/xcrun_db
  printf '/usr/bin/%s\n' "$b" > $S/t4-list-$b.txt
  export RETRACE_SWEEP_LIST=$S/t4-list-$b.txt
  tools/apple-sweep.sh > $S/t4-sweep-$b.log 2>&1; echo "sweep $b exit=$?"
done
ls -la /var/tmp/ | grep -a xcrun_db
grep -a -h -E '^(PASS|FAIL|TALLY)' $S/t4-sweep-*.log
