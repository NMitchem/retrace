#!/bin/zsh
# M47 t0 Step 7 fallback (deviation): lldb is refused attach to Xcode's Apple-signed git (m2b.log:
# "Not allowed to attach to process"), so the same m2b.lldb runs on an ad-hoc-signed dyld guest the
# repo owns (rpath_dyn, built by step2.sh), which issues dyld's own __mac_syscall calls.
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
lldb -b -s $L/t0/m2b.lldb -- $L/t0/rpath_dyn > $L/t0/m2b-rpath.log 2>&1; echo "exit=$?"
grep -a -E 'x0 = |x1 = |"|stop reason|Breakpoint' $L/t0/m2b-rpath.log | head -80
