#!/bin/bash
# t0 Step 6: classify every partial munmap the walk binary split (the probe's "[probe] partial munmap
# <ipa>+<len> of backing <bs>..<be>" lines). head: starts at the backing's start; tail: its rounded
# end is the backing's end; interior: neither. unaligned: ipa+len is not a multiple of 16 KiB.
# spans: ipa+len, rounded up to 16 KiB, passes the backing's end (the range covers more than the one
# backing the probe split). Usage: munmap-census.sh <tag>
P=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node/t0
grep -a 'partial munmap' $P/m2-$1.probe.txt | python3 -c '
import sys, re, collections
G = 0x4000
rows = []
for line in sys.stdin:
    m = re.search(r"^T(\d+) .*partial munmap (0x[0-9a-f]+)\+(0x[0-9a-f]+) of backing (0x[0-9a-f]+)\.\.(0x[0-9a-f]+)", line)
    t, ipa, ln, bs, be = int(m.group(1)), *(int(m.group(i), 16) for i in range(2, 6))
    end = ipa + ln
    ue = (end + G - 1) & ~(G - 1)
    kind = "head" if ipa == bs and ue < be else "tail" if ipa > bs and ue >= be else "interior" if ipa > bs else "whole?"
    rows.append((t, kind, ipa, ln, bs, be, end % G != 0, ue > be))
    print(f"T{t} {kind:8} ipa={ipa:#x} len={ln:#x} backing={bs:#x}..{be:#x} ({be-bs:#x}) unaligned_end={end % G != 0} spans_past_backing={ue > be}")
c = collections.Counter((r[1], r[6], r[7]) for r in rows)
print("-- summary (kind, unaligned_end, spans_past_backing): count")
for k, v in sorted(c.items()): print(f"   {k}: {v}")
print(f"-- total {len(rows)}; lengths: " + ", ".join(f"{k:#x}x{v}" for k, v in sorted(collections.Counter(r[3] for r in rows).items())))
'
