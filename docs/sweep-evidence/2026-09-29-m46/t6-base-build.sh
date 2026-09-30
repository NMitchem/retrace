#!/bin/bash
# M46 Task 6 Step 3: build M46's base, the M45 merge f907c33, from a `git archive` extracted into
# /private/tmp/claude-501/m46-base (the brief's two commands, run before this script), with its own
# target dir, then sign a copy in the session scratchpad (the M45 t3-base-build.sh shape).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
L=$W/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
cd /private/tmp/claude-501/m46-base || exit 2
ls -a | head -5
cargo build -p retrace --target-dir /private/tmp/claude-501/m46-base-target > $L/t6-base-build.log 2>&1; echo "build exit=$?"
cp /private/tmp/claude-501/m46-base-target/aarch64-apple-darwin/debug/retrace $S/retrace-base || exit 2
codesign -f -s - --entitlements $W/retrace.entitlements $S/retrace-base 2>/dev/null; echo "sign exit=$?"
echo "base sha256=$(shasum -a 256 $S/retrace-base | cut -d' ' -f1)"
