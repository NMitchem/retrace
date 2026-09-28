#!/bin/bash
# M44 Task 13 Step 1 (controller-run): the full corpus on a signed scratchpad copy of the close's
# binary, the M38/M39 wrapper shape (pidstart, then the binary's hash/commit/date, then the sweep).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/1996e432-75ea-4d65-a52b-ed70d24a05b8/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed
E=$W/docs/sweep-evidence/2026-09-27-m44
B=$S/retrace-t13
cd "$W" || exit 2
mkdir -p "$E"
cp target/aarch64-apple-darwin/debug/retrace "$B" || exit 2
codesign -f -s - --entitlements retrace.entitlements "$B" 2>/dev/null || exit 2
export RETRACE_SWEEP_KEEP=$E/sweep
{
  echo "pidstart=$$"
  echo "binary=$B sha256=$(shasum -a 256 "$B" | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  tools/apple-sweep.sh "$B"
  echo "SWEEP_EXIT=$?"
} > "$E/sweep.log" 2>&1
