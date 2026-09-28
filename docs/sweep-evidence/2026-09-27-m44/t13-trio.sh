#!/bin/bash
# M44 Task 13: the full sweep keeps no rec.err for a PASS row, so the trio's rc=71 is re-run alone on
# the SAME signed binary with KEEP_ALL (warm xcrun cache, as the full sweep had) to show the line.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/1996e432-75ea-4d65-a52b-ed70d24a05b8/scratchpad
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed
cd "$W" || exit 2
ls -la /var/tmp/xcrun_db 2>&1 | head -1
printf '/usr/bin/desdp\n/usr/bin/dyld_info\n/usr/bin/flex\n' > $S/t13-trio-list.txt
export RETRACE_SWEEP_LIST=$S/t13-trio-list.txt
export RETRACE_SWEEP_KEEP=$S/t13-trio
export RETRACE_SWEEP_KEEP_ALL=1
rm -rf $S/t13-trio
tools/apple-sweep.sh $S/retrace-t13 > $S/t13-trio.log 2>&1
echo "exit=$?"
grep -a -E '^(PASS|FAIL|TALLY)' $S/t13-trio.log
grep -a -H 'refusing posix_spawn' $S/t13-trio/*.rec.err
rm -f $S/t13-trio/*.bin
