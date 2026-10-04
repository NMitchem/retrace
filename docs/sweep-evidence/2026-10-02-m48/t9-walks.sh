#!/bin/bash
# M48 Task 9: the five walks t0 made, their censuses, and the crash demo's debug session on the
# release binary, one after another. Traces are deleted at the end; none is evidence.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
B=/private/tmp/claude-501/m48-t9-release-retrace
cd "$W" || exit 2
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') commit=$(git rev-parse --short HEAD) load=$(sysctl -n vm.loadavg) sha256=$(shasum -a 256 $B | cut -d' ' -f1)"
bash $E/t9-walk.sh e -- -e 'console.log(1)'
bash $E/t9-walk.sh t10 -- -e 'setTimeout(() => console.log(2), 10)'
bash $E/t9-walk.sh t2000 -- -e 'setTimeout(() => console.log(2), 2000)'
bash $E/t9-walk.sh natives -- --allow-natives-syntax -e 'function f(a,v){a[0]=v} const a=new BigUint64Array(1); %PrepareFunctionForOptimization(f); f(a,1n); f(a,2n); %OptimizeFunctionOnNextCall(f); f(a,3n); console.log(String(a[0]))'
bash $E/t9-walk.sh crash -- --allow-natives-syntax $W/crates/retrace-guest/node/crash.js /private/tmp/claude-501/m48-t9-crash_addon.node
for t in e t10 t2000 natives crash; do bash $E/t9-census.sh $t > $E/walk-$t.census 2>&1; done
CELL=$(grep -a -o 'cell=0x[0-9a-f]*' $E/walk-crash.out | head -1 | cut -d= -f2)
/usr/bin/time -l perl -e 'alarm 900; exec @ARGV' $B debug /private/tmp/claude-501/m48-t9-crash.bin --script "continue; watch $CELL 8; reverse-continue; x $CELL 8; stepi; x $CELL 8" > $E/walk-crash.dbg.out 2> $E/walk-crash.dbg.err
echo "debug rc=$?"
rm -f /private/tmp/claude-501/m48-t9-e.bin /private/tmp/claude-501/m48-t9-t10.bin /private/tmp/claude-501/m48-t9-t2000.bin /private/tmp/claude-501/m48-t9-natives.bin /private/tmp/claude-501/m48-t9-crash.bin
echo "walks done $(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)"
