#!/bin/bash
# M45 Task 3 Step 4: the row-by-row diff of the sweep against M44's, keyed on the binary's path,
# comparing result, rc, rp and landmark, then rec_reason with the Rust thread id normalised out.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit
A=$W/docs/sweep-evidence/2026-09-27-m44/sweep.log
B=$W/docs/sweep-evidence/2026-09-28-m45/sweep.log
norm() { grep -a '^ROW' "$1" | awk -F'\t' '{ r=$8; gsub(/\(([0-9]+)\)/, "(TID)", r); print $2 "\t" $3 "\t" $4 "\t" $5 "\t" $7 "\t" substr(r, 1, 120) }' | sort; }
norm $A > /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m44rows.$$ ; norm $B > /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m45rows.$$
echo "rows: M44 $(wc -l < /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m44rows.$$), M45 $(wc -l < /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m45rows.$$)"
echo "identical rows: $(comm -12 /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m44rows.$$ /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m45rows.$$ | wc -l | tr -d ' ')"
echo "--- M44 side of each differing row"; comm -23 /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m44rows.$$ /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m45rows.$$
echo "--- M45 side of each differing row"; comm -13 /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m44rows.$$ /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m45rows.$$
rm -f /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m44rows.$$ /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad/m45rows.$$
