#!/bin/bash
# M48 Task 9 Step 6: the brief's outcome diff (path + result, cut -f2,3) against M47's a8a1ecd sweep
# (rowdiff.txt), then M45/M46's normalised diff (rowdiff-norm.txt): keyed on the binary's path,
# comparing result, rc, rp, landmark and rec_reason with the Rust thread id normalised out.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
S=/private/tmp/claude-501
A=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-09-30-m47/sweep.log
B=$E/sweep.log
cd "$W" || exit 2
grep -a '^TALLY' $A
grep -a '^TALLY' $B
grep -a '^ROW' $A | cut -f2,3 > $E/rows-base.txt
grep -a '^ROW' $B | cut -f2,3 > $E/rows.txt
diff $E/rows-base.txt $E/rows.txt > $E/rowdiff.txt; echo "diff=$?"
cat $E/rowdiff.txt
norm() { grep -a '^ROW' "$1" | awk -F'\t' '{ r=$8; gsub(/\(([0-9]+)\)/, "(TID)", r); print $2 "\t" $3 "\t" $4 "\t" $5 "\t" $7 "\t" substr(r, 1, 120) }' | sort; }
norm $A > $S/m48base.$$ ; norm $B > $S/m48rows.$$
{
  echo "rows: M47 a8a1ecd $(wc -l < $S/m48base.$$ | tr -d ' '), M48 $(wc -l < $S/m48rows.$$ | tr -d ' ')"
  echo "identical rows: $(comm -12 $S/m48base.$$ $S/m48rows.$$ | wc -l | tr -d ' ')"
  echo "--- M47 a8a1ecd side of each differing row"; comm -23 $S/m48base.$$ $S/m48rows.$$
  echo "--- M48 side of each differing row"; comm -13 $S/m48base.$$ $S/m48rows.$$
} > $E/rowdiff-norm.txt
cat $E/rowdiff-norm.txt
rm -f $S/m48base.$$ $S/m48rows.$$
