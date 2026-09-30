#!/bin/bash
# M46 Task 6: who issues the refused kevent_id (375). Stops the debugger at the stub svc (pc
# 0x1804afa70) of the partial recording, dumps the id's queue label and the stack, walks the
# frame-pointer chain from the caller's frame (x29 = 0x27ff440 at the svc, in entry.txt), strips
# each saved lr's PAC bits (low 40 bits kept; every address here is below 2^40), and symbolicates
# x30 and each lr with lldb against the host's shared cache (the guest's libraries sit at their
# unslid addresses: x30 is exactly `_dispatch_kq_poll+216`'s `bl kevent_id` + 4). Measurement only.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers || exit 2
export LC_ALL=C
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/ded102ca-3bed-47b2-9edf-8814adf76fbc/scratchpad
O=$E/automationmodetool.frames.txt
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m46-amt.bin --script 'break 0x1804afa70; continue; where; x 0x6bc40 96; x 0x27ff440 1024' > $S/frames-debug.txt 2>&1; echo "debug exit=$?"
{
  echo "# t6-frames.sh: the refused kevent_id's caller chain (debugger at the svc, lldb symbols)."
  echo "## debugger"
  cat $S/frames-debug.txt
  echo "## label at 0x6bc40 (the queue's dq_label pointer, read at id+0x48 in entry.txt)"
  grep -a '^0x6bc40:' $S/frames-debug.txt | cut -d: -f2 | xxd -r -p | tr '\000' '\n' | head -1
  echo "## frame-pointer chain from x29 = 0x27ff440"
  grep -a '^0x27ff440:' $S/frames-debug.txt | python3 -c '
import sys
addr, hexs = sys.stdin.read().strip().split(":", 1)
b = bytes(int(x, 16) for x in hexs.split())
base = int(addr, 16)
fp = base
print("lr0 (x30 at the svc) 0x180359a64")
while base <= fp <= base + len(b) - 16:
    o = fp - base
    nfp = int.from_bytes(b[o:o+8], "little"); lr = int.from_bytes(b[o+8:o+16], "little")
    print(f"fp {fp:#x} next_fp {nfp:#x} lr {lr:#x} stripped {lr & 0xffffffffff:#x}")
    if nfp <= fp: break
    fp = nfp
' > $S/frames-chain.txt
  cat $S/frames-chain.txt
  echo "## lldb (target /usr/bin/automationmodetool, not running: the host shared cache's unslid addresses)"
  set --
  for a in $(awk '/^lr0/{print $NF} /^fp /{print $NF}' $S/frames-chain.txt); do set -- "$@" -o "image lookup -a $a"; done
  lldb --batch -o 'target create /usr/bin/automationmodetool' "$@" -o 'disassemble -s 0x180359a30 -e 0x180359a68' 2>&1 | grep -a -v '^Current executable'
} > $O 2>&1; echo "frames exit=$?"
cat $O
