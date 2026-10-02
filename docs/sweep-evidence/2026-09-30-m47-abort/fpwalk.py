# M47 t0: walk the x29 frame-pointer chain in a `retrace debug` `x` dump of the guest stack.
# Usage: fpwalk.py <dump-file> <x29> <x30>   (prints each return address, PAC bits stripped)
import re, sys
text = open(sys.argv[1], errors='replace').read()
mem = {}
for m in re.finditer(r'^(0x[0-9a-f]+): ((?:[0-9a-f]{2} ?)+)$', text, re.M):
    base = int(m.group(1), 16)
    for i, b in enumerate(m.group(2).split()):
        mem[base + i] = int(b, 16)
def u64(a):
    return int.from_bytes(bytes(mem[a + i] for i in range(8)), 'little') if all(a + i in mem for i in range(8)) else None
fp, lr = int(sys.argv[2], 16), int(sys.argv[3], 16)
print(f'lr  {lr & 0x0000_ffff_ffff_ffff:#x}')
for _ in range(40):
    nfp, nlr = u64(fp), u64(fp + 8)
    if nfp is None or nlr is None or nlr == 0: break
    print(f'ret {nlr & 0x0000_7fff_ffff_ffff:#x}   (frame {fp:#x})')
    if nfp <= fp: break
    fp = nfp
