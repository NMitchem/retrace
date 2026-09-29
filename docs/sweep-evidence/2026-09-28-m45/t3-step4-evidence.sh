#!/bin/bash
# M45 Task 3 Step 4: gather the moved-row measurements into the committed evidence directory —
# the /bin/ps page map (from the sweep's own kept trace), the control logs, the scripts and the
# throwaway readers' sources — then remove every kept .bin.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
L=$W/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
E=$W/docs/sweep-evidence/2026-09-28-m45
bash $L/t3-ps-ipawho.sh $E/sweep/ps.bin 0x701414078 0x701400000 0x701450000 > $E/ps-pagemap.txt 2>&1; echo "pagemap exit=$?"
cat $L/t3-ps-control-a.log $L/t3-ps-control-b.log > $E/ps-control.txt
cp $L/t3-ps-replay-control.log $E/ps-replay-control.txt
cat $L/t3-dddiagnose-control-a.log $L/t3-dddiagnose-control-b.log > $E/dddiagnose-control.txt
cp $L/t3-csh-samples.log $E/csh-samples.txt
bash $L/t3-dddiagnose-fork.sh a1-base a2-base > $E/dddiagnose-fork.txt 2>&1
bash $L/t3-rowdiff.sh > $E/rowdiff.txt 2>&1
for f in t3-sweep.sh t3-step1.sh t3-step3.sh t3-wall-entry.sh t3-candidates-measure.sh t3-evidence-copy.sh \
         t3-base-build.sh t3-ps-control.sh t3-ps-replay-control.sh t3-ps-ipawho.sh \
         t3-dddiagnose-control.sh t3-dddiagnose-fork.sh t3-csh-samples.sh t3-rowdiff.sh t3-step4-evidence.sh; do
  cp $L/$f $E/$f
done
cp $S/ipawho/src/main.rs $E/ipawho.rs
cp $S/tracedump/src/main.rs $E/tracedump.rs
rm -f $E/sweep/*.bin
# The controls' and the walk's own traces lived in the scratchpad and /private/tmp/claude-501.
rm -f $S/psctl/*.bin $S/ddctl/*.bin $S/cshctl/*.bin
rm -f /private/tmp/claude-501/m45-amt2.bin /private/tmp/claude-501/m45-timer-2.bin /private/tmp/claude-501/m45-after-2.bin
ls -la $E $E/sweep
