#!/bin/bash
# M45 Task 3 Step 3 — the brief's command lines, with `signal` dropped (Ruling T0-a: natively it
# hangs every time and never prints `fired`).
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
export RETRACE_TRACE=1
for c in timer after; do tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0-m2/$c -o /private/tmp/claude-501/m45-$c-2.bin > $E/gcd-$c.rec.out 2> $E/gcd-$c.rec.err; echo "$c record=$?"; tail -3 $E/gcd-$c.rec.err; done
for c in timer after; do echo "== $c"; grep -a -c '^\[trap\]' $E/gcd-$c.rec.err; grep -a -n '^\[trap\] num=374 \|panicked at\|RECORD ERROR\|M45: unmeasured' $E/gcd-$c.rec.err; wc -c < $E/gcd-$c.rec.out; done
