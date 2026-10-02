#!/bin/zsh
# Like loopent.sh but optionally also pins gettimeofday (RETRACE_FIXTIME) — arg 3 is "time" or "notime".
# Usage: loopent2.sh <retrace-bin> <outdir> <time|notime> <seed...>
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; MODE=$3; shift 3
mkdir -p $O
cd /private/tmp/claude-501/m47-abort/repo
export RETRACE_VMLOG=1
if [ $MODE = time ]; then export RETRACE_FIXTIME=1; else unset RETRACE_FIXTIME; fi
i=0
for s in "$@"; do
  i=$((i+1))
  export RETRACE_FIXENTROPY=$s
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/run.bin -- log -1 > $O/r$i.out 2> $O/r$i.err; rc=$?
  seg=$(grep -a 'mask 0x3fffff' $O/r$i.err | head -1 | sed 's/.*-> //')
  res=$(grep -a '4811 hint 0x0 size' $O/r$i.err | head -1 | sed 's/.*size \(0x[0-9a-f]*\).*/\1/')
  echo "mode=$MODE seed=$s rc=$rc first4811size=$res segment=$seg"
done
rm -f $O/run.bin
