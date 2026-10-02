#!/bin/zsh
# M47 t0, after H7 tripped: characterize the (Sandbox, 4) pair the census found (census.tsv, the
# xcrun trio desdp/dyld_info/flex). Records desdp twice on the census binary (stability, H8's
# two-run rule applied to the new pair), then dumps each call-4 site with `retrace debug`, the probe's
# method for sandbox-call2.txt (break at the trap pc lands after the svc, so x0 holds the errno).
R=/private/tmp/claude-501/m47-census-retrace
T=/private/tmp/claude-501/m47-h7
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
mkdir -p $T
export RETRACE_TRACE=1
for i in 1 2; do
  $R record-dyn /usr/bin/desdp -o $T/desdp-$i.bin > $T/desdp-$i.out 2> $T/desdp-$i.err; echo "desdp run $i rc=$?"
  grep -a -E '^\[trap\] num=381 |^\[m47\] |^\[probe\] AMFI' $T/desdp-$i.err | cut -c1-200
done
unset RETRACE_TRACE
cp $T/desdp-1.err $L/t0/h7-desdp-1.rec.err
cp $T/desdp-2.err $L/t0/h7-desdp-2.rec.err
echo "== retrace debug, desdp-1: each libsystem_kernel __mac_syscall stop (call-4, AMFI 2nd caller, call-4)"
$R debug $T/desdp-1.bin --script 'break 0x1804af730; continue; where; regs; x 0x27fe778 64; continue; where; continue; where; regs; x 0x27fd4a8 64' 2>&1
echo "debug rc=$?"
