# M18 Stage 2b Task 1 — the `wqthread` entry contract, and the two trap numbers

Measured 2026-08-23 on branch `m18-workq-stage2b` at `60cea11` (Stage 2a merged; `REQTHREADS` still
the fail-loud `panic!`). Host: macOS **26.5.2**, build **25F84**, `xnu-12377.121.10~1/RELEASE_ARM64_T6041`,
Apple Silicon (`Darwin 25.5.0`) — captured with `sw_vers` and `uname -a`.

This document follows the conventions of its predecessor,
`docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md`: every claim is labelled
**measured** / **attributed** / **unmeasured**, every command that produced a number is quoted
verbatim, and anything that could not be verified is stated **in bold as unverified** rather than
quietly softened. The predecessor's discipline holds throughout — **the raw value is the
measurement, the name is a lead.**

It answers Task 1's six steps and, with them, the design spec's numbered open questions 1–4
(`docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-design.md`).

---

## 0. Method, and the one methodological risk that had to be closed first

Two independent instruments were used:

1. **Static disassembly of the on-disk dylibs** (`otool -tV`).
2. **Live observation of a real host process** under `lldb`, and a C host probe that reads its own
   pthread struct — the same instrument M14 used to measure `pthread + 0xe0` and `pthread + 0xf8`
   "4/4 by host probe."

**The risk:** retrace's guest does not execute the on-disk `/usr/lib/system/*.dylib` files. It
demand-pages the **dyld shared cache**
(`/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e`, per
`crates/retrace-box/src/cache.rs:191`). A disassembly of the on-disk file is only evidence about the
guest if the two carry identical text.

**That equivalence is measured, not assumed**, by four independent PC coincidences between the
Stage 2a raw traces and the on-disk files. `otool`'s addresses are dylib-relative (image base 0), so
each guest `pc` minus its image base must land on a known instruction:

| guest `pc` (Stage 2a trace) | image | image base implied | on-disk offset | instruction there |
|---|---|---|---|---|
| `0x1804adbb0` | libsystem_kernel | `0x1804ad000` | `0xbb0` | `ret` of `_semaphore_wait_trap` (the insn after its `svc`) |
| `0x1804adc34` | libsystem_kernel | `0x1804ad000` | `0xc34` | `ret` of `_mach_msg2_trap` (the insn after its `svc`) |
| `0x1804af9f0` | libsystem_kernel | `0x1804ad000` | `0x29f0` | `b.lo` of `___workq_kernreturn` (the insn after its `svc`) |
| `0x1804ecc08` / `0x1804ecc14` | libsystem_pthread | `0x1804eb000` | `0x1c08` / `0x1c14` | `_start_wqthread` / `_thread_start`, exactly `0xc` apart |

The last row is the strongest: the Stage 2a trace's `bsdthread_register` carries **both** entry
points and their measured difference is `0x1804ecc14 - 0x1804ecc08 = 0xc`, which is precisely the
gap between `_start_wqthread` and `_thread_start` in the on-disk `libsystem_pthread.dylib`. Three
distinct images-relative offsets in libsystem_kernel resolve to one consistent base.

```
$ grep -a 'num=366' .superpowers/sdd/2026-08-21-retrace-m18-workq-stage2a/task-4-raw-trace.err
[trap] num=366 (0x16e) pc=0x1804b3d2c args=[0x1804ecc14,0x1804ecc08,0x4000,0x27fc140,0x38,0xa0]
```
(identical in `task-4-raw-trace-rerun.err`.)

**MEASURED:** for the four call sites this document depends on, the on-disk dylibs and the
shared-cache copies the guest executes are the same text. Every disassembly claim below therefore
transfers to the guest.

**MEASURED, incidentally, and useful when reading any trace:** `RETRACE_TRACE=1`'s `pc=` for a
syscall/trap is the address of the instruction **after** the `svc #0x80`, i.e. the return address —
all four rows above are `svc + 4`.

**MEASURED:** `args[2] = 0x4000` in that `bsdthread_register` is `pthsize`; retrace already captures
it as `Box_::pthread_size` (`crates/retrace-box/src/lib.rs:3335`). `0x4000` = 16 KiB = one page on
this host (`vm_page_size = 0x4000`, printed by the host probe).

### The host probes

Four throwaway probes were built and run; none is committed (they live in this session's scratch
directory, in the spirit of `spikes/`). **§6 reproduces each one's source and lldb script verbatim
as they were on disk, so every number below can be regenerated** — the sources there are the actual
files, not retyped summaries of them.

- `wqprobe.c` — dumps `pthread_self()`, `TPIDRRO_EL0`, `sp`, `pthread_get_stackaddr_np`,
  `pthread_get_stacksize_np`, `mach_thread_self()` and the raw struct words at the offsets
  `__pthread_wqthread_setup` writes, for **three** thread kinds: `main`, a `pthread_create` thread,
  and a libdispatch worker.
- `dispatch_host.c` — a byte-identical copy of the actual guest,
  `crates/retrace-guest/c/dispatch_dyn.c`, run under `lldb`.
- `qosprobe.c` — five global queues at different QoS, to capture
  `(REQTHREADS request → wqthread entry registers)` pairs.
- `qos2.c` — the same, for `QOS_CLASS_DEFAULT` alone.

### The observation ledger

Because several counts below are "n/n", here is exactly where every live `start_wqthread` entry came
from. **Five lldb runs, 13 entries.**

| run | binary | lldb script | entries | registers read at entry |
|---|---|---|---|---|
| A | `wqprobe` | ad-hoc `-o` flags (§6) | 1 | `x0`–`x7`, `sp`, `pc`, `lr` |
| B | `dispatch_host` | `cmds.txt` | 1 | `x0`–`x5`, `sp`, `lr` |
| C | `qosprobe` | `cmds2.txt` | 5 | `x0`, `x2`, `x4` (+ `x0`, `x3` at `__workq_kernreturn`) |
| D | `qosprobe` | `cmds3.txt` | 5 | `x0`–`x7`, `sp`, `lr` |
| E | `qos2` | `cmds2.txt` | 1 | `x0`, `x2`, `x4` (+ `x0`, `x3` at `__workq_kernreturn`) |

