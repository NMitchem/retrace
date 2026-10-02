#!/bin/bash
# M47 Task 6: the two edited test targets (after the sweep and the controls, never during), the two
# re-parked shells run with --ignored as the positive control (each must FAIL, its panic naming
# syscall 7), and clippy. Each cargo exit code is captured before any pipe.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
L=$W/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cd "$W" || exit 2
cargo test -p retrace --test node_e2e --no-fail-fast -- --test-threads=1 > $L/t6-node_e2e.log 2>&1
echo "node_e2e exit=$?"
cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --test-threads=1 > $L/t6-apple_walls_e2e.log 2>&1
echo "apple_walls_e2e exit=$?"
cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --test-threads=1 --ignored csh_records_and_replays > $L/t6-csh-ignored.log 2>&1
echo "csh --ignored exit=$?"
cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --test-threads=1 --ignored tcsh_records_and_replays > $L/t6-tcsh-ignored.log 2>&1
echo "tcsh --ignored exit=$?"
cargo clippy -p retrace --all-targets -- -D warnings > $L/t6-clippy.log 2>&1
echo "clippy exit=$?"
for f in t6-node_e2e t6-apple_walls_e2e t6-csh-ignored t6-tcsh-ignored; do
  echo "--- $f"; grep -a -E '^test |test result|SKIPPED|syscall 7' $L/$f.log | cut -c1-200
done
echo "--- clippy"; tail -3 $L/t6-clippy.log
