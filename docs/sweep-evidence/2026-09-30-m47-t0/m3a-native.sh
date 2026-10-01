#!/bin/zsh
# M47 t0 M3(a) fallback: the native 3403 answer, three ways (m3anative.c's header says what each is).
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cd $L/t0
clang -arch arm64 -o m3anative m3anative.c; echo "build=$?"
for mode in call msg2 msg; do
  echo "== mode $mode"
  ./m3anative $mode; echo "rc=$?"
done
