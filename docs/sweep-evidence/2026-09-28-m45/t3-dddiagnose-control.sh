#!/bin/bash
# M45 Task 3 Step 4, the /usr/bin/dddiagnose control: record-dyn under RETRACE_TRACE=1, then
# replay, on the swept binary (retrace-t3, 09a105f) and the base binary (retrace-base, a78f28f =
# M44's close crates), N rounds alternating. Each run is bounded at 120 s.
# Usage: t3-dddiagnose-control.sh <rounds> <tag>
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
O=$S/ddctl
mkdir -p $O
N=${1:-1}
TAG=${2:-a}
export RETRACE_TRACE=1
for i in $(seq 1 $N); do
  for b in t3 base; do
    R=$O/$TAG$i-$b
    procs=$(ps -A -o pid= | wc -l | tr -d ' ')
    maxpid=$(ps -A -o pid= | sort -n | tail -1 | tr -d ' ')
    perl -e 'alarm 120; exec @ARGV' $S/retrace-$b record-dyn /usr/bin/dddiagnose -o $R.bin > $R.rec.out 2> $R.rec.err; rc=$?
    perl -e 'alarm 120; exec @ARGV' $S/retrace-$b replay $R.bin > $R.rp.out 2> $R.rp.err; rp=$?
    traps=$(grep -a -c '^\[trap\]' $R.rec.err)
    last=$(grep -a -E 'RECORD ERROR|guest crashed|panicked at' $R.rec.err | head -1)
    rplast=$(grep -a -E 'DIVERGENCE|guest crashed' $R.rp.err | head -1)
    echo "RUN $TAG$i $b procs=$procs maxpid=$maxpid rc=$rc rp=$rp traps=$traps rec: ${last} | rp: ${rplast}"
  done
done
