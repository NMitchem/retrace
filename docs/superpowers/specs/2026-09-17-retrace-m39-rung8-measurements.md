# M39-rung8 — measurements (t0)

**Taken 2026-09-17 on `main` at `832a1cb` (the M38-owed merge), before the design spec was
finalised.** Nothing in this document is implemented. Every figure below was produced by a
command in §Method on this machine; anything not listed under a Finding is under "What was not
measured" and must not be cited as measured.

## Why this document exists

Approach A of the M39 brainstorm (operator, 2026-09-17): anchor the spec on one measurement
before predicting walls, because M19, M20, M25 and M32 each asserted a mechanism nobody had
measured and each was wrong in a way that mattered (M25's account of its own wall was wrong three
ways). The question this probe answers is narrow: **on today's tree, how far does a script file
with modest stdlib use and a `ctypes` bad-pointer deref get, and what is the first wall?**

## Method

```sh
REAL=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
S=<scratch>/m39probe          # crash.py + crash.json, the fixture drafted for §3a of the spec
cargo run -q -p retrace -- record-dyn "$REAL" -o "$S/crash.bin"  -- "$S/crash.py"          # rec1
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn "$REAL" -o "$S/crash2.bin" -- "$S/crash.py"   # rec2
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn "$REAL" -o "$S/late.bin"   -- "$S/crash_late.py" # late
cargo run -q -p retrace -- record-dyn "$REAL" -o "$S/late2.bin" -- "$S/crash_late.py"       # late2
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn "$REAL" -o "$S/print1b.bin" -- -c 'print(1)'  # rung 7, for scale
```

Each bounded with `perl -e 'alarm N; exec @ARGV'`; none hit its bound. `crash_late.py` is
`crash.py` with `import ctypes` moved to just before the `cast` and a `JSONOK` marker written after
the JSON work — same work, different import order, so the two runs bisect the wall. Native runs of
both scripts (no retrace) printed their markers and exited 139.

