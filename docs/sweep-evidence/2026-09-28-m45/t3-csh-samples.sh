#!/bin/bash
# M45 Task 3 Step 4: M44's csh/tcsh landmark measurement (t13-csh-samples.sh), repeated for both
# shells on M45's base (a78f28f) and swept (09a105f) binaries, alternating: the [trap] lines before
# the 3403 wall under RETRACE_TRACE=1, and how many of them are gettimeofday (116).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
O=$S/cshctl
mkdir -p $O
export RETRACE_TRACE=1
echo "# t3-csh-samples.sh, $(date '+%Y-%m-%d'), same host, alternating binaries."
echo "# base = a78f28f (M45's base, M44's close crates); t3 = 09a105f (the swept binary)."
echo "# traps = [trap] lines before the 3403 wall; gtod = gettimeofday (116) traps."
for i in 1 2; do
  for sh in csh tcsh; do
    for b in base t3; do
      perl -e 'alarm 120; exec @ARGV' $S/retrace-$b record-dyn /bin/$sh -o $O/$sh-$i-$b.bin < /dev/null > $O/$sh-$i-$b.rec.out 2> $O/$sh-$i-$b.rec.err; rc=$?
      traps=$(grep -a -c '^\[trap\]' $O/$sh-$i-$b.rec.err)
      gtod=$(grep -a -c '^\[trap\] num=116 ' $O/$sh-$i-$b.rec.err)
      wall=$(grep -a -c 'msgh_id 3403' $O/$sh-$i-$b.rec.err)
      echo "$i $sh $b rc=$rc traps=$traps gtod=$gtod traps-gtod=$((traps - gtod)) 3403-lines=$wall"
      rm -f $O/$sh-$i-$b.bin
    done
  done
done
