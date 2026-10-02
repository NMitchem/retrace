#!/bin/zsh
# val-log.sh <retrace-bin> <outdir> <N> — t3b validation, adapted from the diagnosis's
# /private/tmp/claude-501/m47-abort/val-log.sh: `git log -1` recorded then replayed N times, each in a
# FRESH one-commit repo made by Xcode's git under `env -i` (mkrepo.sh's recipe), with `-C <repo>`
# rather than cwd. Per run: record rc, record stdout == the native `git log -1` of that repo, replay rc,
# replay stdout == record stdout. A trace is kept (keep-<i>.bin) only when the run is not clean.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; N=$3
mkdir -p $O
aborts=0; mism=0; rpfail=0
for i in $(seq 1 $N); do
  d=$O/repo-$i; rm -rf $d; mkdir -p $d
  env -i $G -C $d init -q -b main
  print one > $d/a.txt
  env -i $G -C $d add a.txt
  env -i $G -C $d -c user.name=retrace -c user.email=retrace@example.invalid commit -q -m first
  env -i $G -C $d log -1 > $O/native-$i.out 2>&1
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/run.bin -- -C $d log -1 > $O/run-$i.out 2> $O/run-$i.err; rc=$?
  [ $rc = 134 ] && aborts=$((aborts+1))
  cmp -s $O/native-$i.out $O/run-$i.out && same=yes || { same=no; mism=$((mism+1)); }
  perl -e 'alarm 300; exec @ARGV' $R replay $O/run.bin > $O/rp-$i.out 2> $O/rp-$i.err; prc=$?
  cmp -s $O/run-$i.out $O/rp-$i.out && rsame=yes || rsame=no
  ok=yes
  [ $prc = $rc ] && [ $rsame = yes ] || { rpfail=$((rpfail+1)); ok=no; }
  [ $rc = 0 ] && [ $same = yes ] || ok=no
  [ $ok = yes ] && rm -f $O/run.bin || mv $O/run.bin $O/keep-$i.bin
  echo "run=$i rc=$rc stdout==native:$same replay_rc=$prc replay_stdout==record:$rsame $(grep -a -m1 -E 'pointer being freed|guest terminated|panicked at|DIVERGENCE' $O/run-$i.err $O/rp-$i.err | cut -c1-140)"
  rm -rf $d
done
echo "SUMMARY bin=$R N=$N aborts=$aborts stdout_mismatch=$mism replay_failures=$rpfail"
