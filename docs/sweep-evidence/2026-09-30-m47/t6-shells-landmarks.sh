#!/bin/bash
# M47 Task 6 fix round 1 (review I1): the trace's own landmarks for the shells' 3403 → fork → close → read →
# close stretch, which the walk's traces (deleted) were the only record of. Records /bin/csh and /bin/tcsh
# again, traced, with the signed swept copy (retrace-t6, the binary the sweep ran), dumps the last 11
# events with the throwaway reader (tracedump.rs; TD_HEX=48 prints each write's first 48 bytes), and
# appends this run's last [trap] line (the wait4 that panicked, never appended) and its refusal line.
# The walk's landmark numbers (the #[ignore] reasons') are 317 + the guest's gettimeofday count for the
# 3403; this run's count differs, so each file's header gives the offset. The trace is removed after.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
B=$S/retrace-t6
TD=$S/td/target/release/td
for s in csh tcsh; do
  if [ $s = csh ]; then walk3403=336; walkgtod=19; else walk3403=340; walkgtod=23; fi
  t=$S/fix1-$s.bin; rm -f $t
  export RETRACE_TRACE=1
  $B record-dyn /bin/$s -o $t < /dev/null > $S/fix1-$s.rec.out 2> $S/fix1-$s.rec.err
  rc=$?
  unset RETRACE_TRACE
  export TD_HEX=48
  $TD $t 11 > $S/fix1-$s.td 2>&1
  unset TD_HEX
  fork=$(grep -a -o '^#[0-9]* Syscall num=2 ' $S/fix1-$s.td | head -1 | tr -d '#' | cut -d' ' -f1)
  gtod=$(grep -a -o 'gettimeofday(116) events=[0-9]*' $S/fix1-$s.td | cut -d= -f2)
  {
    echo "# /bin/$s: the trace's own landmarks for the 3403 → fork → close(7) → read(6) → close(6) stretch, and the"
    echo "# wait4 (7) after it (never appended: the recorder panics on it, so it is this run's last [trap] line)."
    echo "# Run: $(date '+%Y-%m-%d %H:%M:%S %Z'), t6-shells-landmarks.sh (review I1, fix round 1), RETRACE_TRACE=1,"
    echo "# binary $B sha256=$(shasum -a 256 $B | cut -d' ' -f1) (the sweep's), stdin /dev/null."
    echo "# record rc=$rc, [trap] lines=$(grep -a -c '^\[trap\] ' $S/fix1-$s.rec.err), gettimeofday(116) events=$gtod."
    echo "# Offset: this run's 3403 is landmark $((fork - 1)); the #[ignore] reason quotes the walk's run (t6-step2.sh,"
    echo "# $s.rec.err, gettimeofday $walkgtod), where it is $walk3403. This run's numbers = the reason's + $((fork - 1 - walk3403)),"
    echo "# which is this run's gettimeofday count less the walk's ($gtod - $walkgtod). The landmark is 317 + that count."
    echo "# td: the last 11 events (#0 Snapshot is always printed); each write's first 48 bytes in hex."
    cat $S/fix1-$s.td
    echo "# this run's refusal line and its last [trap] line (the wait4, landmark $((fork + 4)))"
    grep -a 'refusing fork' $S/fix1-$s.rec.err
    grep -a '^\[trap\] ' $S/fix1-$s.rec.err | tail -1
    grep -a -A1 'panicked at' $S/fix1-$s.rec.err | cut -c1-200
  } > $E/$s.landmarks.txt
  echo "$s rc=$rc fork=#$fork gtod=$gtod offset=$((fork - 1 - walk3403))"
  rm -f $t $S/fix1-$s.rec.out $S/fix1-$s.rec.err $S/fix1-$s.td
done
