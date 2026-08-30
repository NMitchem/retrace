# M23-xpcpipe measurements

Measured 2026-08-29 on macOS 26.x / Apple Silicon, against `main` at `10fcf5a` (post-M22).

## Why measure before designing

M22 left 20 of 54 sampled Apple system binaries failing, in what looked like four classes. The
largest — 13 binaries reporting `pc=0x4204` — was assumed to be a wall. The milestone that follows
depends entirely on whether those are 13 causes, four causes, or one, and the error text cannot tell
them apart. It turned out to be **one**, twice over: the four apparent classes collapse to a single
wall, and it is not the one any of the error messages named.

## S1. `pc=0x4204` is a masking artifact, not a wall

`TRAMPOLINE_IPA = 0x4000` and `VBAR_EL1 = TRAMPOLINE_IPA`, so the EL1 vector table is 16 slots of
0x80 bytes. Each slot gets `hvc #0` at slot+0; **the remaining 0x7c bytes are left zero, which is
`UDF #0`**. Separately (S2), the vector's `hvc` occasionally produces no VM exit and execution falls
through. It then hits that `UDF` **at EL1**, which overwrites `ELR_EL1`/`SPSR_EL1` with `0x4404`/EL1h
and vectors to `VBAR+0x200`, reporting `pc=0x4204`. The original exception's identity is destroyed
before any handler sees it.

Filling the padding with `hvc #1` (`0xd4000022`) instead of zero makes a fall-through *observable*
(ESR_EL2 ISS=1) and leaves `ESR_EL1`/`ELR_EL1` intact. With that one change, **all 13 binaries
advance past `0x4204`**:

```
aa afktool AssetCacheManagerUtil automationmodetool avmediainfo bioutil chfn
csrutil dddiagnose desdp dserr dyld_info flex        # 13/13, all now reach S3
```

This is M22 design-risk **R4 ("the parked wall is really several causes wearing one error message")
confirmed** — and it is worse than R4 predicted, because the masking error was not merely uninformative
but actively misattributed the failure to an address in retrace's own trampoline.

## S2. The fall-through itself: characterised, not explained

The anomaly underneath is that **the vector's `hvc` intermittently yields no exit** — 4 occurrences
in 1492 vector entries (~0.27%), deterministic (the same four points across 5 runs and 3 vector
layouts).

Two probes pinned its shape:

- **Moving the `hvc`.** Inserting a NOP so the slot head's `hvc` sits at `0x4404` instead of `0x4400`
  moved the fall-through with it (`0x4408` → `0x440c`), still exactly 4. The anomaly is bound to the
  **`hvc`'s position**, not to an address.
- **Registers across the run.** x0 `0x18f532000`→`0xffffffff`, x8 `0`→`2`. The guest **really
  executed**; this is not a stale-PC resume.

