#!/bin/bash
# Owed T6-a: the Apple sweep on an unloaded host, on a signed scratchpad copy of main 427fa0a's CLI
# (the M46 t6-sweep.sh wrapper shape: pidstart, binary hash/commit/date, load before and after).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/f3fccc20-651e-4651-96ff-661888b49a37/scratchpad
B=$S/retrace; E=$S/sweep
cd /Users/noahmitchem/Documents/GitHub/retrace || exit 2
export RETRACE_SWEEP_KEEP=$E/keep
{
  echo "pidstart=$$"
  echo "binary=$B sha256=$(shasum -a 256 "$B" | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo "load-start: $(uptime)"
  tools/apple-sweep.sh "$B"
  echo "SWEEP_EXIT=$?"
  echo "load-end: $(uptime) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
} > "$E/sweep.log" 2>&1
