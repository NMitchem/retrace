#!/bin/bash
# t0 walk: record node under the walk binary, then replay twice. Usage: walk.sh <tag> -- <node args…>
# t0 addition to the brief's text: secs= on each status line (Step 9 needs the record time).
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
B=/private/tmp/claude-501/m48-walk-retrace
NODE=$(realpath /opt/homebrew/bin/node)
tag=$1; shift 2
T=/private/tmp/claude-501/m48-$tag.bin
export RETRACE_TRACE=1
export RETRACE_PROBE=1
s=$(date +%s)
perl -e 'alarm 900; exec @ARGV' $B record-dyn "$NODE" -o $T -- "$@" < /dev/null 2> $L/t0/m2-$tag.err | cat > $L/t0/m2-$tag.out
rc=${PIPESTATUS[0]}
secs=$(( $(date +%s) - s ))
unset RETRACE_TRACE RETRACE_PROBE
echo "$tag record rc=$rc secs=$secs trace=$(stat -f %z $T 2>/dev/null) traps=$(grep -ac '^\[trap\]' $L/t0/m2-$tag.err)" | tee $L/t0/m2-$tag.status
for i in 1 2; do
  s=$(date +%s)
  perl -e 'alarm 900; exec @ARGV' $B replay $T < /dev/null > $L/t0/m2-$tag.rp$i.out 2> $L/t0/m2-$tag.rp$i.err
  r=$?
  echo "$tag replay$i rc=$r secs=$(( $(date +%s) - s )) same_stdout=$(cmp -s $L/t0/m2-$tag.out $L/t0/m2-$tag.rp$i.out && echo yes || echo no)" | tee -a $L/t0/m2-$tag.status
done
