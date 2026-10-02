#!/bin/zsh
# val-commit.sh <retrace-bin> <outdir> <N> — t3b validation, adapted from the diagnosis's
# /private/tmp/claude-501/m47-abort/val-commit.sh: in a FRESH repo each run (Xcode's git, `env -i`,
# one staged file), record `git -C <repo> -c user.name=r -c user.email=r@x.invalid
# -c maintenance.auto=false commit -q -m m`, then replay it. Per run: record rc, HEAD exists,
# `git fsck --strict` exit and output line count, replay rc == record rc, replay stdout == record
# stdout. A trace is kept (keep-<i>.bin) only when the run is not clean.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=$1; O=$2; N=$3
mkdir -p $O
aborts=0; nohead=0; fsckbad=0; rpfail=0
for i in $(seq 1 $N); do
  d=$O/repo-$i; rm -rf $d; mkdir -p $d
  env -i $G -C $d init -q -b main
  print one > $d/a.txt
  env -i $G -C $d add a.txt
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/run.bin -- -C $d -c user.name=r -c user.email=r@x.invalid -c maintenance.auto=false commit -q -m m > $O/run-$i.out 2> $O/run-$i.err; rc=$?
  [ $rc = 134 ] && aborts=$((aborts+1))
  head=$(env -i $G -C $d rev-parse -q --verify HEAD >/dev/null && echo committed || echo none)
  [ $head = none ] && nohead=$((nohead+1))
  env -i $G -C $d fsck --strict > $O/fsck-$i.out 2>&1; frc=$?
  flines=$(wc -l < $O/fsck-$i.out | tr -d ' ')
  [ $frc = 0 ] && [ $flines = 0 ] || fsckbad=$((fsckbad+1))
  perl -e 'alarm 300; exec @ARGV' $R replay $O/run.bin > $O/rp-$i.out 2> $O/rp-$i.err; prc=$?
  cmp -s $O/run-$i.out $O/rp-$i.out && rsame=yes || rsame=no
  ok=yes
  [ $prc = $rc ] && [ $rsame = yes ] || { rpfail=$((rpfail+1)); ok=no; }
  [ $rc = 0 ] && [ $head = committed ] && [ $frc = 0 ] && [ $flines = 0 ] || ok=no
  [ $ok = yes ] && rm -f $O/run.bin || mv $O/run.bin $O/keep-$i.bin
  echo "run=$i rc=$rc head=$head fsck_rc=$frc fsck_lines=$flines replay_rc=$prc replay_stdout==record:$rsame $(grep -a -m1 -E 'pointer being freed|guest terminated|panicked at|DIVERGENCE' $O/run-$i.err $O/rp-$i.err | cut -c1-140)"
  rm -rf $d
done
echo "SUMMARY bin=$R N=$N aborts=$aborts no_commit=$nohead fsck_unclean=$fsckbad replay_failures=$rpfail"
