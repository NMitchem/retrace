#!/bin/zsh
# Record `git log -1` once per entropy seed with RETRACE_VMLOG=1 and RETRACE_FIXENTROPY=<seed>.
# Usage: loopent.sh <retrace-bin> <outdir> <seed...>
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; shift 2
mkdir -p $O
cd /private/tmp/claude-501/m47-abort/repo
export RETRACE_VMLOG=1
for s in "$@"; do
  export RETRACE_FIXENTROPY=$s
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/seed-$s.bin -- log -1 > $O/seed-$s.out 2> $O/seed-$s.err; rc=$?
  seg=$(grep -a 'mask 0x3fffff' $O/seed-$s.err | head -1 | sed 's/.*-> //')
  res=$(grep -a '4811 hint 0x0 size' $O/seed-$s.err | head -1 | sed 's/.*size \(0x[0-9a-f]*\).*/\1/')
  echo "seed=$s rc=$rc first4811size=$res segment=$seg"
done
