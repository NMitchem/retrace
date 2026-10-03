const addon = require(process.argv[2]);
const fs = require('fs');
const rows = JSON.parse(fs.readFileSync(process.argv[3], 'utf8')).rows;
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
