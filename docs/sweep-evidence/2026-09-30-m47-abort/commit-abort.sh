#!/bin/zsh
# Record `git commit` (fresh repo, cwd = repo) on the census binary until one aborts (max 12); keep
# that trace as $A/commit-abort.bin.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=/private/tmp/claude-501/m47-census-retrace
A=/private/tmp/claude-501/m47-abort
ID=(-c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false)
for i in $(seq 1 12); do
  d=$A/ca-repo; rm -rf $d; mkdir -p $d; cd $d
  env -i $G init -q -b main; print one > a.txt; env -i $G add a.txt
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $A/commit-abort.bin -- $ID commit -q -m first > $A/ca.out 2> $A/ca.err; rc=$?
  echo "try=$i rc=$rc"
  [ $rc = 134 ] && break
done
cd $A