Ruled out with evidence: a dropped register write (readback is correct right up to `hv_vcpu_run`, and
forcing HVF's dirty bit with a bogus intermediate write changed nothing); `save_state`/`restore_state`
(symmetric, balanced 235/235, and one fall-through has none nearby); the `hv-sys` wrapper (direct FFI,
no caching); corrupted vectors (dumped intact at the moment of failure); stale syndromes (all 2594
exits are `reason=1`); VBAR corruption (`0x4000` throughout); thread scheduling; and site-specificity
(all 4 land on the most frequent trap site, 842/1492 — consistent with a uniform rate).

**Why it stays unexplained:** two readings remain and both sit below retrace — either the `hvc`
genuinely does not trap, or HVF **coalesces** two exceptions into one exit, reporting the later
syndrome and silently consuming the first. Distinguishing them needs a `spikes/*.c` probe against
HVF. **This milestone does not claim to know which.** See S5 for why that does not block the work.

## S3. The 20 failures are 17 + 3, and the 17 are one cause

With S1's padding in place, all 13 land on the identical wall, at the identical pc, as the 4 already
known to fail there (bash, cal, date, zsh):

```
RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 412 dest 0x1d03 send_size 40
```

**17 of the 20.** The remaining 3 are unrelated: csh and tcsh panic at `retrace-core/src/lib.rs:1103`,
and `ps` diverges at landmark 306.

**msgh_id 412 is `host_get_special_port`** — host_priv MIG subsystem base 400, slot 12. Measured, not
inferred from the number:

- The request body is exactly `__Request__host_get_special_port_t` — header(24) + NDR(8) +
  `node`(4) + `which`(4) = **40 bytes**, matching `send_size=40`:
  ```
  send+020: ff ff ff ff 01 00 00 00     # node = -1 (HOST_LOCAL_NODE), which = 1 (HOST_PORT)
  ```
- `dest` is **the host port**, not the guest task port (515) — which is why `route()` misses it, since
  every existing serviced arm is keyed on `dest == guest_task_port`. Confirmed by the trap sequence:
  `[trap] num=-29` (`host_self_trap`) is immediately followed by a `mach_msg2` to that same `dest`,
  three separate times in one run (for msgh_ids 200, 206 and 412).

**`(node, which)` is uniform across all 17** — exactly one 412 send per run, `node=-1`, `which=1`,
`rcv_size=48`, every time. No binary asks for a different special port.

## S4. Servicing 412 is one line — and unlocks nothing

`host_info` (200) and `host_get_clock_service` (206) are **already forwarded** to that same host port
via `FORWARD_ALLOWLIST`, which is keyed on msgh_id alone; and 206 already returns a **port**, so the
machinery for a port-returning forward exists. Adding one entry:

```rust
(412, "host_get_special_port")
```

works on the first try:

```
[retrace] forwarding mach_msg2 host_get_special_port (msgh_id 412) to host (decided allowlist)
```

**But every one of the 17 then converges on the same next wall**, so the count of working binaries is
unchanged at 34/54:

```
RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x403114207
              (not the kernel-object send+rcv shape)
```

`0x4_0000_0000` is **`MACH64_SEND_MQ_CALL`** — a real message-queue send, not a kernel-object call.
The body carries the XPC serialization magic and its keys:

```
send+020: 00 00 00 00 00 00 13 00 43 50 58 40 05 00 00 00     # 43 50 58 40 = "@XPC"
send+030: 00 f0 00 00 c0 00 00 00 07 00 00 00 68 61 6e 64     # "hand
send+040: 6c 65 00 00 00 40 00 00 00 00 00 00 00 00 00 00     #  le"
send+050: 69 6e 73 74 61 6e 63 65 00 00 00 00 00 a0 00 00     # "instance"
```

dest `0x1503`, reply port `0x2003`, `send_size=248`. **This is the guest opening a genuine XPC
connection** — the send/dispatch-mach subsystem that M2 recorded as deferred and never exercised
("a write-only guest opens no XPC connection"). It is now exercised, by all 17.

**This is the finding that sets the milestone's scope.** 412 is a one-line prerequisite, not the
deliverable.

## S5. The fall-through determinism test is blocked, and the ordering is forced

The open question from S2 that *matters* for retrace is not the mechanism but whether the fall-through
is identical on record and replay — because `Box_::run()` is shared, so a divergent one would be the
single failure a determinism oracle cannot see.

That test **cannot be run today**. With zero padding a fall-through is always fatal, so **every binary
that records successfully has zero fall-throughs by construction**. Verified rather than assumed, on 8
passing binaries:

```
ls sh dash echo df pwd bzip2 ed     # ft=0 on BOTH record and replay, all 8
```

So no guest that completes a recording exhibits one. The ordering is forced: the padding must land
first, and the invariant must be *checked* rather than assumed — see the design.

## S6. Refusing the XPC send is survivable for 13 of 17 — and Risk R2 fired

**This is the measurement that decides Task 3's posture** (design: refuse vs proxy). The probe returns
a chosen mach error in `x0` for any `mach_msg2` carrying `MACH64_SEND_MQ_CALL`, writes nothing to
guest memory, and lets the run continue. Nothing was appended to the trace: this measures *guest
behaviour*, not replay.

### The headline

`/bin/date` **completes and prints the correct date**, exit 0, after exactly one refused XPC send:

```
[xpcprobe] refuse #1 pc=0x1804adc34 id=0x4000010f dest=0x1403 reply=0x2003 send=248 rcv=248
           -> x0=0x10000003
Sat Aug 29 18:39:15 EDT 2026
```

libxpc takes a no-service path. This is the *preferred* posture from the design, and it works.

### All 17, under `MACH_SEND_INVALID_DEST` (0x10000003)

| outcome | count | binaries |
|---|---|---|
| survived the refusal | **13** | aa, afktool, AssetCacheManagerUtil, avmediainfo, bioutil, chfn, csrutil, dddiagnose, dserr, bash, cal, date, zsh |
| new wall | **4** | automationmodetool, desdp, dyld_info, flex |

Every one sends **exactly one** XPC message before the refusal, so the shape is uniform up to this
point. "Survived" means the recording completed; the exit codes vary (0, 1, 64, 77, 139, 201, 205,
255) and were not compared against native runs — `date`, `bash`, `cal` and `zsh` exit 0, and `date`
was verified to print correct output.

**Risk R2 fired.** The 17 do *not* stay uniform past the refusal.

### The new wall is a guest `brk`, and no refusal code avoids it

```
RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED)
              pc=0x18035f084 elr=0x1804af110
```

`EC=0x3c` is BRK-instruction execution; `ISS=0x1` is `brk #1` — a *deliberate* guest abort, not a
fault. All four binaries land on the identical pc.

The four were re-run against **seven** refusal codes. None survives:

| code | automationmodetool | desdp | dyld_info | flex | date |
|---|---|---|---|---|---|
| `MACH_SEND_INVALID_DEST` 0x10000003 | brk (1 refusal) | brk (1) | brk (1) | brk (1) | **ok** |
| `MACH_SEND_TIMED_OUT` 0x10000004 | brk (3) | brk (5) | brk (5) | brk (5) | **ok** |
| `MACH_SEND_INVALID_REPLY` 0x10000009 | brk (3) | brk (5) | brk (5) | brk (5) | **ok** |
| `MACH_SEND_INVALID_RIGHT` 0x1000000a | brk (1) | brk (1) | brk (1) | brk (1) | **brk** |
| `MACH_RCV_TIMED_OUT` 0x10004003 | brk (3) | brk (5) | brk (5) | brk (5) | **ok** |
| `MACH_RCV_PORT_DIED` 0x10004009 | brk (3) | brk (5) | brk (5) | brk (5) | **ok** |
| `MACH_MSG_SUCCESS`, no reply written | brk (3) | brk (5) | brk (5) | brk (5) | **ok** |

Two things this table settles that reasoning would not have:

1. **The code is not the problem.** Seven different refusals, including "success with no reply", reach
   the same `brk` at the same pc. The four binaries need a *real* reply, not a better error.
2. **The code still matters.** `MACH_SEND_INVALID_RIGHT` is the one refusal that breaks `date`, which
   every other code survives. Picking a refusal at random would have looked like a much worse result.
   `MACH_SEND_INVALID_DEST` is also the only code (with `INVALID_RIGHT`) under which the guest gives
   up after **one** send rather than retrying 3–5 times, which is the behaviour a box with no
   message-queue receivers should produce.

**Posture chosen: refuse, with `MACH_SEND_INVALID_DEST`.** Deterministic, no host contact, no guest
reply port handed to a real daemon, symmetric by construction — and it is what 13 of the 17 tolerate.
The remaining 4 are Risk R3 materialising exactly as the design predicted, and are parked at the wall
named above rather than forcing the proxy design for a quarter of the set.

### Two negative results worth recording

**The wall's addresses could not be symbolicated**, so it is named by address, not by function. The
in-tree symbolicator returns the address unchanged for all three (`0x18035f084`, `0x1804af110`,
`0x1804adc34`) — the shared-cache symbol wall, whose LINKEDIT pages these runs never faulted.

**And `dyld_info -all_dyld_cache -segments` cannot substitute for it.** It prints `unslid-addr` for
most cache images but `load-offset` for others — including `/usr/lib/system/libsystem_kernel.dylib`,
which is precisely the image a trap pc lands in. Attributing addresses from that listing therefore
silently assigns them to whichever image happens to precede them, and the attributions it produced
here (`libc++abi + 0x1bc34` for a `mach_msg2` site) are wrong on their face. Recorded so the next
attempt does not repeat it.
