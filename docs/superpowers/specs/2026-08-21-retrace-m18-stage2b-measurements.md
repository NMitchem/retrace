# M18 Task 4 (t10) — measuring what is behind the Stage 2a wall

> **Relocated by Task 5 (t11).** This document was first committed (t10, `62f7491`) at
> `.superpowers/sdd/2026-08-20-retrace-m18-workq/stage2b-measurements.md`, inside a tree whose
> `.gitignore` is `*` — it had to be `git add -f`ed and was the only tracked file in that tree.
> Its raw artifacts (`task-4-raw-trace.err`, `task-4-raw-trace-rerun.err`,
> `task-4-raw-stdout.out`, `task-4-raw-stdout-rerun.out`) stay UNTRACKED in
> `.superpowers/sdd/2026-08-21-retrace-m18-workq-stage2a/`, which is what "this task's
> `.superpowers/sdd/` directory" below refers to.

Measured 2026-08-21 on `m18-workq-stage2a` at `8f8331e` (M18 t9 — 367/368 emulated, `WQOPS_QUEUE_REQTHREADS`
still a fail-loud `panic!`), macOS 26.x / Apple Silicon.

Two independent recorder runs, both with the `WQOPS_QUEUE_REQTHREADS` arm of `guest_workq_kernreturn`
temporarily replaced with `0` (the Task 4 brief's throwaway permissive stub — reverted before this
document was committed; see "Stub removal" below) and both run under a `perl -e 'alarm 120; exec @ARGV'`
wrapper in place of the brief's `timeout 120` (`timeout(1)` does not exist on this machine — this
substitution was ruled by the controller pre-dispatch, not by this task). Streams separated
(`RETRACE_TRACE=1 ... record-dyn "$G" -o out.bin >out.out 2>out.err`), per the brief's Step 3 note that
merging them (as Task 6 did) interleaves guest stdout into the trace.

| | Run A | Run B |
|---|---|---|
| raw trace | `task-4-raw-trace.err` | `task-4-raw-trace-rerun.err` |
| dispatched traps | 258 | 254 |
| exit code | not independently captured for this run | **142** (SIGALRM — perl's alarm fired; the run hung) |
| guest stdout | 0 bytes | 0 bytes |
| last line | `[trap] num=-36 ... pc=0x1804adbb0` | `[trap] num=-36 ... pc=0x1804adbb0` |

Run A's exit code was not captured by the process that ran it (the first two dispatch attempts on this
task died mid-run on a transient server-side error before recording it; the raw trace survived because
the controller preserved it out of `/tmp` before the agent process exited). Run A's trace is not
evidence against a 142: it ends on the identical trap, at the identical `pc`, with identical structure
(mach_msg2 reply received, then the blocking trap, then nothing — no panic, no `RECORD ERROR`, file
just stops), which is what a mid-syscall kill looks like on both runs. Treat Run A's exit code as
**unmeasured, not as a different outcome from Run B's**.

The "guest stdout: 0 bytes" row is backed by a preserved artifact for each run: `task-4-raw-stdout.out`
(Run A) and `task-4-raw-stdout-rerun.out` (Run B), both in this task's `.superpowers/sdd/` directory,
both confirmed 0 bytes (`wc -c`).

---

## 1. Headline: `dispatch_semaphore_wait` lowers to a mach semaphore trap, not a ulock

This is a **negative answer to the specific question the Stage 2a spec section poses in "The
measurement Stage 2a owes Stage 2b"**
(`docs/superpowers/specs/2026-08-20-retrace-m18-workq-design.md:416-418`): "whether
`dispatch_semaphore_wait` lowers to a `__ulock_wait` (515), a mach semaphore trap, or a `mach_msg2`
RPC." (**Not** the spec's numbered "Open questions for implementation planning" item 5 — that item,
at line 303-305, is a different question, about whether the *parked-worker wake* needs to distinguish
"work available" from "thread requested." This document's first commit mislabeled the citation as
"open question 5"; corrected here.) Grepping both raw traces for the ulock/semaphore family
(`grep -an 'num=515\|num=-3[0-9]\|semaphore'`):

