#!/usr/bin/env python3
"""Summarise tools/destgaps-census.sh output: per syscall, the length operand's distribution, max vs 64 KiB,
and the (op/flavor, len) combinations seen, with which guests issued them."""
import re, sys, collections

WINDOW = 64 * 1024
# (dest index, len index, op-describing indices)
SHAPE = {
    336: ("proc_info",        4, 5, (0, 2)),   # callnum=x0, flavor=x2
    220: ("getattrlist",      2, 3, ()),
    228: ("fgetattrlist",     2, 3, ()),
    169: ("csops",            2, 3, (1,)),     # ops=x1
    170: ("csops_audittoken", 2, 3, (1,)),
}
line_re = re.compile(r'^(?P<label>[^\t]+)\t\[trap\] num=(?P<num>\d+) \(0x[0-9a-f]+\) pc=(?P<pc>0x[0-9a-f]+) args=\[(?P<args>[^\]]*)\]')

rows = []
for line in open(sys.argv[1]):
    m = line_re.match(line.rstrip('\n'))
    if not m:
        continue
    args = [int(a, 16) for a in m['args'].split(',')]
    rows.append((m['label'], int(m['num']), int(m['pc'], 16), args))

print(f"total matched dispatches: {len(rows)} across {len({r[0] for r in rows})} guests\n")
for num, (name, di, li, ops) in SHAPE.items():
    rs = [r for r in rows if r[1] == num]
    guests = sorted({r[0] for r in rs})
    lens = [r[3][li] for r in rs]
    print(f"== {name} ({num}): {len(rs)} dispatches, {len(guests)} guests, "
          f"len operand x{li}: max={max(lens) if lens else 0} (0x{max(lens) if lens else 0:x}) "
          f"{'EXCEEDS' if lens and max(lens) > WINDOW else 'fits'} 64 KiB")
    combos = collections.defaultdict(set)
    for label, _, pc, args in rs:
        key = tuple(args[i] for i in ops) + (args[li],)
        combos[key].add(label.split(':')[0] + (':' + label.split(':',1)[1] if label.startswith('guest') else ''))
    for key in sorted(combos):
        opstr = ' '.join(f"x{i}=0x{v:x}" for i, v in zip(ops, key[:-1]))
        who = sorted(combos[key])
        who_s = ', '.join(who[:6]) + (f", … +{len(who)-6}" if len(who) > 6 else '')
        print(f"   {opstr:<24} len=0x{key[-1]:<6x} ({key[-1]:>6})  [{len(who)} guests: {who_s}]")
    # sanity: dest pointer non-null?
    nulls = sum(1 for r in rs if r[3][di] == 0)
    if nulls:
        print(f"   NOTE: {nulls} dispatches with NULL dest pointer x{di}")
    print()
