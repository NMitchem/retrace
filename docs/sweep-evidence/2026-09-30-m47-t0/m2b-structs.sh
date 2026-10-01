#!/bin/zsh
# M47 t0 M2(b) fallback, part 1: the exact Sandbox call-2 argument structs (and what +0 points at)
# for one repo dyld guest, read from a census-binary recording with `retrace debug` (the method of
# the probe's sandbox-call2.txt). The native C reproduction (m2bnative.c) is built from these bytes.
R=/private/tmp/claude-501/m47-census-retrace
T=/private/tmp/claude-501/m47-m2b
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
GOUT=$(ls -td /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)
mkdir -p $T
export RETRACE_TRACE=1
$R record-dyn $GOUT/hello_dyn -o $T/hello.bin > $T/hello.out 2> $T/hello.err; echo "record rc=$?"
unset RETRACE_TRACE
grep -a -E '^\[trap\] num=381 |^\[m47\] num=381' $T/hello.err
echo "== pass 1: each __mac_syscall stop, the struct at x2"
$R debug $T/hello.bin --script "$1" 2>&1
echo "debug rc=$?"
