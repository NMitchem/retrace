#!/bin/bash
# M48 Task 9 (spec §3i): one node walk on the final release binary, as t0's walk.sh without the
# probe: record under RETRACE_TRACE and RETRACE_SPRR, then replay twice. stdout goes through a pipe,
# as a test harness's does. Usage: t9-walk.sh <tag> -- <node args…>
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
B=/private/tmp/claude-501/m48-t9-release-retrace
NODE=$(realpath /opt/homebrew/bin/node)
tag=$1; shift 2
T=/private/tmp/claude-501/m48-t9-$tag.bin
export RETRACE_TRACE=1
export RETRACE_SPRR=1
s=$(date +%s)
perl -e 'alarm 900; exec @ARGV' $B record-dyn "$NODE" -o $T -- "$@" < /dev/null 2> $E/walk-$tag.err | cat > $E/walk-$tag.out
rc=${PIPESTATUS[0]}
secs=$(( $(date +%s) - s ))
unset RETRACE_TRACE RETRACE_SPRR
echo "$tag record rc=$rc secs=$secs trace=$(stat -f %z $T 2>/dev/null) traps=$(grep -ac '^\[trap\]' $E/walk-$tag.err)" | tee $E/walk-$tag.status
for i in 1 2; do
  s=$(date +%s)
  perl -e 'alarm 900; exec @ARGV' $B replay $T < /dev/null > $E/walk-$tag.rp$i.out 2> $E/walk-$tag.rp$i.err
  r=$?
  echo "$tag replay$i rc=$r secs=$(( $(date +%s) - s )) same_stdout=$(cmp -s $E/walk-$tag.out $E/walk-$tag.rp$i.out && echo yes || echo no)" | tee -a $E/walk-$tag.status
done
