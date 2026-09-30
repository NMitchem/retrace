#!/bin/bash
# Block until the sweep log has at least $1 result lines or its tally, or $2 seconds pass.
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46/sweep.log
want=$1; limit=${2:-540}; t=0
while [ $t -lt $limit ]; do
  n=$(grep -a -c '^\(PASS\|FAIL\|SKIP\) ' $L)
  if [ "$n" -ge "$want" ] || grep -a -q '^TALLY' $L; then break; fi
  sleep 10; t=$((t+10))
done
echo "rows=$(grep -a -c '^\(PASS\|FAIL\|SKIP\) ' $L) waited=${t}s"
grep -a '^\(FAIL\|SKIP\) \|identical fault\|^TALLY\|^SWEEP_EXIT\|^load-end' $L
uptime
