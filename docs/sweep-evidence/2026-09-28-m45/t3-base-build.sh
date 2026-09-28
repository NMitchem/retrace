#!/bin/bash
# M45 Task 3 Step 4: build M45's base (a78f28f, whose crates equal M44's close 60f0452) from a
# `git archive` extracted into the scratchpad, with its own target dir, then sign a copy.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
L=$W/.superpowers/sdd/2026-09-28-retrace-m45-kqinit
cd $W || exit 2
echo "crates diff 60f0452..a78f28f:"; git diff --stat 60f0452 a78f28f -- crates; echo "(end)"
rm -rf $S/base-a78f28f && mkdir -p $S/base-a78f28f || exit 2
git archive a78f28f | tar -x -C $S/base-a78f28f || exit 2
cd $S/base-a78f28f || exit 2
cargo build -p retrace --target-dir $S/base-target > $L/t3-base-build.log 2>&1; echo "build exit=$?"
cp $S/base-target/aarch64-apple-darwin/debug/retrace $S/retrace-base || exit 2
codesign -f -s - --entitlements $W/retrace.entitlements $S/retrace-base 2>/dev/null; echo "sign exit=$?"
echo "base sha256=$(shasum -a 256 $S/retrace-base | cut -d' ' -f1)"
echo "t3   sha256=$(shasum -a 256 $S/retrace-t3 | cut -d' ' -f1)"
