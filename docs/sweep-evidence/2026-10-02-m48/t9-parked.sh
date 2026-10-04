#!/bin/bash
# M48 Task 9: every parked gate, run with --ignored on the final tree, so a wall M48 moved is seen
# (CLAUDE.md, honest-gate discipline). Each must still fail, at the wall its #[ignore] reason names.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
L=$W/.superpowers/sdd/2026-10-02-retrace-m48-node
cd "$W" || exit 2
echo "#[ignore] lines: $(git grep -c -E '^\s*#\[ignore' -- crates | awk -F: '{s+=$2} END {print s}')"
cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --ignored --test-threads=1 > $L/t9-parked-apple.log 2>&1; echo "apple_walls_e2e exit=$?"
cargo test -p retrace --test stackoverflow_rust_e2e --no-fail-fast -- --ignored --test-threads=1 > $L/t9-parked-stackoverflow.log 2>&1; echo "stackoverflow_rust_e2e exit=$?"
cargo test -p retrace --test symbols_e2e --no-fail-fast -- --ignored --test-threads=1 > $L/t9-parked-symbols.log 2>&1; echo "symbols_e2e exit=$?"
grep -a -h -E '^test .* (ok|FAILED)$' $L/t9-parked-*.log
grep -a -h -A3 -E "panicked at|^---- " $L/t9-parked-*.log | cut -c1-300
