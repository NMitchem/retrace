#!/usr/bin/env python3
# Check the prediction rule (abort iff segment base in [0xa007b4000, 0xa00800000)) over every logged
# run: the txt summaries carry rc and segment per run.
import re, glob
rows = []
for f in ['vm1', 'ent1', 'ent2', 'ent3']:
    pass
# vm1: rc from loopvm output is not saved per file; read segment from err and rc from 'guest terminated'
for err in sorted(glob.glob('/private/tmp/claude-501/m47-abort/vm1/run-*.err')):
    t = open(err, errors='replace').read()
    m = re.search(r'mask 0x3fffff .*?-> (0x[0-9a-f]+)', t)
    rows.append((err, int(m.group(1), 16), 'signal 6' in t))
for err in sorted(glob.glob('/private/tmp/claude-501/m47-abort/ent[123]/*.err')):
    t = open(err, errors='replace').read()
    m = re.search(r'mask 0x3fffff .*?-> (0x[0-9a-f]+)', t)
    if not m: continue
    rows.append((err, int(m.group(1), 16), 'signal 6' in t))
ok = 0
for err, seg, aborted in rows:
    pred = 0xa007b4000 <= seg < 0xa00800000
    if pred == aborted: ok += 1
    else: print('MISMATCH', err, hex(seg), aborted)
print(f'runs={len(rows)} aborts={sum(a for _, _, a in rows)} rule_correct={ok}')
