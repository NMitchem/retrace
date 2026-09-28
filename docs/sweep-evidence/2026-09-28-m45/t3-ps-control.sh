#!/bin/bash
# M45 Task 3 Step 4, the /bin/ps control: record-dyn /bin/ps then replay it, alternating the swept
# binary (retrace-t3, 09a105f) and the base binary (retrace-base, a78f28f = M44's close crates),
# N rounds. Each run is bounded at 120 s. Before each run it logs the host's process count and the
# kernel's memory-pressure level (1 normal, 2 warn, 4 critical), because M26 found ps's input
# depends on the host's process table and the kept trace's divergence sits in a range the guest
# MADV_FREE_REUSABLEs (landmark 16040). A divergent run's trace is kept; a clean one's is deleted.
# Usage: t3-ps-control.sh <rounds> <tag>
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
O=$S/psctl
mkdir -p $O
N=${1:-4}
TAG=${2:-a}
for i in $(seq 1 $N); do
  for b in t3 base; do
    R=$O/$TAG$i-$b
    procs=$(ps -A -o pid= | wc -l | tr -d ' ')
    lvl=$(sysctl -n kern.memorystatus_vm_pressure_level)
    free=$(sysctl -n kern.memorystatus_level)
    perl -e 'alarm 120; exec @ARGV' $S/retrace-$b record-dyn /bin/ps -o $R.bin > $R.rec.out 2> $R.rec.err; rc=$?
    perl -e 'alarm 120; exec @ARGV' $S/retrace-$b replay $R.bin > $R.rp.out 2> $R.rp.err; rp=$?
    if cmp -s $R.rec.out $R.rp.out; then same=same; else same=DIFF; fi
    div=$(grep -a -m1 'DIVERGENCE' $R.rp.err)
    lines=$(wc -l < $R.rec.out | tr -d ' ')
    echo "RUN $TAG$i $b procs=$procs pressure_level=$lvl memorystatus_level=$free rc=$rc rp=$rp stdout=$same rec_lines=$lines ${div}"
    if [ $rc -eq 0 ] && [ $rp -eq 0 ] && [ $same = same ]; then rm -f $R.bin; fi
  done
done
