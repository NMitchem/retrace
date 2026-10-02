#!/bin/bash
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
$S/td/target/release/td /private/tmp/claude-501/m47-node.bin 0 197 360 > $S/node-mmap.txt 2>&1
echo "td exit=$?"
python3 - "$S/node-mmap.txt" <<'EOF'
import re, sys
mm = jit = thr = 0
for line in open(sys.argv[1]):
    m = re.match(r'#(\d+) Syscall num=(\d+) args=\[([^\]]*)\]', line)
    if not m: continue
    num = int(m.group(2)); args = [int(x.strip(), 16) for x in m.group(3).split(',')]
    if num == 197:
        mm += 1
        if args[3] & 0x800: jit += 1; print("MAP_JIT:", line.strip()[:200])
    if num == 360: thr += 1
print(f"mmap(197) events={mm} with MAP_JIT(0x800)={jit} bsdthread_create(360)={thr}")
EOF
