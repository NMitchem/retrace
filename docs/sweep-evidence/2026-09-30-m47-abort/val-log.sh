#!/bin/zsh
# Unpinned validation: record `git log -1` (cwd = repo) N times; optionally replay each.
# Usage: val-log.sh <retrace-bin> <outdir> <N> <replay:yes|no>
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; N=$3; RP=$4
mkdir -p $O
cd /private/tmp/claude-501/m47-abort/repo
aborts=0; rpfail=0; mism=0
for i in $(seq 1 $N); do
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/run.bin -- log -1 > $O/run-$i.out 2> $O/run-$i.err; rc=$?
  [ $rc = 134 ] && aborts=$((aborts+1))
  cmp -s /private/tmp/claude-501/m47-abort/native-log.out $O/run-$i.out && same=yes || { same=no; mism=$((mism+1)); }
  line="run=$i rc=$rc stdout==native:$same"
  if [ $RP = yes ]; then
    perl -e 'alarm 300; exec @ARGV' $R replay $O/run.bin > $O/rp-$i.out 2> $O/rp-$i.err; prc=$?
    cmp -s $O/run-$i.out $O/rp-$i.out && rsame=yes || rsame=no
    [ $prc = $rc ] && [ $rsame = yes ] || rpfail=$((rpfail+1))
    line="$line replay_rc=$prc replay_stdout==record:$rsame"
  fi
  echo "$line"
done
rm -f $O/run.bin
echo "SUMMARY bin=$R N=$N aborts=$aborts stdout_mismatch=$mism replay_failures=$rpfail"
