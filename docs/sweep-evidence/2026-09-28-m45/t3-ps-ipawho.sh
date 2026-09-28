#!/bin/bash
# M45 Task 3 Step 4: which landmarks' recorded writes cover /bin/ps's divergent ipa 0x701414078,
# and what the first and final snapshots hold there (throwaway trace reader, retrace-trace by path).
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
cd $S/ipawho && cargo build -q --release || exit 2
$S/ipawho/target/release/ipawho ${1:-$E/sweep/ps.bin} ${2:-0x701414078} $3 $4
