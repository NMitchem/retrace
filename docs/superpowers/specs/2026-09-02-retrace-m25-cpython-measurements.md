# M25-cpython — measurements (t0)

**2026-09-02.** Measured on `m24-restoreaudit` at `81d32ef` (which contains `main` `67384d8` plus
M21 and M24 t1), Homebrew `python@3.14` 3.14.6, macOS 26. Every number below came from running
the CLI; nothing is inferred from reading code alone unless marked so.

## Why this document exists

The 2026-07-05 design spec names one headline: reverse-debugging **real CPython**. Twenty-four
milestones later, no spec, plan, test or status-log entry had ever pointed retrace at `python3`
(verified by a whole-tree grep; the only mentions are the vision spec itself and one footnote in
M2-va47). M22's lesson applies verbatim: *a wall that every instance of a class hits identically
deserves one probe before it is believed.* The belief here was that interpreters were far away.
Nobody had measured it. This is the probe.

## Method

```sh
PY=$(realpath /opt/homebrew/bin/python3)        # the file on PATH
REAL=/opt/homebrew/Cellar/python@3.14/3.14.6/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
cargo run -q -p retrace -- record-dyn "$PY"   -o py.bin     -- -c 'print(1)'
cargo run -q -p retrace -- record-dyn "$REAL" -o pyreal.bin -- -c 'print(1)'
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn "$REAL" -o pyreal2.bin -- -c 'print(1)'
cargo run -q -p retrace -- replay <trace>
```

Each invocation was bounded with `perl -e 'alarm N; exec @ARGV'` (macOS has no `timeout(1)`).
None hit its bound.

For finding 2, exactly one source line was changed in a scratch build and reverted afterwards
(`git checkout -- crates/retrace-box/src/lib.rs`; tree verified clean; CLI rebuilt from the
reverted source and checked to contain no probe string). **Nothing from this document is
implemented.**

## Finding 0 — `python3` on PATH is a launcher, and exec-in-place is unmodelled

`/opt/homebrew/bin/python3` resolves to
`…/Python.framework/Versions/3.14/bin/python3.14`, a 2-dylib arm64 binary
(`Python` framework + `libSystem`) built from CPython's `Mac/Tools/pythonw.c`: it
`posix_spawn`s (syscall **244**, attr `POSIX_SPAWN_SETEXEC`) the real interpreter at
`Resources/Python.app/Contents/MacOS/Python` so the process gets a bundle identity.

Under retrace the launcher's 244 goes through the generic forward arm (it is in no table:
`grep -n 'posix_spawn\|\b244\b' crates/retrace-{core,box,arch}/src` is empty), returns an error
to the guest instead of replacing the image, and the launcher prints

```
python3.14: posix_spawn: …/Python.app/Contents/MacOS/Python: Undefined error: 0
```

(`err(1, …)` with `errno` untouched — libc's `posix_spawn` returns its error and does not set
`errno`), then exits 1.

**That trace records and replays byte-identically**: 70,491,550 bytes, replay exit 1 = the guest's
exit, stdout `cmp`-identical, `fall-throughs: 5` on both sides. So the launcher is not a wall in
retrace's determinism; it is a *capability* gap — **exec-in-place is not modelled**. The vision
scoped follow-fork out of v1; it did not name exec. Apple's `/usr/bin/git` and `/usr/bin/python3`
are `xcrun`-style shims with the same shape. Until exec is modelled, the headline command a user
types cannot be the recorded guest; the real binary can.

**Not measured:** what the forwarded 244 actually did on the host (the returned error code was
not captured — only that the image was not replaced and the launcher took its error path).

## Finding 1 — the real interpreter dies on one instruction, and it is one SCTLR bit

Pointing `record-dyn` at `Resources/Python.app/Contents/MacOS/Python` (33,568 bytes, arm64,
links CoreFoundation + the `Python` framework + libSystem):

```
RECORD ERROR: non-syscall exit: MSR/MRS/sysreg trap (EC=0x18 ISS=0x12dc68 FSC=0x28) far/ipa=0x0 (UNMAPPED) pc=0x4404 elr=0x1804fb070
```

