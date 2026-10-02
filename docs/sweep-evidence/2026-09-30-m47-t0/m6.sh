#!/bin/zsh
# M47 t0 Step 12 (M6): the base #[test] count and test-file count, with the per-file breakdown kept
# for the close's file-by-file reconciliation. The two counting commands are the brief's verbatim.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
echo "# M47 t0 M6 on the tree at $(git rev-parse --short HEAD), $(date '+%Y-%m-%d')"
echo "\$ grep -r -c -E '^\\s*#\\[test\\]' crates --include='*.rs' | awk -F: '{s+=\$2} END {print s}'"
grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
echo "\$ ls crates/*/tests/*.rs | wc -l"
ls crates/*/tests/*.rs | wc -l
echo "# per file (nonzero counts):"
grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | grep -v ':0$' | sort