So the denominators are: **7/7** for anything involving `sp`/`lr`/`x3`/`x5` (runs A, B, D);
**6/6** for `x6`/`x7` (runs A, D — run B's script does not read them); **13/13** for `x4`; and
**7 `(request → flags)` pairs** across **6 distinct queue configurations** (runs B, C, E).

---

## 1. The register contract at `wqthread` entry

### 1a. `_start_wqthread` sets up nothing

```sh
otool -tV /usr/lib/system/libsystem_pthread.dylib | sed -n '/^_start_wqthread:/,/^_[a-z]/p' | head -60
```
```
_start_wqthread:
0000000000001c08	stp	xzr, xzr, [sp, #-0x10]!
0000000000001c0c	bl	__pthread_wqthread
0000000000001c10	brk	#0x1
```

**MEASURED.** Three instructions. It pushes a zeroed 16-byte frame terminator and tail-calls
`__pthread_wqthread`; the `brk #0x1` is unreachable because `__pthread_wqthread` never returns.
It moves **no** register, so **the kernel's entry contract is exactly `__pthread_wqthread`'s argument
registers**. Two consequences retrace must respect: the guest's `SP` must already be a valid,
writable stack pointer at entry (that `stp` writes 16 bytes at `SP-0x10`), and no register is
normalised on the way in.

### 1b. What `__pthread_wqthread` reads before its first call

```sh
otool -tV /usr/lib/system/libsystem_pthread.dylib | sed -n '/^__pthread_wqthread:/,/^_[a-z]/p' | head -200
```
```
__pthread_wqthread:
0000000000002d9c	pacibsp
0000000000002da0	stp	x22, x21, [sp, #-0x30]!
0000000000002da4	stp	x20, x19, [sp, #0x10]
0000000000002da8	stp	x29, x30, [sp, #0x20]
0000000000002dac	add	x29, sp, #0x20
0000000000002db0	mov	x20, x5
0000000000002db4	mov	x22, x4
0000000000002db8	mov	x21, x3
0000000000002dbc	mov	x19, x0
0000000000002dc0	tbnz	w4, #0x11, 0x2dd0
0000000000002dc4	mov	x0, x19
0000000000002dc8	mov	x3, x22
0000000000002dcc	bl	__pthread_wqthread_setup
```

**MEASURED, and this is the M14-shaped surprise the brief warned to look for:** between entry and
the first call, the only registers written are `x19..x22`, `x0`, `x3`, `x29`, `x30` and `SP`.
**`x1` and `x2` are never touched — so they are passed straight through into
`__pthread_wqthread_setup` as its own `x1`/`x2`, and that is the only place they are consumed.**
Reading the ABI of `__pthread_wqthread` alone would have concluded `x1`/`x2` were unused; they are
load-bearing, and `__pthread_wqthread_setup`'s body (§1c, §4) is where they land.

`x3` is saved into `x21` and is then only read on two branches that the measured flags do not take
(§1d). `x6`, `x7` and above are never read anywhere on this path.

### 1c. Where each register ends up

Live capture, first instruction of `start_wqthread`, on the real guest program compiled for the host
(`dispatch_host.c` == `crates/retrace-guest/c/dispatch_dyn.c`) — **run B**, quoted exactly as `lldb`
printed it. `cmds.txt` reads `x0`–`x5`, `sp`, `lr` and no more, so `x6`/`x7` are absent here by
construction:

```sh
lldb -b -s cmds.txt ./dispatch_host        # cmds.txt in §6
```
```
      x0 = 0x000000016fe87000
      x1 = 0x0000000000000e03
      x2 = 0x000000016fe04000
      x3 = 0x0000000000000000
      x4 = 0x0000000000244005
      x5 = 0x0000000000000000
      sp = 0x000000016fe87000
      lr = 0x0000000000000000
```

`x6`/`x7` come from the runs whose script does read them — **run D**, first of its five entries,
again verbatim:

```sh
lldb -b -s cmds3.txt ./qosprobe             # cmds3.txt in §6
```
```
      x0 = 0x000000016fe87000
      x1 = 0x0000000000001b03
      x2 = 0x000000016fe04000
      x3 = 0x0000000000000000
      x4 = 0x0000000000244005
      x5 = 0x0000000000000000
      x6 = 0x0000000000000000
      x7 = 0x0000000000000000
      sp = 0x000000016fe87000
      lr = 0x0000000000000000
```

**§1 — the register contract table. Every row MEASURED.**

| reg | value observed | first instruction that READS it | what it is used for |
|---|---|---|---|
| `x0` | `0x16fe87000` | `0x2dbc mov x19, x0` | **the pthread struct pointer.** Everything writes through `x19`: `strb wzr,[x19,#0xa4]`, `str x9,[x19,#0x100]`, `stp x8,x0,[x19,#0x90]`, `ldrsw x8,[x19,#0xac]`, and `mov x0,x19; bl __pthread_wqthread_exit`. |
| `x1` | `0xe03` | `0x302c str w1, [x0, #0xf8]` (inside `__pthread_wqthread_setup`) | **the thread's mach port name**, stored to `pthread + 0xf8` — the same offset M14 measured for a `bsdthread_create` child. Checked later in the same routine (at `0x30c8`, ~40 instructions on, past `___thread_selfid`, the unfair lock and the thread-list link); `0` or `-1` is fatal (§4). |
| `x2` | `0x16fe04000` | `0x2fe0 sub x10, x2, x10` / `0x2ff0 stp x0, x2, [x0, #0xb0]` (inside setup) | **the low end of the stack region.** Stored to `pthread + 0xb8`; the guard-page base `x2 - vm_page_size` is stored to `pthread + 0xc0`. See §2. |
| `x3` | `0x0` | `0x2db8 mov x21, x3`, consumed only at `0x2e94 sub x0, x21, #0x8` (flags bit 22) and `0x2edc str x21,[x22,#0x98]!` (flags bit 19) | **the kevent / workloop event list.** On the measured flags neither branch is taken, so **`x3` is never read on the path a plain `dispatch_async` worker takes.** Observed `0` in 7/7 entries that read it (runs A, B, D). |
| `x4` | `0x244005` | `0x2dc0 tbnz w4, #0x11` | **the flags word.** It selects fresh-vs-reused, asserts the kernel set the TSD base, and *encodes the worker's QoS*. Decoded in §1d. |
| `x5` | `0x0` | `0x2db0 mov x20, x5`, tested at `0x2e44 cmn w20, #0x1` | **the event count**, and the **kill sentinel**: `w5 == -1` branches to `__pthread_wqthread_exit`. Otherwise stored into the event block on the bits-22/19 paths only. Observed `0` in 7/7 entries that read it (runs A, B, D). |
| `x6`, `x7` | `0x0` | — | **never read.** Observed `0` in 6/6 entries that read them (runs A, D). |
| `SP` | `0x16fe87000` | `0x1c08 stp xzr, xzr, [sp, #-0x10]!` | **MEASURED `SP == x0` exactly, 7/7 entries that read it** (runs A, B, D) across three programs and five QoS classes. The stack top *is* the struct base. |
| `LR` | `0x0` | — | **MEASURED `0`, 7/7 entries that read it** (runs A, B, D). `_start_wqthread`'s `brk #0x1` is the only return target and is unreachable. |

Answering the design spec's **open question 1** (does the entry contract read a kevent list, and does
the fresh path accept it empty): **yes, `x3`/`x5` are that list and its count, and no — on the fresh
non-kevent path they are not read at all.** `x3 = x5 = 0` is what the kernel itself supplies.

### 1d. The flags word `x4`, decoded bit by bit

Every bit below is **MEASURED** — either a `tbnz`/`tbz` in the disassembly, or an observed value.