with `x0=0x700c20000 x2=0x7f80 x3=0x700c20040`, backtrace through `0xa00bbd324 …` (the `Python`
framework's mapping) and `ret 0x180133e00`. Trace file 23,790,407 bytes at the point of death.

`pc=0x4404` is retrace's own EL1 vector slot 8 (lower-EL synchronous) + 4, i.e. the box reporting
what **ESR_EL1** said; `elr=0x1804fb070` is the faulting EL0 instruction, in libsystem_platform's
`_platform_memset`. Decoding `ISS=0x12dc68` for EC 0x18 (bits: `[0]` direction, `[4:1]` CRm,
`[9:5]` Rt, `[13:10]` CRn, `[16:14]` Op1, `[19:17]` Op2, `[21:20]` Op0):

| field | value |
|---|---|
| direction | 0 (write / SYS) |
| Op0, Op1, CRn, CRm, Op2 | 1, 3, 7, 4, 1 |
| Rt | 3 |

`SYS #3, C7, C4, #1, Xt` is **`DC ZVA, Xt`** — data-cache zero by VA — here `dc zva, x3` on a
32,640-byte (`0x7f80`) zero fill for CPython. An EL0 `DC ZVA` is trapped to EL1 with EC 0x18 when
`SCTLR_EL1.DZE == 0`. retrace's SCTLR is built by one derivation:

```rust
// crates/retrace-box/src/lib.rs:185
const SCTLR_MMU_ON_BASE: u64 = 0x30d0_0800 | 1 | 4 | 0x1000;
```

Bit 14 (`DZE`, `0x4000`) is clear. So are bit 15 (`UCT`, EL0 `CTR_EL0` reads) and bit 26 (`UCI`,
EL0 `DC CVAU`/`IC IVAU`) — the two a JIT's `sys_icache_invalidate` needs. `run()`'s only
`Ec::SysReg` arm is `try_emulate_timebase` (`lib.rs:1027`), so anything else in that class is a
non-syscall exit. The fix has the M2-tbi / M2-cpuid shape: one constant, one bit, below the trace,
deterministic on both sides by construction (symmetry rule 2).

**Why nothing hit it in 24 milestones (inferred, not measured):** Apple's memset uses `DC ZVA`
only above a size threshold; the Rust guests zero through `calloc`/`alloc_zeroed`, jq and the
54 Apple binaries never zero a large buffer through memset. CPython's allocator does, at startup.

## Finding 2 — with DZE set, CPython's core initialization records AND replays bit-for-bit

Scratch edit (reverted): `… | 0x1000 | 0x4000;`. Same command. Record ran to the guest's own
exit 1; trace **81,906,665 bytes**; `RETRACE_TRACE=1` log 646 lines. stdout:

```
Fatal Python error: Failed to import encodings module
Python runtime state: core initialized
Traceback (most recent call last):
  File "<frozen importlib._bootstrap>", line 1371, in _find_and_load
  …
  File "<frozen importlib._bootstrap_external>", line 1412, in _fill_cache
OSError: [Errno 22] Invalid argument: '/opt/homebrew/Cellar/python@3.14/3.14.6/Frameworks/Python.framework/Versions/3.14/lib/python3.14'
```

**Replay of that trace: exit 1, stdout `cmp`-identical, `fall-throughs: 5` on both sides, no
divergence.** That is the first time an interpreter's runtime has been through the oracle. It
reached "core initialized", which means the `Python` framework dylib mapped and relocated, the
allocator, the GIL/thread state, frozen importlib and the bytecode interpreter all ran on both
sides identically.

The record's `[retrace]` lines, in full (no fail-loud path fired):

```
[retrace warn] dyld __mac_syscall(Sandbox) synthesized as success/unsandboxed (…)     ×2
[retrace] forwarding mach_msg2 host_info (msgh_id 200) to host (decided allowlist)
[retrace] forwarding mach_msg2 host_get_clock_service (msgh_id 206) to host (decided allowlist)
[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
[retrace] forwarding mach_msg2 task_info (msgh_id 3405) to host (decided allowlist)
[retrace] forwarding mach_msg2 host_get_special_port (msgh_id 412) to host (decided allowlist)
[retrace] refusing mach_msg2 message-queue send (msgh_id 0x400000cf dest 0x1403 send_size 248): the box hosts no message-queue receivers
[retrace] fall-throughs: 5
```

## Finding 3 — the next wall is `getdirentries64` with an untranslated fd (the M10 class, silent)

The last traps before the Python error:

```
[trap] num=346 (0x15a) pc=0x1804b9980 args=[0x4,0x27fda80,0x3ff800,…]      # fstatfs64(fd=4)
[trap] num=344 (0x158) pc=0x1804af9c4 args=[0x4,0x7ad20,0x2000,0x700c05cb8,…]  # getdirentries64(fd=4, buf, 0x2000, &basep)
[trap] num=399 (0x18f) …                                                       # close_nocancel
[trap] num=4  … write(2, "Fatal Python error: ", 0x14)
```

`os.listdir` → `opendir`/`readdir` → `getdirentries64` on the directory the guest just opened as
guest fd 4. `retrace_arch::fd_operands` (`crates/retrace-arch/src/lib.rs:97-111`) lists neither
**344** nor **346**, so `translate_fds` left `x0 = 4` alone and the host kernel serviced the call on
**retrace's own fd 4** — not a directory — and XNU's `getdirentries` returns **EINVAL** for a
non-`VDIR` vnode. That is the value Python printed.

Two things about the shape, both measured against the code rather than inferred:

- It is the M10 class exactly ("the guest's fds were retrace's host fds"), one table entry wide
  per syscall.
- It is **silent**. An fd-taking syscall absent from `fd_operands` is forwarded with the guest's
  number unchanged; nothing asserts. M10's own memory note recorded the same property for
  `F_DUPFD`. A guest that opens a directory whose number happens to be a valid retrace fd of the
  right type would record something *self-consistent and wrong*, which is the one failure class
  the oracle cannot see.

## Syscall census of the CPython core-init run

Distinct syscall numbers issued in the DZE run (positive = BSD, negative = mach trap), by count:

```
 47 -14    32 116    29 338    19 -15    18 6     18 4     16 398    16 202    14 92     13 339
 13 -47    12 5      12 396    12 197    11 -12   10 73     9 399     9 327     7 54      7 483
  6 381     6 38      6 169     5 74      5 -19     4 75     4 336     4 33      4 294     4 25
  4 220     4 -10     3 500     3 41      3 20      3 194     3 -29     3 -28     3 -26     3 -24
  2 58      2 133     2 153     2 344     2 346     2 347     2 372     2 463     2 550     2 2147483648
  1 -89     1 -70     1 -50     1 -27     1 1       1 3       1 24      1 43      1 97      1 98
  1 170     1 228     1 266     1 286     1 366     1 406     1 427     1 470
```

69 distinct numbers. Of these, the fd-taking ones **not** in `fd_operands` today, by inspection of
the table: 344 (`getdirentries64`), 346 (`fstatfs64`), and possibly 228, 406, 427 (unidentified —
identify before classifying). 97/98/133 (`socket`/`connect`/`sendto`) appear once each and did not
fail; they are presumably libnotify/libinfo startup traffic and 98/133 are already in the table.

## What was not measured, stated so nobody cites it as measured

- Anything past wall 2. `encodings` is the first stdlib import; `-c 'print(1)'` will go on to
  import `site`, read `.pyc` files (`open`/`fstat`/`read`/`mmap`), and possibly touch `dup2`
  (`csh`/`tcsh` hit M10's fail-loud `dup2` in the sysbin sweep). Each is a wall or not; none is
  known.
- Whether `UCI`/`UCT` matter for this build (Homebrew's 3.14 may or may not enable the
  experimental JIT). Only `DZE` was observed.
- `node` and `git` as guests. `node` is a JIT and will need W^X promotion and cache maintenance
  at scale; not probed.
- Reverse execution / seeks over the CPython trace. Only `record` and `replay` were run. The
  `debug` path and `from_checkpoint` were not exercised on it.
- Record time and replay time (not captured; both completed well inside a 270 s bound).

## What this changes

The headline demo is not a research problem. It is a wall-chain of the kind this repo clears in
days, and the first two walls are one bit and two table entries. The honest framing for M25 is
therefore the M2 pattern — clear each measured wall, re-measure, park at the first one that is
not small — with `cpython_e2e` parked `#[ignore]`d at wall 1 from the milestone's first commit so
the discipline holds from day one.
