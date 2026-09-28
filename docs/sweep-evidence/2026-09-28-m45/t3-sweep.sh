#!/bin/bash
# M45 Task 3 Step 4 (controller-run, Ruling T3-a): the full corpus on a signed scratchpad copy of
# this task's binary, the M38/M39/M44 wrapper shape (pidstart, then the binary's hash/commit/date,
# then the sweep).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
E=$W/docs/sweep-evidence/2026-09-28-m45
B=$S/retrace-t3
cd "$W" || exit 2
mkdir -p "$E" "$S"
cargo build -p retrace || exit 2
cp target/aarch64-apple-darwin/debug/retrace "$B" || exit 2
codesign -f -s - --entitlements retrace.entitlements "$B" 2>/dev/null || exit 2
export RETRACE_SWEEP_KEEP=$E/sweep
{
  echo "pidstart=$$"
  echo "binary=$B sha256=$(shasum -a 256 "$B" | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  tools/apple-sweep.sh "$B"
  echo "SWEEP_EXIT=$?"
} > "$E/sweep.log" 2>&1