Run A:
```
189:[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
393:[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
411:[trap] num=-36 (0xffffffffffffffdc) pc=0x1804adbb0 args=[0x1403,0x200000003,0x2800001513,0x110300000203,0xd5a00000000,0x110300000000]
```

Run B:
```
189:[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
389:[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
407:[trap] num=-36 (0xffffffffffffffdc) pc=0x1804adbb0 args=[0x1403,0x200000003,0x2800001513,0x180300000203,0xd5a00000000,0x180300000000]
```

`num=515` (`__ulock_wait`) does not appear **anywhere in either trace** — confirmed directly:
`grep -ac 'num=515' task-4-raw-trace.err` and the `-rerun` file both print `0`.

**`num=-36`'s name is an inferred attribution, not a verified one — labelled as such, the way t8/t9
label the `workq_kernreturn` opcode names.** The *measurement* is the raw number, `num=-36`. Its
commonly-cited name, `semaphore_wait_trap`, comes from XNU's public `osfmk/mach/syscall_sw.h`, which
this task did not check against an actual copy on this machine (the earlier draft of this document
claimed SDK verification via a grep for `SYS_gettimeofday`/`SYS_getentropy` in
`sys/syscall.h` — those are unrelated **BSD** syscall constants in a different table, not Mach trap
numbers, and that grep verifies neither `-36` nor `-33`; that claim is withdrawn here). Retrace's own
`crates/retrace-core/src/lib.rs:29-33` names `-10`/`-12`/`-14`/`-15`/`-47` as
`MACH_VM_ALLOCATE`/`MACH_VM_DEALLOCATE`/`MACH_VM_PROTECT`/`MACH_VM_MAP`/`MACH_MSG2` from the same Mach
trap table, which is consistent with `-36` sitting in the same table, but does not by itself establish
`-36`'s specific name — it is supporting context, not a citation. Both runs end on this exact trap, at
this exact `pc`, with the exact same first argument (`0x1403` — see §3, this is the port name the
immediately-preceding `semaphore_create` reply minted), and nothing follows it — those are the
measured facts, and they hold regardless of what `-36` turns out to be called: the sequence is
"forward a `mach_msg2` that mints a port, then forward a trap carrying that same port number, then
hang forever," independent of whichever kernel primitive `-36` names.

**So: `dispatch_semaphore_create`/`dispatch_semaphore_wait` do not lower to a `__ulock_wait`/`__ulock_wake`
pair at all.** They lower to `semaphore_create` (a `mach_msg2` RPC, msgh_id 3418) followed by a raw
Mach trap, `num=-36` (attributed, unverified on this machine, to `semaphore_wait_trap`), which is a
**different kernel object with a different wake primitive** — by the same unverified attribution,
`semaphore_signal_trap`, `num=-33` — never reached in either trace; nothing signals it. This
conclusion does not depend on the names being right: what's measured is that the blocking trap is a
raw Mach trap keyed on a mach-port-namespace value (`0x1403`), not `num=515`, and that is sufficient
on its own to establish the "not a ulock" finding below.
Stage 2b's park/wake seam **cannot** reuse the `pthread + 0x34` address-equality correlation M14 and
M17 built the thread-blocking model on ("Guest threads" / "Signals are per-thread too" in
`CLAUDE.md`): that correlation is specific to `__ulock_wait`/`__ulock_wake`'s address argument, and no
`__ulock_wait` call exists on this path to correlate on. Whatever Stage 2b does for this parking
primitive needs a mach-semaphore-shaped mechanism (most likely correlating on the semaphore's port
name, `0x1403` here, which is exactly analogous to the `pthread + 0x34` address but is a **port name in
retrace's own IPC space**, not a guest memory address — a different kind of value to key a scheduler
off of).

## 2. How the run ends: a real hang, not a crash

