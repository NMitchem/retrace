#!/bin/bash
# M45 Task 3 Step 4: replay the sweep's own /bin/ps recording (made by retrace-t3, 09a105f) on
# BOTH binaries. If the base binary (a78f28f = M44's close crates) stops at the same landmark, ipa
# and bytes, the divergence is carried by the recording, not by anything M45's replay does.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
for b in base t3; do
  perl -e 'alarm 120; exec @ARGV' $S/retrace-$b replay $E/sweep/ps.bin > $S/psctl/sweeptrace-$b.rp.out 2> $S/psctl/sweeptrace-$b.rp.err
  echo "replay of sweep ps.bin on $b: rp=$? $(grep -a -m1 DIVERGENCE $S/psctl/sweeptrace-$b.rp.err)"
done
