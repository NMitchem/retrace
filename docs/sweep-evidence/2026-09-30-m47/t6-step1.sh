#!/bin/bash
# M47 Task 6 Step 1 (the brief's, verbatim in substance): node to its wall on the finished M47 build,
# traced, with the record's exit code, its [trap] count, its first stop line and its last traps.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
cd "$W" || exit 2
mkdir -p "$E"
NODE=$(python3 -c 'import os; print(os.path.realpath("/opt/homebrew/bin/node"))')
echo "node=$NODE version=$($NODE --version) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
echo "load-start: $(sysctl -n vm.loadavg)"
export RETRACE_TRACE=1
start=$(date +%s)
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $NODE -o /private/tmp/claude-501/m47-node.bin -- -e 'console.log(1)' > $E/node.rec.out 2> $E/node.rec.err
echo "record exit=$?"
end=$(date +%s)
unset RETRACE_TRACE
echo "secs=$((end - start)) trace-bytes=$(stat -f %z /private/tmp/claude-501/m47-node.bin 2>/dev/null)"
echo "load-end: $(sysctl -n vm.loadavg)"
echo "traps=$(grep -a -c '^\[trap\] ' $E/node.rec.err)"
echo "--- first stop lines"
grep -a -E 'panicked at|RECORD ERROR|M33:|M47:' $E/node.rec.err | head -3
echo "--- last three traps"
grep -a '^\[trap\] ' $E/node.rec.err | tail -3
echo "--- stderr lines, stdout bytes"
wc -l < $E/node.rec.err
wc -c < $E/node.rec.out