Neither trace contains a `panic`, a `RECORD ERROR`, or a `guest crashed:` line (`grep -an 'panic\|RECORD
ERROR\|EXIT=' <file>` returns nothing in either). Both files simply **stop** immediately after the
`num=-36` line — verified byte-for-byte on Run A: the file's last 4 bytes are `5d 0a` (`]` newline)
closing that trap's `args=[...]`, with nothing after it. This is what an external `SIGALRM` kill looks
like: the vCPU-driving thread was inside a blocking host syscall (see below) when the alarm fired, so
nothing else was ever printed.

**The mechanism, read against the code**: `num=-36` (named `semaphore_wait_trap` per §1's attribution,
unverified on this machine) has **no dedicated arm** in
`record_box`'s dispatch (unlike `MACH_VM_ALLOCATE`/`_DEALLOCATE`/`_PROTECT`/`_MAP`/`MACH_MSG2`, and
unlike `SYS_WORKQ_OPEN`/`SYS_WORKQ_KERNRETURN`, which now have both dedicated arms *and* a fail-loud
assert at `crates/retrace-core/src/lib.rs:1012` guarding the generic-forward **BSD** arm against them —
a guard `num=-36` cannot reach, since Mach traps are negative and that BSD arm sits downstream of a
positive-only match). Being negative, `num=-36` instead falls into the generic **Mach-trap** arm at
`crates/retrace-core/src/lib.rs:531` (`Stop::Syscall { num, args } if (num as i64) < 0`), which forwards
at line 532, `b.forward_and_diff(num, args)`, and issues the real `semaphore_wait_trap` syscall **in
retrace's own process** against the semaphore port `0x1403` that the immediately-preceding forwarded
`semaphore_create` minted in retrace's own IPC space. Nothing in retrace's process will ever call
`semaphore_signal` on that port — no worker thread was created (`WQOPS_QUEUE_REQTHREADS` is stubbed to
return `0` rather than actually starting anything for this measurement, and even un-stubbed, Stage 2a's
`panic!` never got that far in Task 6's run) — so the forwarded trap blocks forever. This is the same
class of hazard `CLAUDE.md` documents for `bsdthread_create`/`workq_open`/`workq_kernreturn`
("forwarding … is not merely wrong but whole-process fatal") except milder in *kind*: it does not crash
the recorder (no new host thread, no jump through a null function pointer), it just **wedges the single
thread retrace has** inside an unbounded host blocking call. Confirmed as real, not theoretical: this is
what the `perl alarm` wrapper caught (exit 142) — without it, per the brief's own Step 3 note and the
controller's ruling log, this run would have hung silently with no timeout mechanism available on this
machine.

## 3. The `mach_msg2` at `pc=0x1804adc34`

`pc=0x1804adc34` is **not unique to one call** — it is the shared `libsystem_kernel.dylib` trampoline
address for every `mach_msg2_trap`. Counted directly (`grep -ac 'pc=0x1804adc34'`), it recurs **12
times per run**, across **10 distinct `msgh_id` values** (identical set in both runs): `200`, `206`,
`3405`, `3409`, `3410`, `3418` (twice), `4811` (twice), `4822`, `8000`, `8001`. (An earlier draft of
this document named only 3410/3405/3418 as if that were the complete list; it understated the count.
The point stands regardless — the `pc` is a shared trampoline, not evidence of anything specific to
this call — but the earlier enumeration was not what the trace contains.) The specific occurrence the
brief means is the one **immediately after** `WQOPS_QUEUE_REQTHREADS`
(`num=368 args=[0x20,...]`) — the one Task 6's run could only see truncated. Identified by matching the
first three (untruncated) args against Task 6's truncated line
(`stage2-measurements.md` §3: `args=[0x27ff6e0,0x200000003,0x2800001513`): both runs here reproduce that
exact prefix.

**Full args (Run A)**, verbatim:
```
[trap] num=-47 (0xffffffffffffffd1) pc=0x1804adc34 args=[0x27ff6e0,0x200000003,0x2800001513,0x110300000203,0xd5a00000000,0x110300000000]
```

**Full args (Run B)**, verbatim — identical except the reply-port name (`0x1803` vs `0x1103` in arg 3,
carried again in arg 5 as the low bits of a packed word), which is expected per-run mach-port-namespace
nondeterminism, not a divergence in what the guest is doing:
```
[trap] num=-47 (0xffffffffffffffd1) pc=0x1804adc34 args=[0x27ff6e0,0x200000003,0x2800001513,0x180300000203,0xd5a00000000,0x180300000000]
```

**The decoded `[mach_msg2]` line** (`RETRACE_TRACE=1`'s decoder), Run A:
```
[mach_msg2] msgh_id=3418 dest=0x203 reply=0x1103 options=0x200000003 bits=0x1513 send_size=40 rcv_size=48
  send+000: 13 15 00 00 28 00 00 00 03 02 00 00 03 11 00 00
  send+010: 00 00 00 00 5a 0d 00 00 00 00 00 00 01 00 00 00
  send+020: 00 00 00 00 00 00 00 00
[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
[mach_msg2] host ret=0x0 err=false
  reply@0x27ff6e0+000: 00 12 00 80 28 00 00 00 00 00 00 00 03 11 00 00
  reply@0x27ff6e0+010: 00 00 00 00 be 0d 00 00 01 00 00 00 03 14 00 00
  reply@0x27ff6e0+020: 00 00 00 00 00 00 11 00 00 00 00 00 08 00 00 00
  reply@0x27ff6e0+030: 00 00 03 00 00 00 00 00 c0 14 06 00 00 00 00 00
  reply@0x27ff6e0+040: 40 f7 7f 02 00 00 00 00 7c a9 4f 80 01 00 00 00
  reply@0x27ff6e0+050: 40 f7 7f 02 00 00 00 00 b0 04 35 80 01 00 00 00
  reply@0x27ff6e0+060: 80 f7 7f 02 00 00 00 00 70 83 33 80 01 00 00 00
  reply@0x27ff6e0+070: 80 ec 6f ec 01 00 00 00 01 00 00 00 00 00 00 00
  reply@0x27ff6e0+080: 00 00 00 00 00 00 00 00 ff ff ff ff ff ff ff ff
  reply@0x27ff6e0+090: 80 14 06 00 00 00 00 00 c0 14 06 00 00 00 00 00
  reply@0x27ff6e0+0a0: b0 f7 7f 02 00 00 00 00 d0 8a 33 80 01 00 00 00
  reply@0x27ff6e0+0b0: 00 00 00 00 00 00 00 00 a8 f8 7f 02 00 00 00 00
  reply@0x27ff6e0+0c0: 00 41 44 ec 01 00 00 00 58 40 44 ec 01 00 00 00
  reply@0x27ff6e0+0d0: 10 f8 7f 02 00 00 00 00 38 05 00 00 01 00 00 00
  reply@0x27ff6e0+0e0: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
  reply@0x27ff6e0+0f0: e0 b7 6f ec 01 00 00 00 00 00 00 40 00 00 00 00
```
Run B's is byte-identical apart from `msgh_local_port`/`reply=` (`0x1803` in place of `0x1103`,
consistent throughout the header and mirrored in the reply bytes) — again per-run port-namespace
noise, not a content difference. `msgh_id 3418` is already a named, forward-allowlisted route
(`crates/retrace-core/src/machmsg.rs:50`, `Route::Forward("semaphore_create")`); it is not part of the
wall. **Both runs confirm the reply carries a minted semaphore port name of `0x1403`** — visible at
reply offset `+01c` (`03 14 00 00`, little-endian `0x1403`) — which is exactly the value passed as
arg 0 of the `semaphore_wait_trap` that follows it (§1). This is the guest asking for the mach
semaphore's kernel object and then immediately blocking on it.

## 4. Every trap after that `mach_msg2`, in order, to the end

Both runs: exactly **one** more trap follows, and then the trace stops.

```
[trap] num=-47 (0xffffffffffffffd1) pc=0x1804adc34 args=[...]        <- semaphore_create (§3)
[mach_msg2] msgh_id=3418 ... send bytes ...
[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
[mach_msg2] host ret=0x0 err=false
  reply bytes ...
[trap] num=-36 (0xffffffffffffffdc) pc=0x1804adbb0 args=[0x1403,0x200000003,0x2800001513,<reply-port-derived word>,0xd5a00000000,<reply-port-derived word>]
<end of file — nothing after this line in either run>
```

There is no trap between the `mach_msg2` reply and `num=-36`, and no trap after `num=-36`. This is a
two-step sequence — mint the semaphore, then block on it — not a longer chain the earlier truncated
measurement might have suggested.

## 5. No new `workq_kernreturn` opcode

```
$ grep -aoE 'num=368 .*args=\[0x[0-9a-f]+' task-4-raw-trace.err | sort -u
num=368 (0x170) pc=0x1804af9f0 args=[0x20
num=368 (0x170) pc=0x1804af9f0 args=[0x400

$ grep -aoE 'num=368 .*args=\[0x[0-9a-f]+' task-4-raw-trace-rerun.err | sort -u
num=368 (0x170) pc=0x1804af9f0 args=[0x20
num=368 (0x170) pc=0x1804af9f0 args=[0x400
```

Exactly the two opcodes t8/t9 already implemented (`WQOPS_SETUP_DISPATCH=0x400`,
`WQOPS_QUEUE_REQTHREADS=0x20`), each firing exactly once per run (`grep -ac 'num=367'` → 1,
`grep -ac 'num=368'` → 2, both runs). **No new opcode appeared.** This measurement does not reach a
running workqueue thread (the stub returns `0` for REQTHREADS rather than starting one), so this is a
floor, not a ceiling, on the same terms `stage2-measurements.md` §2 already stated: opcodes a *running
worker* would issue on the park/return path are still unmeasured and still cannot be enumerated until
something makes a worker actually run.

## 6. Trap-count variance: fully reconciled to two already-known nondeterministic forwards, zero left over

258 (Run A) vs. 254 (Run B) — a difference of 4. Per the withdrawn lesson in
`stage2-measurements.md` §4 ("read count instability as evidence of a racing host thread" —
**formally withdrawn** by
`docs/superpowers/specs/2026-08-20-retrace-m18-workq-design.md:325-341`, "A correction to Task 6's
§4, before it propagates" — an earlier draft of this document misattributed the withdrawal to a
`CLAUDE.md` section called "A trap you must not fall into," which does not exist anywhere in this
repository; corrected here), this delta is not asserted to mean anything
until shown to exceed the baseline dyld guests already have from forwarded `gettimeofday`/`getentropy`.
Checked directly, and reconciled completely — every syscall whose count differs between the two runs,
and by exactly how much:

| syscall | Run A | Run B | delta |
|---|---|---|---|
| `num=116` (`gettimeofday`) | 22 | 19 | **+3** |
| `num=-15` (`MACH_VM_MAP`) | 17 | 16 | **+1** |
| `num=500` (`getentropy`) | 2 | 2 | 0 |
| `num=-24` | 3 | 3 | 0 |
| `num=-14` (`MACH_VM_PROTECT`) | 47 | 47 | 0 |
| `num=-12` (`MACH_VM_DEALLOCATE`) | 10 | 10 | 0 |
| `num=-47` (`MACH_MSG2`) | 12 | 12 | 0 |
| `num=367`/`num=368` (workq) | 1 / 2 | 1 / 2 | 0 |
| **all traps** | **258** | **254** | **+4** |

`+3` (`gettimeofday`) `+1` (`MACH_VM_MAP`, plausibly malloc heap-growth timing, itself downstream of the
same per-run allocation-order noise) accounts for **all 4** of the total difference. There is no
residual to attribute to anything else, let alone a racing host thread — and unlike Task 6's Stage 1
measurement, no host worker thread is created on this path at all (`WQOPS_QUEUE_REQTHREADS` is the
stub `=> 0`, not a real thread-spawning forward), so there is no racing thread available to blame even
speculatively. **The wall's location is the finding (`num=-36` at `pc=0x1804adbb0`, identical in both
runs); the count carries no information beyond baseline forwarded-syscall noise, and here that baseline
fully explains it.**

