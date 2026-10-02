#!/bin/bash
# M47 Task 6 Step 3: the brief's outcome diff (path + result, cut -f2,3) against the 427fa0a baseline
# (rowdiff.txt), then M45/M46's normalised diff (rowdiff-norm.txt): keyed on the binary's path,
# comparing result, rc, rp, landmark and rec_reason with the Rust thread id normalised out.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
A=$W/docs/sweep-evidence/2026-09-30-m47-probe/sweep-427fa0a.log
B=$E/sweep.log
cd "$W" || exit 2
grep -a '^TALLY' $A
grep -a '^TALLY' $B
grep -a '^ROW' $A | cut -f2,3 > $E/rows-base.txt
grep -a '^ROW' $B | cut -f2,3 > $E/rows.txt
diff $E/rows-base.txt $E/rows.txt > $E/rowdiff.txt; echo "diff=$?"
cat $E/rowdiff.txt
norm() { grep -a '^ROW' "$1" | awk -F'\t' '{ r=$8; gsub(/\(([0-9]+)\)/, "(TID)", r); print $2 "\t" $3 "\t" $4 "\t" $5 "\t" $7 "\t" substr(r, 1, 120) }' | sort; }
norm $A > $S/baserows.$$ ; norm $B > $S/m47rows.$$
{
  echo "rows: 427fa0a $(wc -l < $S/baserows.$$ | tr -d ' '), M47 $(wc -l < $S/m47rows.$$ | tr -d ' ')"
  echo "identical rows: $(comm -12 $S/baserows.$$ $S/m47rows.$$ | wc -l | tr -d ' ')"
  echo "--- 427fa0a side of each differing row"; comm -23 $S/baserows.$$ $S/m47rows.$$
  echo "--- M47 side of each differing row"; comm -13 $S/baserows.$$ $S/m47rows.$$
} > $E/rowdiff-norm.txt
cat $E/rowdiff-norm.txt
rm -f $S/baserows.$$ $S/m47rows.$$
