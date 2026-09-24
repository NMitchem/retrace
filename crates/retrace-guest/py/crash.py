# M39 rung-8 fixture: a real script with modest stdlib use, ending in a ctypes bad-pointer deref.
#
# The bad address is COMPUTED from the data file (base + offset, parsed from hex strings), never a
# literal in this file, so the reverse-debug question "who stored this pointer?" has a real answer:
# the store inside _ctypes's cast() that writes the computed value into the Pointer object's
# buffer. The script reveals that buffer's address (ctypes.addressof(p)) on stdout before the
# deref, the M6 marker convention: tests DISCOVER the cell from the recording, never hardcode it.
#
# 0x4000_DEAD_0000 has bit 46 set (L1 index 0x400, never mapped, < 2^47) — the same FAR crashy.c
# uses — so the deref is a stage-1 EL0 data abort with FAR == the computed target.
#
# Import order is ctypes FIRST on purpose: the t0 measurements (spec companion) showed the one
# wall before the marker is `import ctypes` itself (libffi's mach_vm_remap), and everything the
# other imports need already works; keeping ctypes first keeps the walk's first stop where the
# probe measured it.
import ctypes
import json
import os
import sys

here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, "crash.json")) as f:
    cfg = json.load(f)

table = {}
for row in cfg["rows"]:
    table[row["name"]] = int(row["base"], 16) + int(row["offset"], 16)

target = table[cfg["target"]]
p = ctypes.cast(target, ctypes.POINTER(ctypes.c_long))

sys.stdout.write(f"CRASHPY cell={ctypes.addressof(p):#x} target={target:#x} rows={len(table)}\n")
sys.stdout.flush()

print(p[0])  # stage-1 fault at `target`; never returns
sys.stdout.write("UNREACHED\n")
