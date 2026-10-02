#!/bin/zsh
# M47 t0 Step 2: build the fixtures and run them natively (the brief's Step 2 commands, as a script
# because the session guard refuses `cd` into the ledger in a compound command).
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cd $L/t0
for f in fsops_dyn madv_dyn forkfail_dyn; do clang -arch arm64 -o $f $f.c; echo "$f build=$?"; done
clang -arch arm64 -dynamiclib -install_name @rpath/librpath_dyn.dylib -o librpath_dyn.dylib librpath_dyn.c; echo "lib build=$?"
clang -arch arm64 -o rpath_dyn rpath_dyn.c librpath_dyn.dylib -Wl,-rpath,@executable_path; echo "rpath build=$?"
rm -rf fsops-native && mkdir fsops-native && ./fsops_dyn $PWD/fsops-native > native-fsops.out 2>&1; echo "fsops rc=$?"; cat native-fsops.out
stat -f 'h mtime=%m nlink=%l' fsops-native/d/h
for m in zero reuse bad; do ./madv_dyn $m > native-madv-$m.out 2>&1; echo "madv $m rc=$?"; cat native-madv-$m.out; done
./rpath_dyn > native-rpath.out 2>&1; echo "rpath rc=$?"; cat native-rpath.out
./forkfail_dyn > native-forkfail.out 2>&1; echo "forkfail rc=$?"; cat native-forkfail.out
