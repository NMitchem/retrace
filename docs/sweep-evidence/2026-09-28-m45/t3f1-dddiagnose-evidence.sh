#!/bin/bash
# M45 Task 3 fix round 1: the dddiagnose claims' supporting output, committed rather than left in
# the scratchpad. From the eight traced rec.err files of t3-dddiagnose-control.sh (rounds a, b):
#   dddiagnose-traps.txt   per run: [trap] count, kevent_qos (374) traps, the trap # of the
#                          RCV-only mach_msg2 (options 0x404000102) the box refuses, the refusal
#                          lines, and the outcome line;
#   dddiagnose-prefork.txt every [trap] line that differs, arguments included, between a crashing
#                          run (a1-base) and a 205 run (a2-base) before their first differing trap
#                          NUMBER (line 209);
#   dddiagnose-a1-base.vm / dddiagnose-a2-base.vm   the two runs' VM / sysctl / proc_info /
#                          mach_msg2 [trap] lines (pc stripped), which dddiagnose-fork.txt's
#                          "86th call" cite counts in.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/5cf69a66-efd8-4214-bb09-e6d2186f9103/scratchpad
O=$S/ddctl
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m45-kqinit/docs/sweep-evidence/2026-09-28-m45
{
  echo "# run: traps = [trap] lines; 374 = kevent_qos traps; rcv# = trap # of the RCV-only mach_msg2 (options 0x404000102)"
  for r in a1-t3 a1-base a2-t3 a2-base b1-t3 b1-base b2-t3 b2-base; do
    t=$(grep -a -c '^\[trap\]' $O/$r.rec.err)
    k=$(grep -a -c '^\[trap\] num=374 ' $O/$r.rec.err)
    n=$(grep -a '^\[trap\]' $O/$r.rec.err | grep -n ',0x404000102,' | cut -d: -f1 | tr '\n' ' ')
    rf=$(grep -a -c 'refusing mach_msg2 message-queue receive' $O/$r.rec.err)
    out=$(grep -a -E 'RECORD ERROR|guest crashed' $O/$r.rec.err | head -1 | cut -c1-110)
    echo "$r traps=$t 374=$k rcv#=$n receive-refusals=$rf :: $out"
  done
} > $E/dddiagnose-traps.txt
grep -a '^\[trap\]' $O/a1-base.rec.err | sed -n '1,208p' > $S/a1-pre.txt
grep -a '^\[trap\]' $O/a2-base.rec.err | sed -n '1,208p' > $S/a2-pre.txt
{
  echo "# [trap] lines 1-208 (before the first differing trap NUMBER, line 209): every line whose"
  echo "# text differs between a1-base (fault) and a2-base (msgh_id 205), arguments included."
  diff $S/a1-pre.txt $S/a2-pre.txt
} > $E/dddiagnose-prefork.txt
grep -a '^\[trap\] num=\(-14\|-15\|-12\|-10\|197\|202\|336\|339\|-47\) ' $O/a1-base.rec.err | sed 's/ pc=/\t/' | cut -c1-170 > $E/dddiagnose-a1-base.vm
grep -a '^\[trap\] num=\(-14\|-15\|-12\|-10\|197\|202\|336\|339\|-47\) ' $O/a2-base.rec.err | sed 's/ pc=/\t/' | cut -c1-170 > $E/dddiagnose-a2-base.vm
rm -f $S/a1-pre.txt $S/a2-pre.txt
cat $E/dddiagnose-traps.txt
echo "--- prefork diff lines: $(grep -c '^[<>]' $E/dddiagnose-prefork.txt)"
cat $E/dddiagnose-prefork.txt | cut -c1-200
wc -l $E/dddiagnose-a1-base.vm $E/dddiagnose-a2-base.vm
