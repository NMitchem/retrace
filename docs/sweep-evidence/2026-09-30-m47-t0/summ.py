# M47 t0: summarize madvise (75) and __mac_syscall (381) lines from census.tsv and M4's *.rec.err.
import collections, re, sys
adv, align, mac = collections.Counter(), collections.Counter(), collections.Counter()
who = collections.defaultdict(set)
T75 = re.compile(r'\[trap\] num=75 .*?args=\[(0x[0-9a-f]+),(0x[0-9a-f]+),(0x[0-9a-f]+)')
M381 = re.compile(r'\[m47\] num=381 args=\[(0x[0-9a-f]+),(0x[0-9a-f]+),(0x[0-9a-f]+)\] policy="([^"]*)" op="([^"]*)" ret=(0x[0-9a-f]+) err=(\w+) writes=(\d+)')
for path in sys.argv[1:]:
    for line in open(path, errors='replace'):
        label, rest = line.rstrip('\n').split('\t', 1) if '\t' in line else (path, line.rstrip('\n'))
        m = T75.search(rest)
        if m:
            a, l, v = (int(x, 16) for x in m.groups())
            adv[v & 0xffffffff] += 1; who[('madvise', v & 0xffffffff)].add(label)
            align[('addr16k' if a % 0x4000 == 0 else 'addr-UNALIGNED', 'len16k' if l % 0x4000 == 0 else 'len-UNALIGNED', 'len0' if l == 0 else 'len>0')] += 1
        m = M381.search(rest)
        if m:
            key = (m.group(4), int(m.group(2), 16) & 0xffffffff, m.group(5), m.group(6), m.group(7), m.group(8))
            mac[key] += 1; who[('mac',) + key].add(label)
        if rest.startswith('[probe] AMFI'):
            mac[('AMFI', 0x5a, '', rest, '', '')] += 1; who[('amfi',)].add(label)
        if rest.startswith('[m47] amfi policy_resident='):
            mac[('AMFI-resident', 0, rest.split('=', 1)[1], '', '', '')] += 1
print('madvise advice census:'); [print(f'  advice {k}: {n} calls, guests {sorted(who[("madvise", k)])[:8]}') for k, n in sorted(adv.items())]
print('madvise alignment:'); [print(f'  {k}: {n}') for k, n in sorted(align.items())]
print('__mac_syscall census (policy, call, op, ret, err, writes):'); [print(f'  {k}: {n}') for k, n in sorted(mac.items(), key=str)]
