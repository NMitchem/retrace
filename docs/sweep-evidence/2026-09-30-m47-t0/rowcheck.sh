#!/bin/zsh
# M47 t0 Step 10's rowcheck (the brief's command), run twice: on M4's nums.txt as the brief says, and
# on the union of M4's, the tag retry's and M3(b)'s m3b-build runs (default-config commit's fork path).
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
L=$PWD/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
export M47_NUMS=$L/t0/m4/nums.txt
cargo test -p retrace-arch --test zz_m47_rowcheck -- --nocapture > $L/t0/m4-rowcheck.log 2>&1; echo "m4 exit=$?"
grep -a NOROW $L/t0/m4-rowcheck.log
export M47_NUMS=$L/t0/rowcheck-union-nums.txt
cargo test -p retrace-arch --test zz_m47_rowcheck -- --nocapture > $L/t0/union-rowcheck.log 2>&1; echo "union exit=$?"
grep -a NOROW $L/t0/union-rowcheck.log
