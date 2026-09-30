#!/bin/zsh
# gp.sh <tag> <git args...>: rebuild-free probe run of the scratch retrace on the test repo; record + replay + compare.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/f3fccc20-651e-4651-96ff-661888b49a37/scratchpad
P=$S/probe; G=/Applications/Xcode.app/Contents/Developer/usr/bin/git; t=$1; shift
cd $P/repo
perl -e 'alarm 300; exec @ARGV' $S/retrace-scratch record-dyn $G -o $P/g$t.bin -- "$@" > $P/g$t.out 2> $P/g$t.err; rc=$?
echo "== git $* :: record exit=$rc trace=$(stat -f %z $P/g$t.bin 2>/dev/null)"
head -c 300 $P/g$t.out
grep -a -E 'panicked|M33:|RECORD ERROR|refus' $P/g$t.err | grep -v 'message-queue send' | tail -3 | cut -c1-220
if [ -s $P/g$t.bin ] && ! grep -a -q panicked $P/g$t.err; then
  perl -e 'alarm 300; exec @ARGV' $S/retrace-scratch replay $P/g$t.bin > $P/g$t.rp.out 2> $P/g$t.rp.err; r2=$?
  if cmp -s $P/g$t.out $P/g$t.rp.out; then echo "replay exit=$r2 stdout IDENTICAL"; else echo "replay exit=$r2 DIFFERS"; tail -3 $P/g$t.rp.err; fi
fi
