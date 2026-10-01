#!/bin/zsh
# M47 t0 M1(b): git commit with MADV_FREE_REUSABLE forwarded (A) and no-op'd (B), N fresh repos each.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=/private/tmp/claude-501/m47-census-retrace
O=$1; N=${2:-5}
ID=(-c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false)
mkdir -p $O
for mode in A B; do
  for i in $(seq 1 $N); do
    d=$O/repo-$mode-$i; rm -rf $d; mkdir -p $d
    env -i $G -C $d init -q -b main; print one > $d/a.txt; env -i $G -C $d add a.txt
    if [ $mode = B ]; then export PROBE_NOREUSABLE=1; else unset PROBE_NOREUSABLE; fi
    perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o /private/tmp/claude-501/m47-m1b.bin -- -C $d $ID commit -q -m first > $O/$mode-$i.out 2> $O/$mode-$i.err; rc=$?
    abort=$(grep -a -c 'pointer being freed was not allocated' $O/$mode-$i.err)
    head=$(env -i $G -C $d rev-parse -q --verify HEAD >/dev/null && echo committed || echo none)
    echo "M1b mode=$mode run=$i rc=$rc abort_lines=$abort head=$head"
  done
done