## 7. The final line of the log

Both runs: `[trap] num=-36 (0xffffffffffffffdc) pc=0x1804adbb0 args=[0x1403,...]` — a **clean trace
line**, not a panic, not a `RECORD ERROR`, not a mid-line truncation (contrast Task 6's Stage 1
measurement, whose tail was cut mid-`args=[...]` by a concurrent SIGSEGV in the recorder itself). The
file simply stops after this line. Combined with §2's read of `record_box`'s dispatch (no dedicated arm for
`num=-36` — it is caught only by the generic negative-trap arm at `crates/retrace-core/src/lib.rs:531`,
which forwards it) and Run B's confirmed `EXIT=142`, the
interpretation is: **the recorder hung inside a real, host-forwarded `semaphore_wait_trap` with nothing
in its own process ever able to signal it, and the external alarm killed it there.** This is not a
guest-side wall (the guest's own logic — `dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER)` — is
working as designed against a real kernel semaphore; nothing in the guest is wrong) and not a crash; it
is retrace's forwarding of a blocking primitive it has not yet built a park/wake seam for.

---

## Stub removal (brief Step 5)

`crates/retrace-box/src/lib.rs`'s `WQOPS_QUEUE_REQTHREADS` arm was reverted from the throwaway `=> 0`
back to its `panic!` via `git checkout -- crates/retrace-box/src/lib.rs`. Verification output is in the
Task 4 process report (`task-4-report.md`) rather than duplicated here, since this document is the
measurement deliverable and carries no code state.

