# M23-xpcpipe design — the guest's first real XPC connection

Companion to `2026-08-29-retrace-m23-xpcpipe-measurements.md`. Read that first; every claim here
rests on it.

## What this milestone is

The milestone began as "service msgh_id 412". Measurement (S4) shrank that to a **one-line**
allowlist entry and simultaneously revealed that it unlocks **zero** binaries: all 17 immediately
converge on a real XPC message-queue send. So the milestone is scoped to the wall that is actually
load-bearing:

> **Let a guest that opens an XPC connection record and replay — either by being answered, or by
> being told faithfully that there is nobody there.**

Three things must land, in this order. Only the third is a capability; the first two are the
prerequisites measurement proved are required.

## Task 1 — Unmask: make trampoline padding trap, and *check* the fall-through invariant

**The defect (root-caused, S1):** vector-slot padding is zero, i.e. `UDF #0`. A fall-through executes
it at EL1, which overwrites `ELR_EL1`/`SPSR_EL1` and reports `pc=0x4204` — an address in retrace's own
trampoline that has nothing to do with the guest's actual exception. **Trampoline padding must trap
back to the VMM, never be an undefined instruction.** That is independently correct regardless of S2's
unresolved mechanism.

**The trap this creates, and the reason for the second half of the task.** Making the padding a `hvc`
means the dispatch re-reads the still-valid `ESR_EL1` and redoes the ERET — the run *self-heals*.
That is exactly the shape of failure a determinism oracle cannot see: record and replay would agree
with each other while the anomaly silently vanished. It is the M18 semaphore lesson
(`semaphore_wait_trap` asserts rather than dropping a wake) applied to a new case.

**So the recovery must be counted, not silent.** `Box_` carries a fall-through counter; the count is
compared between record and replay and **fails loud on mismatch**. This converts S2's unexplained HVF
behaviour from an *assumed* invariant into a *checked* one, which is what the divergence oracle exists
to do — and it is the reason this milestone can proceed honestly without first resolving whether the
cause is a non-trapping `hvc` or a coalesced exit.

**Symmetry:** the recovery lives in `Box_::run()`, below the trace (rule 2), so it fires identically
on both sides and nothing is recorded. The *count* is the checked quantity, not a landmark — adding an
`Event` variant would renumber every landmark, which `Event::Sched`'s removal already established as
forbidden.

**Risk R1 — the count is per-run, not per-position.** A mismatch tells us determinism broke but not
where. Accepted: the alternative (recording positions) is a trace-format change for a phenomenon we
expect to be empty in every gate guest. If R1 ever fires, that is the moment to spend the format bump.

## Task 2 — `host_get_special_port` (msgh_id 412)

One entry in `FORWARD_ALLOWLIST` (S4, verified working). Forward-and-record is the correct posture and
is precedented twice over: `host_get_clock_service` (206) already forwards to the same host port and
already returns a **port**; `task_info` (3405) established forward-and-record for a reply whose
contents are nondeterministic. Replay applies the recorded writes, so the guest sees identical bytes.

**Why not synthesize it** (the 3409/3410 posture): those two are *refused* forwards because forwarding
them would hand over or overwrite retrace's own real launchd / debug-control port. `which=1` is
`HOST_PORT` — the host name port the guest **already holds**, since it is the `dest` of this very
message. Forwarding gives back what the guest already has; there is nothing to protect.

**Guard it.** The allowlist is keyed on msgh_id alone, so decode and assert `(node, which) ==
(-1, 1)`, mirroring how 3409 asserts `which == 4` and 3410 asserts `which == 10`. S3 measured that
every one of the 17 sends exactly that pair; a different special port must fail loud rather than be
forwarded blind, because HOST_PRIV_PORT (2) *would* be a right worth protecting.

## Task 3 — the XPC pipe: answer it, or refuse it faithfully

The real wall. All 17 send a 248-byte `"@XPC"` message with `MACH64_SEND_MQ_CALL` to a service port,
carrying a reply port and blocking for the response.

**Two postures, and Task 3a must measure before choosing:**

- **Refuse.** Return a failure so libxpc takes its no-service path. This is retrace's established
  answer for "a system service that cannot exist inside the box": `vm_reclaim` → `KERN_NOT_SUPPORTED`,
  `__mac_syscall(Sandbox)` → success/unsandboxed. Deterministic, no host contact, symmetric by
  construction. **Preferred if the guest tolerates it.**
- **Proxy.** Forward to the real daemon. Faithful, but drags a live system service and its
  nondeterministic replies into the trace, and hands a guest reply port to a real daemon. Only if
  refusal is not survivable.

**The measurement that decides it (Task 3a):** return a refusal and observe what libxpc does — take a
fallback, retry, or abort. That is a guest-behaviour question no amount of reasoning settles, and it is
the same shape as M2-xpcport's finding that the XPC-pipe wall was *small* once measured.

**Risk R2 — refusal may be survivable for some of the 17 and fatal for others.** They are uniform up
to this point (S3, S4) but need not stay uniform after it. The plan therefore re-sweeps all 17 rather
than generalising from one.

**Risk R3 — a further wall behind this one.** Near-certain; that was the shape of the entire M2 chain
(objc → PAC → TBI → mmapcommit → carveout → cpuid → bootstrap → xpcport → setport → taskinfo). The
milestone is honest about this: its gate is parked at whatever wall remains, per the honest-gate
discipline, and a milestone that parks a *new* gate for a capability it does not yet have has
regressed nothing.

## Gate

A new end-to-end gate, `xpc_e2e`, recording and replaying one of the 17 (`/bin/date` — smallest,
needs no padding, so it isolates Tasks 2–3 from Task 1). Parked `#[ignore]`d at the wall that remains
if Task 3 does not clear it, with the `#[ignore]` reason naming that wall precisely — never the
generic "XPC unsupported".

**Assert on the difference this work makes**, per the rule `protnone_rust_e2e` established: assert the
`"@XPC"` send was *serviced* (the specific route taken and its reply bytes), not merely that the
process exited 0 — a guest that never reached XPC would also exit 0.

## What this milestone explicitly does not do

- **Resolve S2's mechanism.** Task 1 makes it *checked*, not *explained*. The `spikes/*.c` probe
  distinguishing a non-trapping `hvc` from a coalesced HVF exit is real work and is deliberately not
  on this critical path.
- **Touch csh/tcsh (the `lib.rs:1103` panic) or `ps` (divergence).** Three binaries, unrelated causes,
  and folding them in would make the milestone's result unreadable.
