#!/usr/bin/env python3
# Parse a pctrace.sh regs dump into a list of (pc, regs dict). Print pc list or compare two.
import re, sys

def parse(path):
    text = open(path, errors='replace').read()
    steps = []
    cur = {}
    for m in re.finditer(r'(x\d+|sp|pc|elr|far|spsr)\s*=\s*(0x[0-9a-f]+)', text):
        k, v = m.group(1), int(m.group(2), 16)
        if k == 'x0' and cur:
            steps.append(cur); cur = {}
        cur[k] = v
    if cur: steps.append(cur)
    return steps

if __name__ == '__main__':
    a = parse(sys.argv[1])
    if len(sys.argv) == 2:
        for s in a: print(hex(s.get('pc', 0)))
        sys.exit()
    b = parse(sys.argv[2])
    for i, (x, y) in enumerate(zip(a, b)):
        if x.get('pc') != y.get('pc'):
            print(f'control flow diverges at step {i}: A pc={x["pc"]:#x} B pc={y["pc"]:#x}')
            for j in range(max(0, i - 6), i + 1):
                pa, pb = a[j], b[j]
                diffs = [k for k in pa if pa.get(k) != pb.get(k) and k not in ('elr', 'far')]
                print(f'  step {j} pc A={pa["pc"]:#x} B={pb["pc"]:#x} ' +
                      ' '.join(f'{k}:{pa[k]:#x}/{pb.get(k, 0):#x}' for k in diffs))
            break
    else:
        print('no pc divergence in', min(len(a), len(b)), 'steps')
