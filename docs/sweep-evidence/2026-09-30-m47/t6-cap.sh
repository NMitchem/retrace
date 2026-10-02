#!/bin/bash
# M47 Task 6: cap node.rec.err (a [trap] line per trap) to its last 400 lines before commit (brief Step 1),
# under a first line naming the cap (the 2026-09-30-m47-abort convention). Then remove the sweep's kept
# traces (.bin), which are never committed (read by t6 first: the dddiagnose crash line and the shells'
# landmarks).
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
n=$(wc -l < $E/node.rec.err | tr -d ' ')
if [ "$n" -gt 400 ]; then
  { echo "[capped: the last 400 of $n lines of the traced record's stderr (t6-step1.sh)]"; tail -400 $E/node.rec.err; } > $S/node.rec.err.capped
  mv $S/node.rec.err.capped $E/node.rec.err
fi
echo "node.rec.err lines now: $(wc -l < $E/node.rec.err | tr -d ' ') (was $n)"
rm -f $E/sweep/*.bin
ls $E/sweep