| bit | mask | disassembly that tests it | meaning |
|---|---|---|---|
| 17 | `0x20000` | `0x2dc0 tbnz w4, #0x11, 0x2dd0` | **SET ⇒ skip `__pthread_wqthread_setup`** (thread reused, struct already initialised). **CLEAR ⇒ call it** (fresh thread). This is §4's whole verdict. |
| 21 | `0x200000` | `0x3048 tbz w3, #0x15, 0x3114` (inside setup, where `x3` is the flags) | **must be SET on the fresh path.** Clear ⇒ `brk #0xb001` with `"BUG IN LIBPTHREAD: thread_set_tsd_base() wasn't called by the kernel"`. |
| 23 | `0x800000` | `0x2dd0 tbnz w22, #0x17, 0x2dfc` | SET ⇒ `pthread+0xa4 = 1` and the priority word is forced to the constant `0x040008ff`. This is a *distinct* thread role (see §5, note 3) — retrace must **not** set it for a worker. |
| 22 | `0x400000` | `0x2e4c tbnz w22, #0x16, 0x2e94` | kevent-delivery path; consumes `x3`/`x5`. Clear in all observations. |
| 20 | `0x100000` | `0x2de0 tbnz w22, #0x14, 0x2e10` | `priority |= 0x2000000`. Clear in all observations. |
| 19 | `0x80000` | `0x2e50 tbnz w22, #0x13, 0x2ed0` | workloop path; consumes `x3`/`x5`. Also feeds `priority` bit 24 via `0x2dd8 lsl w8, w22, #5; and w8, w8, #0x1000000`. Clear in all observations. |
| 16 | `0x10000` | `0x2de4 lsr w9, w22, #16; 0x2de8 bfi w8, w9, #31, #1` | copied into `priority` bit 31 (the request's `0x80000000`). Set only in the overcommit observation. |
| 15 | `0x8000` | `0x2df0 tbz w22, #0xf, 0x2f50` | if **both** 14 and 15 are clear ⇒ `__pthread_wqthread.cold.1`, `"BUG IN LIBPTHREAD: Missing priority"`, `brk #0xb001`. |
| 14 | `0x4000` | `0x2dec tbnz w22, #0xe, 0x2e18` | **the QoS-index encoding.** SET in every observation. See the arithmetic below. |
| 18 | `0x40000` | *(nothing on this path tests it)* | SET in **13/13** entries, **read by nothing `__pthread_wqthread` executes.** |
| 7:0 | `0xff` | `0x2e18`–`0x2e38` | the **QoS index**. Must be in `1..=6` (see the guard below). |

The bit-14 arithmetic, verbatim, because Task 2 must reproduce its inverse:

```
0000000000002e18	and	w9, w22, #0xff
0000000000002e1c	sub	w9, w9, #0x1
0000000000002e20	add	w10, w22, #0x7
0000000000002e24	mov	w11, #0x1
0000000000002e28	lsl	w10, w11, w10
0000000000002e2c	add	w10, w8, w10
0000000000002e30	orr	w10, w10, #0xff
0000000000002e34	cmp	w9, #0x5
0000000000002e38	csel	w8, w8, w10, hi
0000000000002e3c	mov	w9, w8
0000000000002e40	str	x9, [x19, #0x100]
```

i.e. `priority = (1 << ((flags + 7) & 31)) | 0xff`, applied **only when `1 <= (flags & 0xff) <= 6`**
(the `csel ... hi` discards it otherwise, leaving priority `0`). That closed form is exact **only with
flags bits 16/19/20 clear** — `0x2e2c add w10, w8, w10` accumulates into `w8`, which those three bits
have already contributed to (bit 16 -> prio 31, bit 19 -> prio 24, bit 20 -> prio 25). All three are
clear in the fresh-worker flags this document recommends. The result is stored at
`pthread + 0x100` and, on the plain worker path, passed to the dispatch callback in `x0`:

```
0000000000002e54	lsl	w9, w22, #3
0000000000002e58	and	w9, w9, #0x8000000
0000000000002e5c	orr	w0, w8, w9
0000000000002e60	adrp	x8, 50 ; 0x34000
0000000000002e64	ldr	x8, [x8, #0x20]
0000000000002e68	stp	x8, x0, [x19, #0x90]
0000000000002e6c	str	wzr, [x19, #0xa0]
0000000000002e70	adrp	x9, 46 ; 0x30000
0000000000002e74	ldrb	w9, [x9, #0x28]
0000000000002e78	cmp	w9, #0x1
0000000000002e7c	b.ne	0x2f48
0000000000002e80	blraaz	x8
```

`0x34000+0x20` is the dispatch worker callback. **MEASURED** that libpthread stores it itself, at
`0x29bc ldr x8, [x19, #0x18]; 0x29c0 str x8, [x20, #0x20]` inside `_pthread_workqueue_setup`, on the
success path of `___workq_kernreturn(0x400, …)` and **before** `___workq_open` — exactly the
`kernreturn(0x400) → open → kernreturn(0x20)` order the Stage 2a trace shows. So the guest already
has it set and retrace has to do nothing for it. Note `blraaz`: the callback is authenticated with
the **IA key, zero modifier**.

### 1e. The measured `(request → entry flags)` map

Captured by breaking on `__workq_kernreturn` and `start_wqthread` in the same run — **seven pairs
across six distinct queue configurations**, from runs B, C and E of the ledger in §0:

| run | queue configuration | REQTHREADS `x3` (priority) | entry `x4` (flags) | struct `x0` | fresh? |
|---|---|---|---|---|---|
| C | legacy `DISPATCH_QUEUE_PRIORITY_DEFAULT` | `0x000010ff` | `0x244005` | `0x16fe87000` | fresh (bit 17 clear, bit 21 set) |
| C | `QOS_CLASS_BACKGROUND` | `0x000002ff` | `0x064002` | `0x16fe87000` | reused (bit 17 set) |
| C | `QOS_CLASS_UTILITY` | `0x000004ff` | `0x244003` | `0x16ff13000` | fresh |
| C | `QOS_CLASS_USER_INITIATED` | `0x000010ff` | `0x064005` | `0x16ff13000` | reused |
| C | `QOS_CLASS_DEFAULT` + overcommit | `0x800010ff` | `0x074005` | `0x16ff13000` | reused, + bit 16 |
| E | `QOS_CLASS_DEFAULT` | `0x000010ff` | `0x244005` | `0x16fe87000` | fresh |
| B | legacy `DISPATCH_QUEUE_PRIORITY_DEFAULT` (the guest's own source) | `0x000010ff` | `0x244005` | `0x16fe87000` | fresh |

Rows B and E are independent runs of different programs; they duplicate row C-1's values rather than
adding new configurations, which is itself the reproducibility check.

**MEASURED:** every **fresh** entry has flags of the exact form `0x244000 | qos_index` — three
observations, two distinct indices — with
`qos_index` the inverse of the arithmetic above (`0x10ff` → bit 12 → index 5; `0x04ff` → bit 10 →
index 3). Every **reused** entry has `0x064000 | qos_index` (bit 17 in place of bit 21), plus bit 16
when the request carried `0x80000000`.

**The guest's own request is `0x040008ff`** (Stage 2a `args[3]`), whose QoS bit is `0x800` = `1<<11`
⇒ index **4**, predicting fresh flags **`0x244004`** — **unverified; see the blockquote below**.

> **UNVERIFIED: no live `(request → flags)` pair was captured for `0x040008ff`.** The six queue
> configurations in the table above are every configuration tried, and none reproduced it — the
> host's main thread carries a real QoS, so the requests came out `0x10ff` / `0x02ff` / `0x04ff` /
> `0x10ff` / `0x800010ff` / `0x10ff` instead. **`0x244004`** is an **extrapolation** from the
> measured points of the `0x244000 | n` form plus the measured arithmetic — not an observation.

One measured fact that *reduces* the risk in that extrapolation: **the `0x04000000` bit in the
request cannot survive to the worker on this path at all.** `__pthread_wqthread` reconstructs the
priority from flags bits 14/15/16/19/20 only (bit 16→prio 31, bit 19→prio 24, bit 20→prio 25, bit
24→prio 27); **no flags bit maps to priority bit 26.** The only way libpthread ever emits
`0x040008ff` is the bit-23 constant path, which is a different thread role. So a real kernel
delivering flags `0x244004` would hand the dispatch callback `0x8ff`, not `0x040008ff` — the
round-trip is lossy **by design**, and retrace reproducing that loss is fidelity, not a defect.

Answering the design spec's **open question 4** (does `args[3] = 0x40008ff` need decoding): **yes,
partially.** Its low-16-bit QoS field must be decoded to recover the index that goes in the flags
low byte, because that index is what libpthread turns back into the priority it hands the dispatch
callback. Its `0x04000000` bit must be **dropped**, per the paragraph above.

---

## 2. Memory layout: what retrace must allocate, and where

### 2a. Struct-relative facts, from the host probe

`wqprobe` output, worker-thread stanza — **an excerpt**: the `sp`, `mach_thread_self()`,
`[pthread+0x000]`, `+0x048`, `+0x0a4`, `+0x0a6`, `+0x0ac`, `+0x0d8`, `+0x0e8` and `+0x118` lines are
elided here and appear in §6's unabridged copy.

```
=== dispatch worker (workqueue) thread ===
  pthread_self         = 0x000000016b61b000
  TPIDRRO_EL0          = 0x000000016b61b0e0   (TPIDRRO - pthread = 0xe0)
  stackaddr_np         = 0x000000016b61b000   (pthread - stackaddr = 0x0)
  stacksize_np         = 0x83000
  vm_page_size         = 0x4000
  [pthread+0x0b0]            = 0x000000016b61b000
  [pthread+0x0b8]            = 0x000000016b598000
  [pthread+0x0c0]            = 0x000000016b594000
  [pthread+0x0c8]            = 0x000000000008c000
  [pthread+0x0d0]            = 0x0000000000004000
  [pthread+0x0e0] (tsd[0])   = 0x000000016b61b000
  [pthread+0x0f8] (kport)    = 0x00001d03
  [pthread+0x100] (prio)     = 0x00000000000010ff
```

**MEASURED, and it answers the design spec's open question 3 affirmatively:**
**`TPIDRRO_EL0 - pthread_self == 0xe0` for a workqueue thread**, identical to M14's
`bsdthread_create` measurement. The probe saw `0xe0` for **all three** thread kinds — main,
`pthread_create`, and workqueue — 3/3.

That offset is also **MEASURED in the disassembly**, independently of the probe, in `_pthread_exit`:

```
00000000000043e4	mrs	x8, TPIDRRO_EL0
00000000000043f4	sub	x0, x8, #0xe0
```

and `_thread_chkstk_darwin` reads the stack bounds the same way — `ldur x11, [x10, #-0x30]`
(`= pthread+0xb0`) and `ldur x11, [x10, #-0x28]` (`= pthread+0xb8`) off `TPIDRRO_EL0`.

**MEASURED:** `pthread_get_stackaddr_np() == pthread_self()` for both non-main thread kinds. The
struct base **is** the top of the stack; the stack grows down from it. Also 2/2 on the reported
`stacksize_np` (`0x83000`) equalling `pthread - [pthread+0xb8]` = `pthread - x2`.

### 2b. `__pthread_wqthread_setup`'s arithmetic, and its verification

The routine derives every stack field from `x0` and `x2`. Verbatim:

```
0000000000002f74	adrp	x8, 14 ; 0x10000
0000000000002f78	ldr	x8, [x8, #0x28] ; literal pool symbol address: _vm_page_size
0000000000002f7c	adrp	x9, 14 ; 0x10000
0000000000002f80	ldr	x9, [x9, #0x20] ; literal pool symbol address: _vm_page_mask
0000000000002f84	ldr	x10, [x8]
0000000000002f88	ldr	x9, [x9]
0000000000002fa0	add	x12, x0, x10
0000000000002fa4	sub	x12, x12, #0x1
0000000000002fb8	neg	x13, x10
0000000000002fbc	and	x12, x12, x13
0000000000002fc0	mov	w13, #0x18e0
0000000000002fcc	mvn	w11, w9
0000000000002fd0	sxtw	x11, w11
0000000000002fd4	add	x9, x9, x13
0000000000002fd8	and	x9, x11, x9
0000000000002fe0	sub	x10, x2, x10
0000000000002fe4	sub	x12, x12, x10
0000000000002fec	add	x9, x12, x9
0000000000002ff0	stp	x0, x2, [x0, #0xb0]
0000000000002ff4	stp	x10, x9, [x0, #0xc0]
0000000000002ff8	ldr	x8, [x8]
0000000000002ffc	str	x8, [x0, #0xd0]
```

Substituting the probe's numbers (`P = 0x4000`, `x0 = 0x16b61b000`, `x2 = 0x16b598000`):

- `[pthread+0xb0] = x0` → `0x16b61b000` ✓ observed
- `[pthread+0xb8] = x2` → `0x16b598000` ✓ observed
- `[pthread+0xc0] = x2 - P` → `0x16b594000` ✓ observed (the guard page below the stack)
- `[pthread+0xc8] = round_up(x0, P) - (x2 - P) + round_up(0x18e0, P)`
  `= 0x16b61c000 - 0x16b594000 + 0x4000 = 0x8c000` ✓ observed
- `[pthread+0xd0] = vm_page_size` → `0x4000` ✓ observed

**MEASURED:** the disassembly's arithmetic reproduces all five observed struct words exactly. The
struct's own size constant is `0x18e0`, page-rounded to `0x4000` — which is precisely the `pthsize`
`0x4000` the guest already passed to `bsdthread_register`.

### 2c. §2 — the layout retrace must build

**Every row MEASURED unless marked.**

| what | value / relationship |
|---|---|
| registers that carry it | **`x0` (struct) and `x2` (stack low). Neither is derived from the other, and neither is derived from `SP`** — libpthread computes the stack *bounds* from `x0` and `x2` and never inspects `SP`. Retrace must supply both. |
| struct size to allocate | at least `0x18e0`; the kernel's own allocation rounds to `round_up(0x18e0, vm_page_size) = 0x4000`, which equals the `pthsize` already captured in `Box_::pthread_size`. |
| struct placement | at the **top** of the region, above the stack. `x0 > x2` is required for `[pthread+0xc8]` to be a sane size. Observed `x0 - x2 = 0x83000`. |
| struct alignment | **not page-aligned in the observation** (`0x16b61b000 & 0x3fff = 0x3000`); the code page-rounds it itself. Retrace may page-align it; nothing measured forbids either. |
| stack region | `[x2, x0)`. Host-observed size `0x83000` (524 KiB). **The size is retrace's choice — nothing in the entry path requires a particular value.** |
| guard page | `[x2 - vm_page_size, x2)`. libpthread only *records* its base at `pthread+0xc0`; it does not map or check it. Retrace need not make it `PROT_NONE` for the entry path to work. |
| `SP` at entry | **`SP == x0`, 7/7 entries that read it** (§0 ledger). `_start_wqthread` immediately writes 16 bytes at `SP-0x10`, so `[x0-0x10, x0)` must be writable stack. |
| `TPIDRRO_EL0` | **`= pthread + 0xe0`.** Not optional: EL0 cannot write `TPIDRRO_EL0`, so the *kernel* must set it. **The guard is weaker than it looks: libpthread `brk`s only when flags bit 21 is CLEAR** (`0x3048 tbz w3, #0x15`) — it tests the *flag*, never the register. Bit 21 set with `TPIDRRO_EL0` wrong is **silent corruption, with no `brk` to hunt for** (§4). |
| struct memory must be **zero** | `__pthread_wqthread_setup` does read-modify-write on `ldrb w11,[x0,#0x31]` and `ldrh w9,[x0,#0x4e]` before storing them back. Fresh anonymous guest memory is zero, so this is satisfied by construction — but it is a real requirement, not a nicety. |
| stack depth check | `_thread_chkstk_darwin` requires `sp - framesize >= [pthread+0xb8] = x2`. A too-small stack fails there, not at entry. |

---

## 3. The trap numbers — both now VERIFIED

### 3a. The stubs

```sh
otool -tV /usr/lib/system/libsystem_kernel.dylib \
  | grep -A6 -E '^_semaphore_(wait|signal|wait_signal|timedwait)_trap:'
```
```
_semaphore_signal_trap:
0000000000000b84	mov	x16, #-0x21
0000000000000b88	svc	#0x80
0000000000000b8c	ret
_semaphore_signal_all_trap:
0000000000000b90	mov	x16, #-0x22
0000000000000b94	svc	#0x80
--
_semaphore_wait_trap:
0000000000000ba8	mov	x16, #-0x24
0000000000000bac	svc	#0x80
0000000000000bb0	ret
_semaphore_wait_signal_trap:
0000000000000bb4	mov	x16, #-0x25
0000000000000bb8	svc	#0x80
0000000000000bbc	ret
_semaphore_timedwait_trap:
0000000000000bc0	mov	x16, #-0x26
0000000000000bc4	svc	#0x80
0000000000000bc8	ret
_semaphore_timedwait_signal_trap:
0000000000000bcc	mov	x16, #-0x27
0000000000000bd0	svc	#0x80
=== rc=0 ===
```

**MEASURED — the two predictions are CONFIRMED, from libsystem_kernel's own stubs, on this machine:**

| name (now measured, not attributed) | `x16` immediate | decimal |
|---|---|---|
| `_semaphore_signal_trap` | `#-0x21` | **-33** |
| `_semaphore_signal_all_trap` | `#-0x22` | -34 |
| `_semaphore_signal_thread_trap` | `#-0x23` | -35 |
| `_semaphore_wait_trap` | `#-0x24` | **-36** |
| `_semaphore_wait_signal_trap` | `#-0x25` | -37 |
| `_semaphore_timedwait_trap` | `#-0x26` | -38 |
| `_semaphore_timedwait_signal_trap` | `#-0x27` | -39 |

Two neighbours pin the table further and cross-check the whole numbering:
`_mach_msg_overwrite_trap = #-0x20` (-32) and `_mach_msg2_trap = #-0x2f` (-47) — the latter matching
`crates/retrace-core/src/lib.rs`'s existing `MACH_MSG2 = -47` exactly.

**The predecessor document's attributions were right.** `docs/superpowers/specs/2026-08-21-retrace-m18-stage2b-measurements.md`
§1 labelled `-36` as `semaphore_wait_trap` and `-33` as `semaphore_signal_trap` with the explicit
caveat that neither had been checked against this machine. Both are now verified here. Stage 2a's
largest attribution debt is discharged.

`otool` succeeded, so the brief's `objdump` fallback was not needed.

### 3b. The wrappers that libdispatch actually calls

```
_semaphore_wait:
0000000000003dac	b	_semaphore_wait_trap
_semaphore_signal:
0000000000003db0	b	_semaphore_signal_trap
```

**MEASURED:** both are unconditional tail branches into the trap stubs — no argument marshalling, no
retry, no MIG. `semaphore_signal(port)` **is** trap `-33` with the port in `x0`.

### 3c. Step 5 — the libdispatch signal path

**The brief's Step 5 command FAILS on this machine, and that is a finding:**

```sh
$ otool -tV /usr/lib/system/libdispatch.dylib | sed -n '/^_dispatch_semaphore_signal:/,/^_[a-z]/p' | head -80
error: /Applications/Xcode.app/.../otool-classic: can't open file: /usr/lib/system/libdispatch.dylib (No such file or directory)
$ objdump --macho --disassemble --disassemble-symbols=_dispatch_semaphore_signal /usr/lib/system/libdispatch.dylib
objdump: error: '/usr/lib/system/libdispatch.dylib': No such file or directory
$ ls /usr/lib/system/
introspection
libsystem_kernel.dylib
libsystem_platform.dylib
libsystem_pthread.dylib
wordexp-helper
```

**MEASURED:** on macOS 26.5.2 exactly **three** dylibs survive on disk under `/usr/lib/system` —
`libsystem_kernel`, `libsystem_platform`, `libsystem_pthread`. `libdispatch.dylib` exists **only** in
the shared cache. (This is also why §0's on-disk-vs-cache equivalence check mattered: the two files
this document *can* read on disk are exactly the two it needs.)

**The fallback used instead** — `lldb`, disassembling out of the shared cache mapped into a live,
signable, non-SIP-protected process of our own:

```sh
lldb -b -o "b main" -o "run" -o "disassemble -n dispatch_semaphore_signal" ./wqprobe
```
```
libdispatch.dylib`dispatch_semaphore_signal:
    0x18d3eca60 <+0>:  add    x8, x0, #0x30
    0x18d3eca64 <+4>:  mov    w9, #0x1
    0x18d3eca68 <+8>:  ldaddl x9, x8, [x8]
    0x18d3eca6c <+12>: tbnz   x8, #0x3f, 0x18d3eca78
    0x18d3eca70 <+16>: mov    x0, #0x0
    0x18d3eca74 <+20>: ret
    0x18d3eca78 <+24>: b      0x18d3eca1c    ; _dispatch_semaphore_signal_slow
```

**MEASURED — the answer to the brief's question is yes: the fast path is a pure atomic increment
with no trap.** `ldaddl` adds `+1` to the counter at `sem+0x30` and returns the *previous* value in
`x8`; only if that was negative (a waiter exists) does it fall into the slow path.

The slow path, and where it lands:

```
libdispatch.dylib`_dispatch_semaphore_signal_slow:
    0x18d3eca30 <+20>: ldr    w8, [x19, #0x40]!
    0x18d3eca34 <+24>: cbnz   w8, 0x18d3eca44
    0x18d3eca40 <+36>: bl     0x18d3ec278   ; _dispatch_sema4_create_slow
    0x18d3eca44 <+40>: mov    x0, x19
    0x18d3eca48 <+44>: mov    w1, #0x1
    0x18d3eca4c <+48>: bl     0x18d3ec420   ; _dispatch_sema4_signal

libdispatch.dylib`_dispatch_sema4_signal:
    0x18d3ec438 <+24>: ldr    w0, [x20]
    0x18d3ec43c <+28>: bl     0x18d4271e4   ; symbol stub for: semaphore_signal
    0x18d3ec440 <+32>: cmn    w0, #0x12d
    0x18d3ec444 <+36>: b.eq   0x18d3ec460   ; _dispatch_sema4_create_slow.cold.6
    0x18d3ec448 <+40>: cbnz   w0, 0x18d3ec464
    0x18d3ec44c <+44>: subs   x19, x19, #0x1
    0x18d3ec450 <+48>: b.ne   0x18d3ec438
```

**§3 — the verified trap numbers, and what the seam must return.**

| | trap | source |
|---|---|---|
| `dispatch_semaphore_wait` slow path | `_dispatch_sema4_wait` → `bl semaphore_wait` → `b _semaphore_wait_trap` = **`-36`** | MEASURED (disassembly + Stage 2a trace) |
| `dispatch_semaphore_signal` slow path | `_dispatch_sema4_signal` → `bl semaphore_signal` → `b _semaphore_signal_trap` = **`-33`** | MEASURED (disassembly). **Still never observed in a retrace trace** — no run has yet got far enough to execute it. |
| `dispatch_semaphore_signal` fast path | **no trap at all** — `ldaddl` only | MEASURED |
| required return value | **`0`** from both. `_dispatch_sema4_signal` does `cbnz w0, …cold.7` (fatal) on anything non-zero, and `cmn w0,#0x12d` (`-301`) is separately fatal. `_dispatch_sema4_wait` returns cleanly **only** on `0`; `0xe` re-issues the trap, `0xf` is fatal, and every other value reaches a `cold` routine. | MEASURED |
| retry loop on wait | `0x18d3ec49c b.eq 0x18d3ec488` — returning `0xe` makes libdispatch **re-issue `-36` forever**. Only `0` exits cleanly. | MEASURED |

This settles the predecessor document's open point that `-33` "has never been observed by anything":
it is now verified as *the instruction libdispatch reaches*, on this machine, at
`_dispatch_sema4_signal+28`. **It remains unobserved in a retrace trace**, and that distinction is
kept deliberately.

### 3d. One more trap number, free: the park opcode

Immediately after the dispatch callback returns, `__pthread_wqthread` does:

```
0000000000002e84	mov	w0, #0x4
0000000000002e88	mov	x1, #0x0
0000000000002e8c	mov	w2, #0x0
0000000000002f04	mov	w3, #0x0
0000000000002f08	bl	0xb5e8 ; symbol stub for: ___workq_kernreturn
0000000000002f0c	ldrsw	x8, [x19, #0xac]
0000000000002f14	adrp	x20, 9 ; 0xb000
0000000000002f18	add	x20, x20, #0xe11 ; literal pool for: "BUG IN LIBPTHREAD: __workq_kernreturn returned"
```

and the host confirms it live (last breakpoint hit before exit, on `dispatch_host`):

```
      x0 = 0x0000000000000004
      x1 = 0x0000000000000000
      x2 = 0x0000000000000000
      x3 = 0x0000000000000000
```

**MEASURED: the park/return opcode is `0x4`, with `(0, 0, 0)`, and `workq_kernreturn` must NOT
return from it.** If it returns, libpthread stores a crash message and falls off the end into
`_start_wqthread`'s `brk #0x1`. This is the design spec's **risk 3** partially retired before any
code is written: at least one park opcode is now known by value, and its no-return contract is known
too. It remains a floor, not a ceiling — `x5 == -1` at entry selects `__pthread_wqthread_exit`
instead, which is a second, separate teardown path.

---

## 4. §4 — the struct-init verdict: **CONFIRMED**

The design spec's hypothesis
(`docs/superpowers/specs/2026-08-23-retrace-m18-stage2b-design.md`, "The kernel-allocates problem,
and the escape hatch"):

> libpthread's workqueue entry distinguishes a *fresh* thread from a *reused* one by a flag bit, and
> on the fresh path performs its own struct initialisation on the memory the kernel handed it.

### **CONFIRMED.**

- **The flag bit is `x4` bit 17 (`0x20000`).** `0x2dc0 tbnz w4, #0x11, 0x2dd0` — **set** ⇒ jump past
  the setup call (reused); **clear** ⇒ fall through and `bl __pthread_wqthread_setup` (fresh).
  Retrace must leave it **clear**.
- **The routine is `__pthread_wqthread_setup`** (`libsystem_pthread.dylib`, offset `0x2f58`).
- **It writes the struct rather than reading it** — verified instruction by instruction. In its first
  60 instructions it stores to `pthread + 0x00, 0x30, 0x31, 0x38, 0x3c, 0x48, 0x4e, 0xa6, 0xb0, 0xb8,
  0xc0, 0xc8, 0xd0, 0xd8, 0xe0, 0xe8, 0xf8, 0x100, 0x118`, then links the thread into libpthread's
  global thread list. The only fields it *reads* first are `+0x31` and `+0x4e`, both read-modify-write
  on zeroed memory.
- **It writes the signature and the TSD self-pointers itself:**
  ```
  0000000000002f70	mov	w16, #0x5b9
  0000000000002f8c	pacdb	x17, x16
  0000000000002f98	eor	x12, x12, x17
  0000000000002f9c	str	x12, [x0]
  0000000000002fa8	add	x13, x0, #0xac
  0000000000002fac	stp	x0, x13, [x0, #0xe0]
  ```
  The struct's own PAC signature is computed **in-guest with the guest's own keys**. Retrace must not
  — and now need not — author it. This is exactly the property M14's rule wanted: **retrace invents an
  address, not a layout.**

### What the kernel still owes, and how libpthread says so

The `brk`s are the contract, and they are the same `brk #0xb001` shape M14 hit for
`PTHREAD_START_TSD_BASE_SET`:

| requirement | check | failure |
|---|---|---|
| flags bit 21 set (kernel set the TSD base) | `0x3048 tbz w3, #0x15, 0x3114` | `"BUG IN LIBPTHREAD: thread_set_tsd_base() wasn't called by the kernel"` + `brk #0xb001` |
| `TPIDRRO_EL0 = pthread + 0xe0` in fact | *(unchecked directly, but every TSD read depends on it)* | silent corruption if bit 21 is set and the register is not |
| `x1` is a real mach port | `0x30c8 ldr w8,[x19,#0xf8]; add w9,w8,#1; cmp w9,#1; b.ls 0x315c` — fatal for `0` and `-1` | `"BUG IN CLIENT OF LIBPTHREAD: Unable to allocate thread port, possible port leak"` + `brk #0xb001` |
| `__thread_selfid()` succeeds | `0x3054 bl ___thread_selfid; 0x305c cmn x0,#0x1; b.eq 0x313c` | `"BUG IN LIBPTHREAD: failed to set thread_id"` + `brk #0xb001` |
| flags bit 14 or 15 set | `0x2dec tbnz w22,#0xe` / `0x2df0 tbz w22,#0xf, 0x2f50` | `"BUG IN LIBPTHREAD: Missing priority"` + `brk #0xb001` |

Answering the design spec's **open question 2** (does the worker need its own mach thread port
written somewhere, as `bsdthread_create`'s child needed one at `pthread + 0xf8`): **yes — but
retrace supplies it in `x1`, and libpthread stores it, so retrace still writes no struct field.**
And unlike M14's silent `pthread_join` failure, this one is **loud**: `0` and `-1` both `brk`.

`__thread_selfid` is BSD syscall **372**, which `retrace-arch` already knows
(`SYS_THREAD_SELFID`, `crates/retrace-arch/src/lib.rs:230`) and which every dynamic guest already
issues. Its return value is stored at `pthread + 0xd8` and only checked against `-1`.

---

## 5. What this hands Tasks 2–4

1. **§1's table is complete and small.** Build the worker's `ThreadCtx` with `x0` = struct IPA,
   `x1` = a mach port name, `x2` = stack-low IPA, `x3 = 0`, `x4` = flags, `x5 = 0`, `x6 = x7 = 0`,
   `SP = x0`, `LR = 0`, `PC = Box_::wq_thread_pc()`, and set `TPIDRRO_EL0 = x0 + 0xe0`. Nothing else
   is read.
2. **§4 is CONFIRMED, so Stage 2b is not walled here.** Retrace allocates two regions and clears
   flags bit 17; libpthread writes its own struct, including its own PAC signature. M14's
   "no inventing guest state" rule survives without an exception.
3. **Do not set flags bit 23.** It is the one path that produces the constant `0x040008ff` — the very
   value the guest's `REQTHREADS` carries — which makes it a tempting-looking match. It also sets
   `pthread + 0xa4 = 1` and bypasses the QoS arithmetic entirely, i.e. it is a **different thread
   role**, not a worker. Setting it to "make the priority come out right" would be the exact class of
   mistake this document exists to prevent.
4. **The flags word to use is `0x244000 | qos_index`** where `qos_index` inverts
   `priority_qos_bit == 1 << (qos_index + 7)` over the request's bits `[15:8]`, and must land in
   `1..=6`. For the guest's measured request `0x040008ff` that is `qos_index = 4`, i.e.
   **`0x244004` — an extrapolation, flagged unverified in §1e.** Two distinct measured fresh values
   (`0x244005`, `0x244003`) support the form. The `0x04000000` bit of the request is dropped; that
   loss is what the real kernel path does too (§1e).
5. **`args[2]` of `REQTHREADS` really is the thread count.** The spec explicitly flagged
   "one worker" as an attribution. It is now **MEASURED** from libpthread's own caller:
   ```
   __pthread_workqueue_addthreads:
   [0x2d48 .. 0x2d5c ELIDED — the null-callback check and the prologue]
   0000000000002d60	mov	x2, x0
   0000000000002d64	and	w3, w1, #0xdfffffff
   0000000000002d68	mov	w0, #0x20
   0000000000002d6c	mov	x1, #0x0
   0000000000002d70	bl	0xb5e8 ; symbol stub for: ___workq_kernreturn
   ```
   The bracketed line is **the only edit** to `otool`'s output; every instruction above is verbatim.
   Reading it — **annotation, not disassembly**: `x2 = x0` is `numthreads`, `w3` is the
   priority with bit 29 masked off, `w0 = 0x20` is `REQTHREADS`, `x1 = 0`.
   The guest's `[0x20, 0x0, 0x1, 0x40008ff]` is therefore literally "one thread, at this priority."
   It must not return `-1`: `0x2d74 cmn w0,#0x1; b.ne 0x2d88` takes the **success** branch to the
   epilogue; `-1` is the fall-through, into the errno read at `0x2d7c`.
6. **The park opcode is `0x4` and it must never return** (§3d). Task 2's `REQTHREADS` arm has a
   known counterpart to implement, and `guest_workq_kernreturn`'s refuse-by-value posture keeps any
   *other* opcode loud.
7. **The semaphore seam's numbers are settled**: wait `-36`, signal `-33`, both must return `0`, and
   the signal **fast path issues no trap at all** — so a worker that signals a semaphore nobody is
   waiting on produces no landmark. The park/wake seam must not assume a signal trap always appears.
8. **Nothing here requires a `kevent` list.** `x3`/`x5` are the kevent/workloop arguments and are
   dead on the measured path.

### Deliberately not measured

- **The `0x040008ff` → flags pair on a live kernel.** All six queue configurations tried failed to
  produce that request value (§1e). It could only be closed by making a host process whose main
  thread has no QoS, which was not achievable inside this task. **Stated as unverified rather than
  inferred.**
- **What flags bit 18 (`0x40000`) means.** Set in 13/13 entries; read by nothing on the path
  `__pthread_wqthread` executes. Retrace can set it (matching the kernel) or not; **untested either
  way.**
- **The reused-thread path.** Measured to exist (flags bit 17, three observations) but Stage 2b
  builds one worker and does not reuse it. If a later stage parks and re-dispatches a worker, the
  `0x064000 | qos_index` form is the shape to expect.
- **Whether the guest's libdispatch is byte-identical to the host's.** §0 established equivalence for
  `libsystem_kernel` and `libsystem_pthread` via four PC matches. **No such cross-check exists for
  `libdispatch`** — it is not on disk, so both instruments read the same shared cache and cannot
  disagree. The §3c disassembly is of the cache copy, which *is* what the guest runs, so this is a
  weaker form of the same assurance rather than a gap; it is named here so nobody upgrades it.

---

## 6. Reproduction

Everything in this section is the **actual file as it sat on disk**, not a retyped summary. The
probes themselves are not committed; these listings are what makes their output regenerable.

### Commands, verbatim, in the order they were run

```sh
sw_vers; uname -a

# §3a — the trap stubs
otool -tV /usr/lib/system/libsystem_kernel.dylib \
  | grep -A6 -E '^_semaphore_(wait|signal|wait_signal|timedwait)_trap:'

# §0 — the cross-validation offsets
otool -tV /usr/lib/system/libsystem_kernel.dylib | grep -B2 -A6 -E '^___workq_(open|kernreturn):'
otool -l /usr/lib/system/libsystem_kernel.dylib | grep -A8 'sectname __text'

# §1 — the wqthread entry
otool -tV /usr/lib/system/libsystem_pthread.dylib \
  | sed -n '/^_start_wqthread:/,/^_[a-z]/p' | head -60
otool -tV /usr/lib/system/libsystem_pthread.dylib \
  | sed -n '/^__pthread_wqthread:/,/^_[a-z]/p' | head -200

# §2a — the struct/stack/TSD probe (produces the output at the end of this section)
clang -g -O0 -o wqprobe wqprobe.c && ./wqprobe

# run A — ad-hoc entry capture, and the source of the pc/lr reading
lldb -b -o "breakpoint set -n start_wqthread" -o "run" \
     -o "register read x0 x1 x2 x3 x4 x5 x6 x7 sp pc lr" -o "bt" \
     -o "disassemble -n _pthread_wqthread -c 12" ./wqprobe

# §3c — libdispatch, via lldb because the dylib is not on disk. TWO separate invocations.
lldb -b -o "b main" -o "run" -o "disassemble -n dispatch_semaphore_signal" ./wqprobe
lldb -b -o "b main" -o "run" \
     -o "disassemble -n _dispatch_semaphore_signal_slow" \
     -o "disassemble -n dispatch_semaphore_wait" \
     -o "disassemble -n _dispatch_semaphore_wait_slow" ./wqprobe
lldb -b -o "b main" -o "run" \
     -o "disassemble -n _dispatch_sema4_signal" \
     -o "disassemble -n _dispatch_sema4_wait" \
     -o "disassemble -n _dispatch_sema4_create_slow" ./wqprobe

# run B — the real guest source, compiled for the host
cp crates/retrace-guest/c/dispatch_dyn.c ./dispatch_host.c
clang -g -O0 -o dispatch_host dispatch_host.c
lldb -b -s cmds.txt ./dispatch_host

# runs C and D — the QoS sweep, then the same binary re-run for sp/lr
clang -g -O0 -o qosprobe qosprobe.c
lldb -b -s cmds2.txt ./qosprobe
lldb -b -s cmds3.txt ./qosprobe

# run E — QOS_CLASS_DEFAULT alone
clang -g -O0 -o qos2 qos2.c
lldb -b -s cmds2.txt ./qos2
```

### `cmds.txt` (run B)

```
breakpoint set -n __workq_kernreturn
breakpoint command add 1
register read x0 x1 x2 x3
continue
DONE
breakpoint set -n start_wqthread
breakpoint command add 2
register read x0 x1 x2 x3 x4 x5 sp lr
continue
DONE
run
```

### `cmds2.txt` (runs C and E)

```
breakpoint set -n __workq_kernreturn
breakpoint command add 1
register read x0 x3
continue
DONE
breakpoint set -n start_wqthread
breakpoint command add 2
register read x0 x2 x4
continue
DONE
run
```

### `cmds3.txt` (run D)

```
breakpoint set -n start_wqthread
breakpoint command add 1
register read x0 x1 x2 x3 x4 x5 x6 x7 sp lr
continue
DONE
run
```

### `wqprobe.c`

```c
// M18 Stage 2b Task 1 host probe: measure the workqueue thread's pthread-struct /
// stack / TPIDRRO_EL0 relationships on the real OS, the way M14 measured
// pthread+0xe0 and pthread+0xf8 for a bsdthread_create child.
#include <dispatch/dispatch.h>
#include <pthread.h>
#include <stdio.h>
#include <stdint.h>
#include <mach/mach.h>
#include <unistd.h>

static uint64_t tpidrro(void) { uint64_t v; __asm__ volatile("mrs %0, TPIDRRO_EL0" : "=r"(v)); return v; }
static uint64_t sp_now(void)  { uint64_t v; __asm__ volatile("mov %0, sp"          : "=r"(v)); return v; }

static void dump(const char *who) {
    pthread_t self = pthread_self();
    uint64_t  p    = (uint64_t)self;
    uint64_t  t    = tpidrro();
    uint64_t  sp   = sp_now();
    void     *sa   = pthread_get_stackaddr_np(self);
    size_t    ss   = pthread_get_stacksize_np(self);
    const unsigned char *b = (const unsigned char *)p;

    printf("=== %s ===\n", who);
    printf("  pthread_self         = 0x%016llx\n", p);
    printf("  TPIDRRO_EL0          = 0x%016llx   (TPIDRRO - pthread = 0x%llx)\n", t, (unsigned long long)(t - p));
    printf("  sp                   = 0x%016llx   (pthread - sp = 0x%llx)\n", sp, (unsigned long long)(p - sp));
    printf("  stackaddr_np         = 0x%016llx   (pthread - stackaddr = 0x%llx)\n",
           (unsigned long long)(uintptr_t)sa, (unsigned long long)(p - (uint64_t)(uintptr_t)sa));
    printf("  stacksize_np         = 0x%zx\n", ss);
    printf("  mach_thread_self()   = 0x%x\n", (unsigned)mach_thread_self());
    printf("  vm_page_size         = 0x%lx\n", (unsigned long)vm_page_size);
    /* raw struct words at the offsets __pthread_wqthread_setup writes */
    printf("  [pthread+0x000] (sig)      = 0x%016llx\n", *(const uint64_t *)(b + 0x000));
    printf("  [pthread+0x048]            = 0x%08x\n",    *(const uint32_t *)(b + 0x048));
    printf("  [pthread+0x0a4] (byte)     = 0x%02x\n",    *(const uint8_t  *)(b + 0x0a4));
    printf("  [pthread+0x0a6] (half)     = 0x%04x\n",    *(const uint16_t *)(b + 0x0a6));
    printf("  [pthread+0x0ac] (i32)      = %d\n",        *(const int32_t  *)(b + 0x0ac));
    printf("  [pthread+0x0b0]            = 0x%016llx\n", *(const uint64_t *)(b + 0x0b0));
    printf("  [pthread+0x0b8]            = 0x%016llx\n", *(const uint64_t *)(b + 0x0b8));
    printf("  [pthread+0x0c0]            = 0x%016llx\n", *(const uint64_t *)(b + 0x0c0));
    printf("  [pthread+0x0c8]            = 0x%016llx\n", *(const uint64_t *)(b + 0x0c8));
    printf("  [pthread+0x0d0]            = 0x%016llx\n", *(const uint64_t *)(b + 0x0d0));
    printf("  [pthread+0x0d8] (selfid)   = 0x%016llx\n", *(const uint64_t *)(b + 0x0d8));
    printf("  [pthread+0x0e0] (tsd[0])   = 0x%016llx\n", *(const uint64_t *)(b + 0x0e0));
    printf("  [pthread+0x0e8] (tsd[1])   = 0x%016llx\n", *(const uint64_t *)(b + 0x0e8));
    printf("  [pthread+0x0f8] (kport)    = 0x%08x\n",    *(const uint32_t *)(b + 0x0f8));
    printf("  [pthread+0x100] (prio)     = 0x%016llx\n", *(const uint64_t *)(b + 0x100));
    printf("  [pthread+0x118]            = 0x%016llx\n", *(const uint64_t *)(b + 0x118));
    fflush(stdout);
}

static void *pt_main(void *arg) { (void)arg; dump("pthread_create thread"); return NULL; }

int main(void) {
    dump("main thread");

    pthread_t th;
    pthread_create(&th, NULL, pt_main, NULL);
    pthread_join(th, NULL);

    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
        dump("dispatch worker (workqueue) thread");
        dispatch_semaphore_signal(sem);
    });
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);
    printf("done\n");
    return 0;
}
```

### `qosprobe.c`

```c
#include <dispatch/dispatch.h>
#include <stdio.h>
static void one(dispatch_queue_t q, const char *tag) {
    dispatch_semaphore_t s = dispatch_semaphore_create(0);
    dispatch_async(q, ^{ printf("ran on %s\n", tag); fflush(stdout); dispatch_semaphore_signal(s); });
    dispatch_semaphore_wait(s, DISPATCH_TIME_FOREVER);
}
int main(void) {
    one(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_DEFAULT, 0), "legacy DEFAULT");
    one(dispatch_get_global_queue(QOS_CLASS_BACKGROUND, 0),            "QOS BACKGROUND");
    one(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0),               "QOS UTILITY");
    one(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0),        "QOS USER_INITIATED");
    one(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0x2ull), "DEFAULT overcommit");
    return 0;
}
```

### `qos2.c`

```c
#include <dispatch/dispatch.h>
#include <stdio.h>
static void one(dispatch_queue_t q, const char *tag) {
    dispatch_semaphore_t s = dispatch_semaphore_create(0);
    dispatch_async(q, ^{ printf("ran on %s\n", tag); fflush(stdout); dispatch_semaphore_signal(s); });
    dispatch_semaphore_wait(s, DISPATCH_TIME_FOREVER);
}
int main(void) {
    one(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), "QOS DEFAULT");
    return 0;
}
```

### `dispatch_host.c`

Byte-identical to `crates/retrace-guest/c/dispatch_dyn.c`; not reproduced here, since the repo
already carries it.

### Full `wqprobe` output — verbatim, unabridged

This is the complete stdout of `./wqprobe`, unedited. The `wqprobe.c` listing above is the source
that produced it.

```
=== main thread ===
  pthread_self         = 0x00000001f97add80
  TPIDRRO_EL0          = 0x00000001f97ade60   (TPIDRRO - pthread = 0xe0)
  sp                   = 0x000000016b5925d0   (pthread - sp = 0x8e21b7b0)
  stackaddr_np         = 0x000000016b594000   (pthread - stackaddr = 0x8e219d80)
  stacksize_np         = 0x7fc000
  mach_thread_self()   = 0x103
  vm_page_size         = 0x4000
  [pthread+0x000] (sig)      = 0x5b9bc89e5703f5c6
  [pthread+0x048]            = 0x00000000
  [pthread+0x0a4] (byte)     = 0x00
  [pthread+0x0a6] (half)     = 0x0003
  [pthread+0x0ac] (i32)      = 0
  [pthread+0x0b0]            = 0x000000016b594000
  [pthread+0x0b8]            = 0x000000016ad98000
  [pthread+0x0c0]            = 0x0000000167594000
  [pthread+0x0c8]            = 0x0000000004000000
  [pthread+0x0d0]            = 0x0000000000004000
  [pthread+0x0d8] (selfid)   = 0x0000000000025d3e
  [pthread+0x0e0] (tsd[0])   = 0x00000001f97add80
  [pthread+0x0e8] (tsd[1])   = 0x00000001f97ade2c
  [pthread+0x0f8] (kport)    = 0x00000103
  [pthread+0x100] (prio)     = 0x00000000000020ff
  [pthread+0x118]            = 0x5b9bc89fae792846
=== pthread_create thread ===
  pthread_self         = 0x000000016b61b000
  TPIDRRO_EL0          = 0x000000016b61b0e0   (TPIDRRO - pthread = 0xe0)
  sp                   = 0x000000016b61af40   (pthread - sp = 0xc0)
  stackaddr_np         = 0x000000016b61b000   (pthread - stackaddr = 0x0)
  stacksize_np         = 0x83000
  mach_thread_self()   = 0x1f03
  vm_page_size         = 0x4000
  [pthread+0x000] (sig)      = 0x5b9bc89ec5189846
  [pthread+0x048]            = 0x00000000
  [pthread+0x0a4] (byte)     = 0x00
  [pthread+0x0a6] (half)     = 0x0003
  [pthread+0x0ac] (i32)      = 0
  [pthread+0x0b0]            = 0x000000016b61b000
  [pthread+0x0b8]            = 0x000000016b598000
  [pthread+0x0c0]            = 0x000000016b594000
  [pthread+0x0c8]            = 0x000000000008c000
  [pthread+0x0d0]            = 0x0000000000004000
  [pthread+0x0d8] (selfid)   = 0x0000000000025e74
  [pthread+0x0e0] (tsd[0])   = 0x000000016b61b000
  [pthread+0x0e8] (tsd[1])   = 0x000000016b61b0ac
  [pthread+0x0f8] (kport)    = 0x00001f03
  [pthread+0x100] (prio)     = 0x00000000000008ff
  [pthread+0x118]            = 0x5b9bc89fae792846
=== dispatch worker (workqueue) thread ===
  pthread_self         = 0x000000016b61b000
  TPIDRRO_EL0          = 0x000000016b61b0e0   (TPIDRRO - pthread = 0xe0)
  sp                   = 0x000000016b61adb0   (pthread - sp = 0x250)
  stackaddr_np         = 0x000000016b61b000   (pthread - stackaddr = 0x0)
  stacksize_np         = 0x83000
  mach_thread_self()   = 0x1d03
  vm_page_size         = 0x4000
  [pthread+0x000] (sig)      = 0x5b9bc89ec5189846
  [pthread+0x048]            = 0x00000000
  [pthread+0x0a4] (byte)     = 0x00
  [pthread+0x0a6] (half)     = 0x0003
  [pthread+0x0ac] (i32)      = 0
  [pthread+0x0b0]            = 0x000000016b61b000
  [pthread+0x0b8]            = 0x000000016b598000
  [pthread+0x0c0]            = 0x000000016b594000
  [pthread+0x0c8]            = 0x000000000008c000
  [pthread+0x0d0]            = 0x0000000000004000
  [pthread+0x0d8] (selfid)   = 0x0000000000025e75
  [pthread+0x0e0] (tsd[0])   = 0x000000016b61b000
  [pthread+0x0e8] (tsd[1])   = 0x000000016b61b0ac
  [pthread+0x0f8] (kport)    = 0x00001d03
  [pthread+0x100] (prio)     = 0x00000000000010ff
  [pthread+0x118]            = 0x5b9bc89fae792846
done
```

**Note on that output**, so nobody misreads it: the worker reused the *same* struct address
`0x16b61b000` as the just-joined `pthread_create` thread — libpthread recycled the freed allocation.
It is nonetheless a genuinely different thread (`kport` `0x1f03` vs `0x1d03`, `selfid` `…e74` vs
`…e75`). The `prio` field differs accordingly (`0x8ff` vs `0x10ff`), which is what makes the two
stanzas independently informative.

### No production code was touched

This task wrote exactly one file — this document. `crates/`, `README.md` and `docs/status-log.md` are
untouched; the probes above live in scratch and are not committed, in the spirit of `spikes/`.
