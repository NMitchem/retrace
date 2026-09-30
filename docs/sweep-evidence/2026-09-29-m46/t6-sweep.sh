#!/bin/bash
# M46 Task 6 Step 3 (M45 T3-a: detached, no concurrent cargo): the full corpus on a signed scratchpad
# copy of this task's binary, the M38/M39/M44/M45 wrapper shape (pidstart, then the binary's
# hash/commit/date, then the sweep), with the host's load average before and after (an unrelated
# cargo-mutants run shares the host).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
E=$W/docs/sweep-evidence/2026-09-29-m46
B=$S/retrace-t6
cd "$W" || exit 2
mkdir -p "$E" "$S"
cargo build -p retrace || exit 2
cp target/aarch64-apple-darwin/debug/retrace "$B" || exit 2
codesign -f -s - --entitlements retrace.entitlements "$B" 2>/dev/null || exit 2
export RETRACE_SWEEP_KEEP=$E/sweep
{
  echo "pidstart=$$"
  echo "binary=$B sha256=$(shasum -a 256 "$B" | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo "load-start: $(uptime)"
  tools/apple-sweep.sh "$B"
  echo "SWEEP_EXIT=$?"
  echo "load-end: $(uptime) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
} > "$E/sweep.log" 2>&1
