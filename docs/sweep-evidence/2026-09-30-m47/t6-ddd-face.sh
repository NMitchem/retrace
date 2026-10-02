#!/bin/bash
# M47 Task 6: the third dddiagnose outcome, seen once in the controls on the BASE binary (round d5-base):
# `RECORD ERROR: non-syscall exit: data abort (EC=0x24 ISS=0x7 FSC=0x7) far/ipa=0x1bf0 (UNMAPPED)
# pc=0x193bbbca0 elr=0x193bbbc9c`. Symbolicates pc/elr with lldb against /usr/bin/dddiagnose and the host's
# shared cache (not running: unslid addresses, as M46's t6-frames.sh), and keeps the run's rec.err.
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
export LC_ALL=C
cp $S/ctl/d5-base/dddiagnose.rec.err $E/dddiagnose.d5-base.rec.err
{
  echo "# dddiagnose, controls round d5-base (base binary m47-base-retrace, 427fa0a's code): the recorder's last line"
  tail -1 $E/dddiagnose.d5-base.rec.err
  echo "# lldb (target /usr/bin/dddiagnose, not running)"
  lldb --batch -o 'target create /usr/bin/dddiagnose' -o 'image lookup -a 0x193bbbca0' -o 'image lookup -a 0x193bbbc9c' -o 'disassemble -s 0x193bbbc80 -e 0x193bbbcb0' 2>&1 | grep -a -v '^Current executable'
} > $E/dddiagnose.face3.txt 2>&1
echo "exit=$?"