Static facts were taken with `otool -L`, `dyld_info -imports`/`-segments`, `file`, `stat`, and
`lldb -b -o 'image lookup -a …'` against `/usr/bin/true` (no process; the cache's unslid
addresses, which are the guest's — the box maps the cache at its unslid base).

## Finding 1 — the script gets everything but `ctypes`: the first wall is `mach_vm_remap` (4813)

Both `rec1` and `rec2` exit 4 with

```
RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 4813 dest 0x203 (guest task port Some(515)) send_size 92
```

after **979** dispatched traps (rung 7's whole `print(1)` run is **834**). The `late` run reaches
the same line after **1069** traps, and at trap 1139 of its log the guest has written
`JSONOK rows=3 target=0x4000dead0000` to fd 1 (`write(1, 0x7008ac000, 0x24)`, echoed by the
`[fd1 (console 1)]` line the trace flag prints). So on today's tree, with no change:

- the script file is located, opened and compiled from disk;
- `import json`, `import os`, `import sys` complete — `_json.cpython-314-darwin.so` is a runtime
  `dlopen` of a Homebrew arm64 dylib, and it loads;
- `crash.json` is opened, read and parsed; the table is built; `target` is computed as
  `0x4000dead0000` from two hex strings — the value the fixture needs;
- `sys.stdout.write` + `flush` reach the console mirror.

**The one wall between rung 7 and the marker is `import ctypes`.** Everything after it (the
`cast`, the marker, the deref) is unmeasured under retrace because nothing gets there.

## Finding 2 — the wall is libffi's Apple trampoline table, reached from `import _ctypes`

`0x1804adc34` is `libsystem_kernel.dylib`\`mach_msg2_trap + 8` — the trap stub, not the caller
(the `[trap] pc` is always the stub). The caller was established three ways:

1. **The message.** `send_size` 92 is exactly `mach_vm_remap`'s request: header 24 + body 4 + one
   port descriptor 12 (`src_task`) + NDR 8 + `target_address` 8 + `size` 8 + `mask` 8 + `flags` 4
   + `src_address` 8 + `copy` 4 + `inheritance` 4. Decoded from the `rec2` hexdump:

   | field | value | meaning |
   |---|---|---|
   | `msgh_bits` | `0x80001513` | complex; COPY_SEND remote, MAKE_SEND_ONCE local |
   | `remote` / `local` | `0x203` / `0x1403` | the guest's task port; a reply port |
   | descriptor | port `0x203`, disposition `0x13` | `src_task` = the same task |
   | `target_address` | `0x0a017fc000` | inside the region libffi `vm_allocate`d (below) |
   | `size` | `0x8000` | two 16 KiB pages |
   | `mask` | `0` | |
   | `flags` | `0x4000` | `VM_FLAGS_OVERWRITE`, `VM_FLAGS_FIXED` (0) |
   | `src_address` | `0x0a0183c000` | see 3 |
   | `copy` | `0` (FALSE) | **shared** mapping, not a copy |
   | `inheritance` | `0` | `VM_INHERIT_SHARE` |

   `rcv_size` 60 = header 24 + NDR 8 + `RetCode` 4 + `target_address` 8 + `cur_protection` 4 +
   `max_protection` 4 + an 8-byte trailer.

2. **The importer.** `dyld_info -imports /usr/lib/libffi.dylib` lists `_vm_allocate`,
   `_vm_deallocate`, `_vm_remap`, `_dlopen`, `_dlsym`; libsyscall's 64-bit `vm_remap` is
   `mach_vm_remap` (4813). `_ctypes.cpython-314-darwin.so` imports `_ffi_closure_alloc`,
   `_ffi_prep_closure_loc`, `_ffi_call`, `_ffi_prep_cif` from it; `_ctypes` links only
   `/usr/lib/libffi.dylib` (shared cache) and libSystem — **no Homebrew libffi**.

3. **The sequence in the trace** (`rec2` lines 1154–1182, identical in `late` 1269–1297):
   `_kernelrpc_mach_vm_allocate_trap(size 0xc000, ANYWHERE)` → dyld's `__DATA_CONST`
   protect/unprotect pair → `open` of a path whose pointer (`0x194c29ff6`) lies inside
   **libffi's own `__TEXT`** (a libffi string constant) → the full runtime-dylib load shape
   (`mmap` header, `fstat64`, two `pread`s, `fcntl` F_ADDFILESIGS_RETURN / F_CHECK_LV,
   `vm_allocate 0x10000`, **`mmap(0xa0183c000, 0x8000, RX, MAP_FIXED, fd, off 0xc000)`**,
   `mmap(…+0x8000, 0x8000, R, MAP_FIXED, off 0x14000)`, `munmap`, `close`) → the remap with
   `src_address == 0xa0183c000`, i.e. **the `__TEXT` that was just mapped**.

   That file is **`/usr/lib/libffi-trampolines.dylib`**: 100704 = `0x18960` bytes (the header
   `mmap`'s length), a **universal binary (x86_64 + arm64e)** whose arm64e slice sits at file
   offset `0xc000` — which is why the segment maps start there. Apple's libffi keeps its
   trampoline page in a separate dylib and `vm_remap`s it, shared, to the page above the writable
   config page it allocated, so the trampoline's PC-relative load reaches its config.

**When.** Natively with `DYLD_PRINT_LIBRARIES=1`, `libffi-trampolines.dylib` loads between
`import _ctypes` starting and returning — during the C module's init, before any line of
`ctypes/__init__.py` runs and before any `cast`. (The historical
`CFUNCTYPE(c_int)(lambda: None)` workaround line is **not** in this install's `ctypes/__init__.py`;
whatever allocates the first closure does so inside `PyInit__ctypes`. Which call is not
measured and does not matter to the wall.) So this wall is on **every `import ctypes`** on this
OS, not on the fixture's use of it.

## Finding 3 — runtime `dlopen` works today, including a fat arm64e system dylib

Counting `mmap` calls with `PROT_READ|PROT_EXEC` and `MAP_FIXED|MAP_PRIVATE|0x40000` (dyld's
`__TEXT` segment map): rung 7's `print(1)` run has **1**; the `ctypes`-first run **3**; the
`ctypes`-last run **4**. The three beyond baseline are `_json…so` (Homebrew arm64),
`_ctypes…so` (Homebrew arm64) and `libffi-trampolines.dylib` (Apple, fat, arm64e slice at
`0xc000`) — each completed its load shape and returned to the guest. The fat header on the
`dlopen` path is therefore **not** a wall (M22's class, already cleared at load time, holds at
run time); `place_fixed`'s FIXED-exec-over-live-backing path handled all three.

## Finding 4 — an aborted record swallows the guest's console bytes (diagnostic, not a wall)

`late2` (no trace flag) exits 4 with **empty stdout** although the guest wrote `JSONOK`.
`crates/retrace/src/main.rs` writes the accumulated `s.stdout` only on `Ok(s)`; a `RECORD ERROR`
returns `Err` and exits first. A *completed* record — including a crash outcome, which is `Ok`
with `Outcome::Crash` — emits it before exiting (this is `crashy`'s path, and rung 7's), so the
gate's stdout assertion is sound. Noted so nobody reads an empty stdout on an aborted record as
"the guest never wrote".

## Syscall census of the walk past rung 7 (from `late`, traps 835–1297, by number)

`stat64` 338 ×54 (import-path probing dominates), `read` 3 ×18, `close` 6 ×16, `open` 5 ×16,
`mmap` 197 ×16, `fcntl` 92 ×15, `lstat64` 340 ×15, `fstat64` 339 ×14, `madvise` 75 ×11,
`lseek` 199 ×10, `munmap` 73 ×6, `pread` 153 ×6, `mach_vm_protect` trap −14 ×6,
`mach_vm_allocate` trap −10 ×5, `read_nocancel` 396 ×4, `mach_vm_deallocate` trap −12 ×4,
`mach_vm_map` trap −15 ×3, `mprotect` 74 ×2, `readlink` 58 ×2, `close_nocancel` 399 ×2,
`open_nocancel` 398 ×2, `write` 4 ×1, `fstatfs64` 346 ×1, `getdirentries64` 344 ×1 (the M25
row), `issetugid` 327 ×1, `geteuid` 25 ×1, `getuid` 24 ×1, `getrlimit` 194 ×1, `mach_msg2` −47
×1 (the remap). One message-queue `mach_msg2` send earlier in the run was refused (`0x400000cf`,
the M38 refusal, non-fatal). **No `arg_kinds` row was missing on this path**: no M33 fail-loud
fired. Of M38's owed set {461, 468, 464, 345, 374}, none was reached before the wall (the
`_nocancel` twins reached are 396/398/399, all rows that exist).

## What was not measured, stated so nobody cites it as measured

- **Anything after `import ctypes`.** The `cast`, `addressof`, the marker write, the deref, the
  `Event::Crash`, the DFSC, the thread tag, replay, and the reverse-continue — all unmeasured
  under retrace. Natively the deref faults at `FAR = 0x4000dead0000` with exit 139; that is the
  only fact about the crash this document carries.
- **What the real kernel returns for this remap** — `cur_protection`/`max_protection` in the reply
  and whether libffi checks them. The reply the model synthesises must be *correct*, not merely
  deterministic; the task that models 4813 owes this measurement (a freestanding C probe doing the
  same `vm_remap` on a `MAP_FIXED` text page, natively).
- **Whether `ffi_closure_free` / `vm_deallocate` of the alias is reached** before the crash
  (natively the script crashes first; under retrace unmeasured).
- **Which call inside `PyInit__ctypes` allocates the first closure.** Irrelevant to the wall.
- **The second wall.** There may be none; there may be several. The spec's ceiling is six.

## What this changes

The spec's §2 cites this document; its §3f designs the one measured wall (a stage-1 alias plus a
synthesised MIG reply, symmetry rule 1); its predicted walls are marked predicted. The fixture's
import order is kept as drafted (`ctypes` first) — the wall is the same either way, and the
`ctypes`-last variant is retained only here, as the bisection.
