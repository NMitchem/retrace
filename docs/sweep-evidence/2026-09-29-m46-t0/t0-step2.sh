#!/bin/bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
for f in after_dyn timer_dyn; do clang -arch arm64 -o $L/t0/$f $L/t0/$f.c; echo "$f build=$?"; done
for m in "" two wall clock; do $L/t0/after_dyn $m > $L/t0/native-after-${m:-default}.out 2>&1; echo "after '$m' rc=$?"; cat $L/t0/native-after-${m:-default}.out; done
$L/t0/timer_dyn > $L/t0/native-timer.out 2>&1; echo "timer rc=$?"; cat $L/t0/native-timer.out
