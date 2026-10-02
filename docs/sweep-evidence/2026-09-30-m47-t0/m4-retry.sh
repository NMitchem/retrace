#!/bin/zsh
# M47 t0 M4 follow-up: re-run the rows that failed with rc 134 (M1(b)'s intermittent libmalloc
# abort) up to 3 more times each, with m4.sh's OWN function bodies (sourced from m4.sh's text up to
# its first command, so they cannot drift), into a separate directory. Each retry also records which
# abort it was (the console's last lines and whether `guest terminated by signal 6` appeared).
# Usage: m4-retry.sh <out-dir> "<tag> <kind> <prep|-> <git args…>" …   (kind = read|write)
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
FUNCS=/private/tmp/claude-501/m47-m4-funcs.zsh
sed -n '1,/^read_cmd status-porcelain/p' $L/t0/m4.sh | sed '$d' > $FUNCS
OUT=$1; shift
set -- $OUT "$@"
source $FUNCS          # sets G, R, O=$1 (truncates $O/nums.txt), ID, and the functions
shift
for spec in "$@"; do
  words=(${=spec}); tag=$words[1]; kind=$words[2]
  for try in 1 2 3; do
    if [ $kind = read ]; then read_cmd $tag-try$try ${words[3,-1]}; r=$rrc
    else write_cmd $tag-try$try ${words[3]} ${words[4,-1]}; r=$rrc; fi
    [ $r = 134 ] || break
  done
done
