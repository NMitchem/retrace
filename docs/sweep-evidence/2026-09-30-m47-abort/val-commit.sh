#!/bin/zsh
# Validation: `git commit` in a fresh one-file repo (cwd = repo, no -C), N times; optionally replay.
# Usage: val-commit.sh <retrace-bin> <outdir> <N> <replay:yes|no>
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; N=$3; RP=$4
ID=(-c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false)
mkdir -p $O
aborts=0; nohead=0; rpfail=0
for i in $(seq 1 $N); do
  d=$O/repo-$i; rm -rf $d; mkdir -p $d
  cd $d
  env -i $G init -q -b main; print one > a.txt; env -i $G add a.txt
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/run.bin -- $ID commit -q -m first > $O/run-$i.out 2> $O/run-$i.err; rc=$?
  [ $rc = 134 ] && aborts=$((aborts+1))
  head=$(env -i $G rev-parse -q --verify HEAD >/dev/null && echo committed || echo none)
  [ $head = none ] && nohead=$((nohead+1))
  fsck=$(env -i $G fsck --no-dangling 2>&1 | wc -l | tr -d ' ')
  line="run=$i rc=$rc head=$head fsck_lines=$fsck $(grep -a -m1 -E 'guest terminated|panicked at|RECORD ERROR' $O/run-$i.err | cut -c1-100)"
  if [ $RP = yes ]; then
    perl -e 'alarm 300; exec @ARGV' $R replay $O/run.bin > $O/rp-$i.out 2> $O/rp-$i.err; prc=$?
    [ $prc = $rc ] || rpfail=$((rpfail+1))
    line="$line replay_rc=$prc"
  fi
  echo "$line"
  cd /private/tmp/claude-501/m47-abort
  rm -rf $d
done
rm -f $O/run.bin
echo "SUMMARY bin=$R N=$N aborts=$aborts no_commit=$nohead replay_failures=$rpfail"
