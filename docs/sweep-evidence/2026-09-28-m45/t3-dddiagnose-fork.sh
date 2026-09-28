#!/bin/bash
# M45 Task 3 Step 4: where a dddiagnose run that crashes and one that reaches msgh_id 205 part
# ways — the first differing `[trap]` number, by position, between two traced recordings.
# Usage: t3-dddiagnose-fork.sh <run-a> <run-b>   (names under the scratchpad's ddctl/)
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
O=$S/ddctl
grep -a '^\[trap\]' $O/$1.rec.err | sed 's/ pc=.*//' > $O/$1.nums
grep -a '^\[trap\]' $O/$2.rec.err | sed 's/ pc=.*//' > $O/$2.nums
n=$(cmp $O/$1.nums $O/$2.nums | sed -n 's/.* line \([0-9]*\).*/\1/p')
echo "first differing [trap] line: ${n:-none}"
[ -n "$n" ] || exit 0
lo=$((n - 6)); hi=$((n + 3))
echo "--- $1"; grep -a '^\[trap\]' $O/$1.rec.err | sed -n "${lo},${hi}p" | cut -c1-220
echo "--- $2"; grep -a '^\[trap\]' $O/$2.rec.err | sed -n "${lo},${hi}p" | cut -c1-220
