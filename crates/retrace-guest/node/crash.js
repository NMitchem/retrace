// M48 rung 10 (spec §3g, D1): node's crash demo, rung 8's shape in JavaScript. The target is
// COMPUTED from crash.json, never a literal here. TurboFan-compiled code stores it into an
// ArrayBuffer's backing store, and the addon loads through it, which faults. The marker line
// reveals the cell (the M6 marker convention), so a test DISCOVERS it from the recording.
//
// Run as `node --allow-natives-syntax crash.js <addon>`. The natives force `store` through
// TurboFan synchronously on main (R6). Left to its heuristics, V8 compiles on a worker thread,
// which the cooperative scheduler runs only when main blocks, and main never blocks here.
//
// 0x4000_DEAD_0000 has bit 46 set (an L1 slot that is never mapped, below 2^47), the FAR crashy.c
// and crash.py use, so the load is a level-1 translation fault at exactly the target.
const addon = require(process.argv[2]);
const fs = require('fs');
const path = require('path');
const rows = JSON.parse(fs.readFileSync(path.join(__dirname, 'crash.json'), 'utf8')).rows;
const target = BigInt(rows[0].value) + BigInt(rows[1].value);
const ab = new ArrayBuffer(8);
const cell = addon.addressOf(ab);
function store(view, v) { view[0] = v; }
const view = new BigUint64Array(ab);
%PrepareFunctionForOptimization(store);
store(view, 1n); store(view, 2n);
%OptimizeFunctionOnNextCall(store);
store(view, target);
console.log(`CRASHJS cell=0x${cell.toString(16)} target=0x${target.toString(16)} rows=${rows.length} opt=${%GetOptimizationStatus(store).toString(2)}`);
addon.deref(ab);
console.log('UNREACHED');
