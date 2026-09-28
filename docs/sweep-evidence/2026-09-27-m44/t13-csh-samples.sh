#!/bin/bash
# M44 Task 13: sample csh's trap count before the 3403 wall, alternating base/close binaries, and
# count its gettimeofday (116) traps — the field that moved run to run on ONE binary.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/1996e432-75ea-4d65-a52b-ed70d24a05b8/scratchpad
cd $S || exit 2
for i in 1 2 3 4; do
  for v in base t13; do
    RETRACE_TRACE=1 $S/retrace-$v record-dyn /bin/csh -o $S/t13-s.bin < /dev/null > /dev/null 2> $S/t13-s.err
    echo "$i $v traps=$(grep -a -c '^\[trap\]' $S/t13-s.err) gtod=$(grep -a -c 'num=116 ' $S/t13-s.err)"
    rm -f $S/t13-s.bin
  done
done
