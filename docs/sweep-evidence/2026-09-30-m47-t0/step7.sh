#!/bin/zsh
# M47 t0 Step 7 (M2(b)): the brief's commands, as a script (session guard). m2b.lldb was generated
# with the brief's exact command list (8 repetitions).
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
rm -rf $L/t0/m2b-repo
mkdir -p $L/t0/m2b-repo && cd $L/t0/m2b-repo && env -i /Applications/Xcode.app/Contents/Developer/usr/bin/git init -q -b main && cd -
lldb -b -s $L/t0/m2b.lldb -- /Applications/Xcode.app/Contents/Developer/usr/bin/git -C $L/t0/m2b-repo status --porcelain > $L/t0/m2b.log 2>&1; echo "exit=$?"
grep -a -E 'x0 = |x1 = |"' $L/t0/m2b.log | head -80
