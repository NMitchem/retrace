#!/bin/bash
# M46 Task 6 Step 3: traced controls for the moved rows, on the base (f907c33) and swept (d369fcc)
# binaries, alternating, two rounds each. No cargo runs while this runs.
#  A. kill: the sweep's own kill.bin (recorded to Exit + final Snapshot, replay killed by the 30 s
#     watchdog) replayed on both binaries, timed (bash SECONDS), bounded at 300 s.
#  B. csh/tcsh: M45's t3-csh-samples.sh shape — the [trap] lines before the 3403 wall under
#     RETRACE_TRACE=1, and how many of them are gettimeofday (116).
#  C. dddiagnose: traced runs, bounded at 180 s: traps, gettimeofday count, the refused RCV-only
#     receive's trap number, and the outcome line (the M45 dddiagnose-traps.txt shape).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
O=$S/samples
mkdir -p $O
echo "# t6-samples.sh, $(date '+%Y-%m-%d %H:%M:%S'), same host, alternating binaries."
echo "# base = f907c33 (the M45 merge); t6 = d369fcc (the swept binary)."
echo "## A. the sweep's kill.bin replayed"
for i in 1 2; do
  for b in base t6; do
    SECONDS=0
    perl -e 'alarm 300; exec @ARGV' $S/retrace-$b replay $E/sweep/kill.bin < /dev/null > $O/kill-$i-$b.rp.out 2> $O/kill-$i-$b.rp.err; rp=$?
    echo "A $i $b rp=$rp secs=$SECONDS stdout=$(wc -c < $O/kill-$i-$b.rp.out | tr -d ' ') divergence-lines=$(grep -a -c 'DIVERGENCE' $O/kill-$i-$b.rp.err) $(uptime | sed 's/.*load/load/')"
  done
done
export RETRACE_TRACE=1
echo "## B. csh/tcsh: traps = [trap] lines before the 3403 wall; gtod = gettimeofday (116) traps"
for i in 1 2; do
  for sh in csh tcsh; do
    for b in base t6; do
      perl -e 'alarm 120; exec @ARGV' $S/retrace-$b record-dyn /bin/$sh -o $O/$sh-$i-$b.bin < /dev/null > $O/$sh-$i-$b.rec.out 2> $O/$sh-$i-$b.rec.err; rc=$?
      traps=$(grep -a -c '^\[trap\]' $O/$sh-$i-$b.rec.err)
      gtod=$(grep -a -c '^\[trap\] num=116 ' $O/$sh-$i-$b.rec.err)
      wall=$(grep -a -c 'msgh_id 3403' $O/$sh-$i-$b.rec.err)
      echo "B $i $sh $b rc=$rc traps=$traps gtod=$gtod traps-gtod=$((traps - gtod)) 3403-lines=$wall"
      rm -f $O/$sh-$i-$b.bin
    done
  done
done
echo "## C. dddiagnose: traced record, bounded at 180 s"
for i in 1 2; do
  for b in base t6; do
    SECONDS=0
    perl -e 'alarm 180; exec @ARGV' $S/retrace-$b record-dyn /usr/bin/dddiagnose -o $O/ddd-$i-$b.bin < /dev/null > $O/ddd-$i-$b.rec.out 2> $O/ddd-$i-$b.rec.err; rc=$?
    traps=$(grep -a -c '^\[trap\]' $O/ddd-$i-$b.rec.err)
    gtod=$(grep -a -c '^\[trap\] num=116 ' $O/ddd-$i-$b.rec.err)
    rcv=$(awk '/^\[trap\]/{n++} /refusing mach_msg2 message-queue receive/{print n; exit}' $O/ddd-$i-$b.rec.err)
    out=$(grep -a -m1 'RECORD ERROR:\|guest crashed\|panicked at crates/' $O/ddd-$i-$b.rec.err | cut -c1-160)
    echo "C $i $b rc=$rc secs=$SECONDS traps=$traps gtod=$gtod receive-refused-at-trap=$rcv outcome: $out"
    rm -f $O/ddd-$i-$b.bin
  done
done
echo "## end $(date '+%H:%M:%S') $(uptime | sed 's/.*load/load/')"
