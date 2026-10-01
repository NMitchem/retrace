#!/bin/zsh
# M47 t0 M2(b) fallback (deviation; lldb cannot launch a debuggee on this host): the native
# Sandbox/AMFI answers, by (1) the __mac_syscall interposer on native ad-hoc processes and (2)
# m2bnative.c, which issues dyld's and libsystem_sandbox's call-2 structs from their measured bytes.
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
GOUT=$(ls -td /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)
cd $L/t0
clang -arch arm64 -dynamiclib -o m2binterpose.dylib m2binterpose.c; echo "interposer build=$?"
clang -arch arm64 -o m2bnative m2bnative.c; echo "m2bnative build=$?"
echo "== (1) interposer on hello_dyn (a repo dyld guest), natively"
export DYLD_INSERT_LIBRARIES=$L/t0/m2binterpose.dylib
$GOUT/hello_dyn; echo "hello_dyn rc=$?"
echo "== (1) interposer on rpath_dyn, natively"
./rpath_dyn; echo "rpath_dyn rc=$?"
echo "== (1) interposer on call4native, natively"
./call4native; echo "call4native rc=$?"
unset DYLD_INSERT_LIBRARIES
echo "== (2) m2bnative: the measured structs, issued natively"
./m2bnative; echo "m2bnative rc=$?"
