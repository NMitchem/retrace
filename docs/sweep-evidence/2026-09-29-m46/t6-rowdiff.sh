#!/bin/bash
# M46 Task 6 Step 3: the brief's raw ROW diff against M45's sweep (rowdiff.txt), then the M45
# t3-rowdiff.sh normalised diff (rowdiff-norm.txt): keyed on the binary's path, comparing result,
# rc, rp and landmark, then rec_reason with the Rust thread id normalised out.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
E=$W/docs/sweep-evidence/2026-09-29-m46
A=$W/docs/sweep-evidence/2026-09-28-m45/sweep.log
B=$E/sweep.log
cd "$W" || exit 2
grep -a '^TALLY' $B
grep -a '^ROW' $B > $E/rows.txt
grep -a '^ROW' $A | diff - $E/rows.txt > $E/rowdiff.txt; echo "diff=$?"
norm() { grep -a '^ROW' "$1" | awk -F'\t' '{ r=$8; gsub(/\(([0-9]+)\)/, "(TID)", r); print $2 "\t" $3 "\t" $4 "\t" $5 "\t" $7 "\t" substr(r, 1, 120) }' | sort; }
norm $A > $S/m45rows.$$ ; norm $B > $S/m46rows.$$
{
  echo "rows: M45 $(wc -l < $S/m45rows.$$ | tr -d ' '), M46 $(wc -l < $S/m46rows.$$ | tr -d ' ')"
  echo "identical rows: $(comm -12 $S/m45rows.$$ $S/m46rows.$$ | wc -l | tr -d ' ')"
  echo "--- M45 side of each differing row"; comm -23 $S/m45rows.$$ $S/m46rows.$$
  echo "--- M46 side of each differing row"; comm -13 $S/m45rows.$$ $S/m46rows.$$
} > $E/rowdiff-norm.txt
cat $E/rowdiff-norm.txt
rm -f $S/m45rows.$$ $S/m46rows.$$
