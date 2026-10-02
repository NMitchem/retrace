#!/bin/zsh
# Record `git log -1` (cwd = repo, no -C) N times with RETRACE_VMLOG=1; keep no traces.
# Usage: loopvm.sh <retrace-bin> <outdir> <N>
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; N=$3
mkdir -p $O
cd /private/tmp/claude-501/m47-abort/repo
export RETRACE_VMLOG=1
for i in $(seq 1 $N); do
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/run.bin -- log -1 > $O/run-$i.out 2> $O/run-$i.err; rc=$?
  echo "run=$i rc=$rc misaligned=$(grep -a -c MISALIGNED $O/run-$i.err)"
done
rm -f $O/run.bin