## What this hands Stage 2b

1. **The park/wake seam is not a `__ulock_wait`/`__ulock_wake` problem for this primitive.**
   `dispatch_semaphore_wait` lowers to `semaphore_create` (mach_msg2, already forward-allowlisted) +
   a raw Mach trap, `num=-36` (attributed, unverified on this machine, to `semaphore_wait_trap`),
   currently unarmed and hazardous to forward. Stage 2b needs a `num=-36`/`num=-33` emulation pair
   (names attributed, not verified — see §1), keyed on the semaphore's port name (e.g. `0x1403` here)
   rather than a guest memory address, the same shape as `workq_open`/`workq_kernreturn`'s "emulate,
   never forward" rule but for a different object class.
2. **The `num=-36` trap must never reach `forward_and_diff`** — it is not whole-process-fatal the way
   forwarding `workq_open`/`bsdthread_create` is (no new host thread, no null-pointer jump), but it is
   whole-process-**hanging**, which is just as fatal to a recording. The same fail-loud-assert *shape*
   CLAUDE.md documents for the workq pair (`crates/retrace-core/src/lib.rs:1012`) is the template for a
   `num=-36`/`num=-33` guard — but not its *location*: that assert guards the generic **BSD** forward
   arm, which negative trap numbers never reach. Once Stage 2b has arms to guard for, the guard belongs
   inside or before the generic **negative-trap** arm at `crates/retrace-core/src/lib.rs:531`, next to
   wherever `num=-36`/`num=-33` end up being serviced.
3. **No new `workq_kernreturn` opcode to implement yet** — the only two measured (`0x400`, `0x20`) are
   already both emulated by t8/t9. Opcodes a running worker would issue on its park/return path remain
   unmeasured; they cannot be measured until a worker actually runs, which needs the semaphore seam (or
   a guest that doesn't need one) first.
4. **The port-name-vs-address distinction is structural, not incidental**: M14/M17's whole
   thread-blocking model keys on a **guest-memory address** (`pthread + 0x34`) because `__ulock_wait`'s
   correlating value lives in guest memory the box already tracks. A mach semaphore's correlating value
   (`0x1403`) is a **port name in retrace's own host-side IPC space**, minted by a forwarded call and
   never written into guest memory as such. Whatever Stage 2b builds needs to key off values in a
   different address space than M14/M17's mechanism did — this is worth designing deliberately rather
   than trying to force-fit the existing correlation.
