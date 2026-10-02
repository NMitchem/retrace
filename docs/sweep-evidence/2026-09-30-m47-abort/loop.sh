#!/bin/zsh
# Record `git log -1` (cwd = repo, no -C) N times with RETRACE_TRACE=1, keeping every trace + log.
# Usage: loop.sh <retrace-bin> <outdir> <N> [keep-clean-traces]
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; N=$3; KEEP=${4:-3}
mkdir -p $O
cd /private/tmp/claude-501/m47-abort/repo
export RETRACE_TRACE=1
kept=0
for i in $(seq 1 $N); do
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/run-$i.bin -- log -1 > $O/run-$i.out 2> $O/run-$i.err; rc=$?
  echo "run=$i rc=$rc $(date +%T)"
  if [ $rc = 0 ]; then
    kept=$((kept+1))
    if [ $kept -gt $KEEP ]; then rm -f $O/run-$i.bin; fi
  fi
done
