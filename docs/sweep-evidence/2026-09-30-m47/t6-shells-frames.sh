#!/bin/bash
# M47 Task 6: who issues the refused fork and the wait4 (7) behind it, in /bin/csh's recording. Stops the
# debugger at the fork stub's svc (pc 0x1804b58fc, one before the trap pc 0x1804b5900) and, separately,
# at the wait4 stub's svc (pc 0x1804b5474, one before the trap pc 0x1804b5478), dumps registers and
# the stack, walks the frame-pointer chain from x29, strips each saved lr's PAC bits (low 40 bits
# kept; every address here is below 2^40), and symbolicates x30 and each lr with lldb against
# /bin/csh and the host's shared cache (not running: unslid addresses — the guest's csh sits at
# 0x100000000, where t6 read the string its open at landmark #255 names, "/dev/null").
# Measurement only (the M46 t6-frames.sh shape).
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
cd "$W" || exit 2
export LC_ALL=C
O=$E/csh.frames.txt
{
  echo "# t6-shells-frames.sh: /bin/csh's refused fork and the wait4 behind it (debugger at each svc, lldb symbols)."
  for stop in fork:0x1804b58fc wait4:0x1804b5474; do
    name=${stop%%:*}; pc=${stop#*:}
    echo "## $name: debugger at $pc"
    tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m47-csh.bin --script "break $pc; continue; where; regs" > $S/frames-$name-1.txt 2>&1
    cat $S/frames-$name-1.txt
    fp=$(printf '%#x' "$(grep -a -o 'x29=0x[0-9a-f]*' $S/frames-$name-1.txt | head -1 | cut -d= -f2)")
    lr0=$(grep -a -o 'x30=0x[0-9a-f]*' $S/frames-$name-1.txt | head -1 | cut -d= -f2)
    tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace debug /private/tmp/claude-501/m47-csh.bin --script "break $pc; continue; x $fp 1024" > $S/frames-$name-2.txt 2>&1
    echo "## $name: frame-pointer chain from x29 = $fp"
    grep -a "^$fp:" $S/frames-$name-2.txt | LR0=$lr0 python3 -c '
import os, sys
addr, hexs = sys.stdin.read().strip().split(":", 1)
b = bytes(int(x, 16) for x in hexs.split())
base = int(addr, 16)
fp = base
print("lr0 (x30 at the svc)", hex(int(os.environ["LR0"], 16) & 0xffffffffff))
while base <= fp <= base + len(b) - 16:
    o = fp - base
    nfp = int.from_bytes(b[o:o+8], "little"); lr = int.from_bytes(b[o+8:o+16], "little")
    print(f"fp {fp:#x} next_fp {nfp:#x} lr {lr:#x} stripped {lr & 0xffffffffff:#x}")
    if nfp <= fp: break
    fp = nfp
' > $S/frames-$name-chain.txt
    cat $S/frames-$name-chain.txt
    echo "## $name: lldb (target /bin/csh, not running)"
    set --
    for a in $(awk '/^lr0/{print $NF} /^fp /{print $NF}' $S/frames-$name-chain.txt); do set -- "$@" -o "image lookup -a $a"; done
    lldb --batch -o 'target create /bin/csh' "$@" 2>&1 | grep -a -v '^Current executable'
  done
} > $O 2>&1
echo "frames exit=$?"
