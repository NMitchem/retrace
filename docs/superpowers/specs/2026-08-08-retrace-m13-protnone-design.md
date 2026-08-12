# retrace M13-protnone — the guard page actually guards

M12 closed its boundary and named the next one in the same breath: `PROT_NONE` is not enforced, so a
page the guest protects stays readable and writable. M13 makes it fault.

> **Read the "Measured corrections" section below before the rest of this spec.** Two of the claims
> M12 handed forward — that libstd mmaps its guard `PROT_NONE`, and that a Rust stack overflow
> "grows straight through" that guard — were inherited rather than measured, and executing this
> plan's measurement-first tasks falsified both. The mechanism this spec designs is unchanged and
> correct; its *headline* moved, and two call sites swapped billing.

The milestone is smaller than M12's Status section feared, and the reason is worth stating up front:
**three of the four mechanisms it needs already exist and are already tested.** The fault route
(`Stop::Fault` → disposition → deliver-or-crash) is M6's and M12's, unchanged. The page-table
attribute stamper (`set_region_exec_attr`) already takes an arbitrary attribute and already promotes
an L2 block to an L3 table on demand. The TLB invalidation a revocation requires is M9's guest-side
`flush_guest_tlb` oracle, which until now has had no caller that genuinely needed it. What is missing
is a fourth thing: an attribute that denies EL0 access, and the four call sites that install it.

## The problem, precisely

**`Box_::guest_mprotect` (`lib.rs:1894`) is the live wrong answer, and it states its own debt.** It
discards `_prot`, re-`hv_vm_protect`s the stage-2 range to `RWX`, and documents: *"A finer prot map
lands if a guest ever needs a fault from this."* A guest does — measured: `hello_rust` issues three
`mprotect(addr, 0x4000, PROT_NONE)` calls, one of which is libstd's stack-overflow guard at
`0x2004000`. All three silently do nothing today.

`map_mmap_region` (`lib.rs:1774`) accepts `prot` and consults exactly one bit (`PROT_EXEC`, only to
decide block-exclusive placement), so a `PROT_NONE` mmap is equally unenforced. Its doc comment cites
`install_main_guard` as the motivating case, and that citation is **half wrong**: libstd does mmap
`MAP_FIXED` at the guard address, but with `PROT_READ|PROT_WRITE` — the `PROT_NONE` arrives from the
following `mprotect`. So this path is real but is *not* what makes libstd's guard a guard.

`mach_vm_protect` is a third path and the weakest: its record arm (`retrace-core/src/lib.rs:348`) and
replay arm (`:1123`) return `KERN_SUCCESS` without calling into the box at all. Measured, no dynamic
gate ever passes it `prot == 0`, so wiring it is dormant today and exists to keep the three paths
from disagreeing.

`mach_vm_protect` is a third path and the weakest: its record arm (`retrace-core/src/lib.rs:348`) and
replay arm (`:1123`) return `KERN_SUCCESS` without calling into the box at all.

## Measured corrections to this spec (Tasks 1-2, 2026-08-08)

Three claims below were written from inherited documentation rather than measurement. Executing the
plan's measurement-first tasks falsified them. They are corrected here rather than quietly edited
away, because what the spec believed is part of the record.

1. **Darwin raises `SIGBUS`/`BUS_ADRALN` for a `PROT_NONE` access, not `SIGSEGV`/`SEGV_ACCERR`**
   (`spikes/protnone.c`, reproduced independently at two optimization levels). `signal_of_esr`'s
   permission row was the Linux answer and had never been reached by a running guest in six
   milestones. An unmapped address still raises `SIGSEGV`, so M6's classification is untouched.

2. **libstd does NOT mmap its guard page `PROT_NONE`.** It issues
   `mmap(guard, page, PROT_READ|PROT_WRITE, MAP_FIXED|MAP_ANON)` and *then*
   `mprotect(guard, page, PROT_NONE)`. `hello_rust` records **zero** `PROT_NONE` mmaps.
   So **M13-sites' case 2 (`guest_mprotect`) is the path that makes libstd's guard fault**, and
   case 1 (`map_mmap_region`) is not on libstd's path at all — the reverse of what this spec said.
   Both remain in scope; only their billing was wrong. M12's Status section and
   `crates/retrace-box/src/lib.rs:35-37` carry the same error and are corrected by this milestone.

3. **The stack-overflow headline is unachievable in M13, and the reason is pre-existing measured
   debt.** libstd puts its guard at `pthread_get_stackaddr_np() - pthread_get_stacksize_np()` =
   `DYN_STACK_TOP - 0x7fc000` = `0x2004000`, which is **7.73 MiB below** the real 256 KiB stack
   bottom. `crates/retrace-box/src/lib.rs:35-53` already documents this as **M8-stack spec risk R3**,
   and already measured both candidate fixes and rejected them: backing a full 8 MiB cost **~1.7x on
   `hello_rust` and far worse across the dyld suite**, and `getrlimit` cannot move the subtrahend
   because libpthread ignores the reply (answering `0x10000000` instead of `0x40000` left the address
   bit-identical). Worse than "the guard does not guard": an overflow today runs into unbacked IPA,
   takes a stage-2 fault, and reaches the fatal `describe_stop` path — a retrace hard error rather
   than a guest-visible signal.

   **Ruling:** M13's headline becomes a full-`std` Rust guest that `mprotect`s a page `PROT_NONE` and
   faults on it. The stack-overflow gate is written and **parked `#[ignore]`d at the R3 wall**, with
   the wall named in the test and the README — honest-gate discipline, and a precisely diagnosed
   target for the milestone that fixes R3.

Two risks retired by measurement rather than by argument: **`mach_vm_protect` never carries
`prot == 0`** (47 calls across `hello_rust`, only `0x1`/`0x3`/`0x13`), so M13-sites case 3 is a
dormant change; and **`commit_reserved_page` fires zero times** in `hello_rust`, `hello_dyn`, and
`jq` across three runs each — with the diagnostic proven live against the static `reservecommit`
fixture, so the zero is a measurement and not a broken probe. The deviation this spec retains has
**no observed cost in any dynamic gate**.

## Verified facts (this host, HEAD `45b0821`, 2026-08-08)

Read out of the tree rather than recalled. Each is load-bearing for a decision below.

### The stage-1 attribute set (`crates/retrace-box/src/lib.rs:326-334`)

```
A_COMMON  = 0x400 (AF) | 0x300 (SH inner)      AttrIndx 0 = Normal WBWA
UXN       = 1 << 54    (EL0 execute-never)
PXN       = 1 << 53    (EL1 execute-never)
ATTR_DATA  = A_COMMON | 0x40 (AP EL0-RW)      | UXN | PXN
ATTR_CODE  = A_COMMON | 0xC0 (AP RO both ELs)       | PXN
ATTR_TRAMP = A_COMMON | 0x80 (AP EL1-RO, EL0 none) | UXN
```

`A_COMMON` sets **AF**. That is the fact that makes the design work: an AP=`0b00` page yields a
*permission* fault, not an access-flag fault, so the DFSC lands in the row M13 wants.

### The fault-to-signal table already covers permission faults

`crates/retrace-arch/src/lib.rs:305-319`:

```
0x04..=0x07 => (SIGSEGV, SEGV_MAPERR)   translation fault, levels 0..3
0x08..=0x0b => (SIGSEGV, SEGV_ACCERR)   access-flag fault
0x0c..=0x0f => (SIGSEGV, SEGV_ACCERR)   permission fault
```

with a golden test pinning `signal_of_esr(0x9200_000f) == (SIGSEGV, SEGV_ACCERR)`.

**No guest has ever produced that ESR.** Every fault M6, M11, and M12 recorded was a *translation*
fault — a wild pointer to an unmapped address, DFSC `0x04..0x07` — where SIGSEGV is correct on both
Linux and Darwin. The permission row exists, is unit-tested, and has never been exercised by running
code. M13 is the first milestone to reach it. See R1.

### Both protection syscalls are already dispatched on both sides

| call | record | replay |
|---|---|---|
| `mprotect` (74) | `retrace-core/src/lib.rs:236` → `guest_mprotect` | `:1150` → `guest_mprotect` |
| `mach_vm_protect` (−14) | `:348` → nothing | `:1123` → nothing |

Trap layout, from the constant's own comment (`retrace-core/src/lib.rs:26`):
`_kernelrpc_mach_vm_protect_trap(target, addr, size, setmax, prot)` — so `args[1]`, `args[2]`,
`args[4]`.

This is why M13 needs no new trace event and no new replay mirror: the syscalls are already recorded
`Event::Syscall`s, and replay already re-dispatches them into the same box methods.

### Checkpoint restore cannot carry a stale TLB

`Box_::from_checkpoint` (`lib.rs:2814`) creates a **fresh `Vm` and `Vcpu`**, then re-maps every
backing out of `state.mem` — which includes the L2 and L3 page tables, since they are backings at
fixed IPAs. M9's own comment at `:2857` already relies on precisely this: *"The page table entry
(`ATTR_TRAMP`) is already restored as part of `state.mem`."*

So attribute stamps survive a checkpoint for free, and no restored session can hold a stale
translation. Stale-entry risk exists only for **in-run** attribute changes — exactly what
`flush_guest_tlb` addresses.

### `BoxState` already carries a range table of this shape

`lib.rs:553` — `pub reservations: Vec<(u64, u64)>`. M13's `noaccess` map is the same shape and gets
the same treatment at every site `reservations` gets one. Mirror it; do not invent a second pattern.

## Unmeasured — the plan's first task must measure these before any code is written

1. **Which signal does Darwin raise for a `PROT_NONE` access, with what `si_code`?** retrace's table
   says `SIGSEGV`/`SEGV_ACCERR`. Darwin's `ux_exception` translates `EXC_BAD_ACCESS` by code —
   `KERN_INVALID_ADDRESS` → `SIGSEGV`, everything else including `KERN_PROTECTION_FAILURE` →
   `SIGBUS` — and libstd's comment on this exact code path says *"This ensures SIGBUS will be raised
   on stack overflow."* Two sources disagree with the code. Do not guess. (R1)
2. **Where does libstd's guard page actually land relative to the guest stack backing?** If it falls
   outside, `protect_none`'s must-be-backed assert fires and the milestone stalls at task 1. (R2)
3. **Does `mach_vm_protect` fire at all in the four dynamic gates, and with what `prot`?** If dyld
   ever protects something to `PROT_NONE`, honoring it is a live behavior change rather than a
   dormant one. (R4)
4. **How many `commit_reserved_page` hits occur per dynamic gate?** This is the quantification of the
   deviation retained below — a number for the Status section, not a hand-wave.

## The mechanism

### M13-attr — one new attribute

```rust
const ATTR_NONE: u64 = A_COMMON | 0x00 /*AP EL1-RW, EL0 none*/ | UXN | PXN;
```

An EL0 load or store to such a page takes a stage-1 permission fault at leaf level: DFSC `0b001111`
= `0x0f`, which is the row `signal_of_esr` already claims. Load and store differ only in the ISS
`WnR` bit, which the table does not index on — so both land in the same row. Open question 1 asks the
plan to confirm that against a live ESR rather than against this paragraph.

The alternative — marking the descriptor invalid, producing a *translation* fault — is rejected. A
`PROT_NONE` page is mapped and access-denied, which is `SEGV_ACCERR`/protection-failure semantics, not
`SEGV_MAPERR`/unmapped. Choosing the invalid descriptor would make a protected page indistinguishable
from an unmapped one at the fault, which is the distinction the whole milestone rests on.

### M13-split — the hardware separates committable from must-fault

M12's Status section anticipated needing *"a fault path that separates 'reserved and committable'
from 'reserved `PROT_NONE`, must fault'."* No such software gate is needed. The separation is
structural:

| | backing | exception route | `Stop` variant | serviced by |
|---|---|---|---|---|
| protected `PROT_NONE` page | always backed | stage-1 permission fault → EL1 trampoline → `hvc` | `Stop::Fault` | M12 disposition check |
| reserved-uncommitted page | never backed | stage-2 translation fault → EL2 directly | `Stop::Other` | `commit_reserved_page` |

Two different exception routes producing two different `Stop` variants. Nothing chooses between them
— for exactly the reason M6's fault arm provably cannot steal a demand-paging case, restated one
milestone later.

This holds only while the invariant **"no-access ⇒ backed"** does. The `mmap` path maintains it for
free: `guest_mmap` allocates a backing before it ever inspects `prot`. The single way to violate it
is `mprotect`/`mach_vm_protect` over a range inside a `mach_vm_map` reservation that was never
committed. No guest does this. M13 makes it **fail loud**, rather than silently doing nothing (a
plausible lie) or eagerly materializing a 24 GiB reservation (infeasible).

### M13-map — the protection state

`Box_` gains `noaccess: Vec<(u64, u64)>`: page-granular no-access extents, mirroring `reservations`
in shape, storage, `BoxState` treatment, and test accessor (`noaccess()`, alongside `reservations()`).

Two operations:

- **`protect_none(ipa, len)`** — page-round the range; assert every page in it is backed
  (M13-split's invariant); stamp `ATTR_NONE`; insert into the map; `flush_guest_tlb()`.
- **`unprotect(ipa, len)`** — stamp `ATTR_DATA`; subtract from the map; `flush_guest_tlb()`.

The TLBI is mandatory here in a way it has never been at an existing stamp site. Every current caller
of `set_region_exec`/`set_region_exec_attr` stamps a **fresh IPA the guest has never translated** —
the sign stub, the TLBI stub, a cache page, a fresh exec mmap — and each documents that as its
soundness argument. The guard page is inside the stack the guest is *running on*. M13 is the first
caller for which M9's oracle is a correctness requirement rather than an unused capability.

Note the direction of the danger: a missing flush leaves a stale **permissive** entry, so the guard
silently fails to guard — the same class of quiet wrong answer M13 exists to eliminate. `protrestore`
(below) is the gate that makes it loud.

### M13-sites — four call sites

1. **`map_mmap_region`** (`lib.rs:1774`) — `prot == 0` → `protect_none`. This is the headline path.
   It has **two** exits: the `place_fixed` contained-case early return at `:1794` and the normal
   fall-through at `:1802`. Both need the hook, or the function needs a single exit. The contained
   case is the one libstd takes.
2. **`guest_mprotect`** (`lib.rs:1894`) — `prot == 0` → `protect_none`; otherwise → `unprotect` when
   the range intersects `noaccess`, else today's behavior unchanged.
3. **`mach_vm_protect`** — both arms route to the same `guest_mprotect` with `(args[1], args[2],
   args[4])`. One implementation, so record and replay cannot drift.
4. **`guest_munmap`** (`lib.rs:1879`) — subtract the range from `noaccess` as it already does from
   `reservations`. The pages are gone; leaving them in the map would deny access to whatever is
   mapped there next.

**Only `prot == 0` is honored. Every other protection value keeps today's behavior**, and this is a
decision rather than an omission. dyld issues `mach_vm_protect` RW→RO during fixups and then writes
through the result; honoring the read-only bit would break the loader. The no-access case is what the
headline needs and is the only change with no blast radius on the four green dynamic gates. A finer
map remains deferred, on the same terms `guest_mprotect` already states.

### M13-tidy — two renames in code M13 is already editing

- `set_region_exec_attr` → `set_region_attr`. It takes an arbitrary attribute, and M13 hands it a
  *non-exec* one; the current name would become a lie at its newest call site.
- `subtract_reservations`' head/tail/split body (`lib.rs:1856`) — already correct and covered by
  `carveout.rs` — is extracted into a pure `subtract_range(&mut Vec<(u64,u64)>, addr, len)` used by
  both tables. `subtract_reservations` becomes a one-line caller and `carveout.rs` passes unchanged.
  Without this, `noaccess` grows a second, subtly-different copy of tested arithmetic.

Neither is speculative refactoring: both are in functions M13 edits.

## Determinism posture

**Standard, and structural rather than disciplined.** All three protection calls are already recorded
`Event::Syscall`s, and replay already re-dispatches all three into the same `Box_` methods. Record and
replay therefore stamp identical attributes from identical call sequences, through one implementation.
Symmetry rule 1 is satisfied by construction — there is no second code path to keep in step, and no
new mirror to write.

`flush_guest_tlb` is below the trace (symmetry rule 2): it is called from paths shared by record and
replay, fires identically on both sides, and never surfaces to the record/replay loop.

Nothing about a protection change enters the trace beyond the syscall event that was already there.
**`TRACE_MAGIC` is unchanged; no existing trace is invalidated.**

## Fail-loud boundaries

Each asserts at the point it would otherwise guess:

- `protect_none` over a page with no backing — M13-split's invariant.
- A protection change that would need to split a backing (a true partial straddle), mirroring
  `map_mmap_region`'s existing case-3 posture.
- A `prot` value other than `0` reaching `protect_none`.

## Scope

**In:** `ATTR_NONE`; `noaccess` on `Box_` and in `BoxState`; `protect_none`/`unprotect`; the four call
sites; `mach_vm_protect` routed into the box for the first time; the `set_region_attr` rename and the
`subtract_range` extraction; a Darwin-correct permission row in `signal_of_esr` if the spike calls for
one; the guest fixtures and gates below.

**Out, and named as such:** every protection bit other than no-access — read-only pages, write-without-
read, exec transitions via `mprotect` — deferred on the terms `guest_mprotect` already documents;
enforcing `PROT_NONE` on **reservations** (see below); `guest_munmap`'s wholesale-drop defect, which is
adjacent but independent and stays on M12's deferred list; threads; and everything else M12 carries
forward unchanged.

### The retained deviation: reservations stay demand-committable

`commit_reserved_page` (`lib.rs:1081`) continues to silently back any page inside a `mach_vm_map
cur_protection == 0` reservation — libmalloc's 24 GiB nano pointer range. On real macOS those pages
would fault too, so this is a deviation, and M13 keeps it deliberately:

- it is a known-working accommodation that four green dynamic headline gates rest on;
- the headline capability does not need it changed;
- removing it risks an M2-era wall-chain in a milestone that otherwise has none.

What M13 owes in exchange is a **number**: the measurement task counts `commit_reserved_page` hits
across `hello_dyn`, `hello_rust`, `jq`, and `jq_file`, and the Status section reports them. A
quantified deviation is honest; an unquantified one is the thing this project refuses.

## Exit criterion

`just gate` green with no existing assertion loosened, all six current headline gates still green and
un-ignored, and `protnone_rust_e2e` — a stock full-`std` Rust guest that `mprotect`s a page
`PROT_NONE` and then touches it — recording and replaying bit-for-bit, replayed twice.

**The exit code is necessary and nowhere near sufficient, and the gate must say so.** A guest that
dies of any uncaught fault exits the same way — that is exactly what `crashy_e2e` already asserts — so
a gate resting on the exit code would pass unchanged with M13's enforcement entirely absent, because
the *unprotected* store to a wild address would kill it just as dead. This is M12's "exit 139 is
necessary and nowhere near sufficient" lesson recurring one milestone later, so the gate asserts on
the trace:

- a fault whose ESR carries **DFSC `0x0f`** — a *permission* fault, which is the thing M13 creates and
  which no guest could produce before this milestone; a translation fault here would mean the page was
  unmapped rather than protected, and would pass a weaker gate;
- mapping to the **measured** signal (`SIGBUS`, Task 1's answer — not the table's prior assumption);
- `far` equal to the page the guest protected, learned from the recorded `mprotect` event rather than
  hardcoded;
- replay byte-identical, twice.

**Parked at the wall, per honest-gate discipline:** `stackoverflow_rust_e2e` is written and committed
`#[ignore]`d, naming M8 spec risk R3 — the guest believes it has an 8 MiB stack while 256 KiB is
backed, so libstd's guard sits 7.73 MiB below the real stack bottom and an overflow never reaches it.
The `#[ignore]` reason and the README Status section must both state that wall and both measured
non-fixes (8 MiB backing ≈ 1.7x on `hello_rust`; `getrlimit` ignored by libpthread). That is a NEW
parked gate for the capability the wall blocks — never a regression of the existing six.

## Testing

**Pure, no VM.** `ATTR_NONE`'s bit composition against the AP/UXN/PXN table; `subtract_range`'s four
cases (trim head, trim tail, interior split, full cover) — the arithmetic `carveout.rs` already pins
for `reservations`, now shared; `signal_of_esr`'s permission row against the measured answer.

**Box-level, freestanding asm guests** — libc out of the way, so they test the mechanism rather than
libstd's use of it:

- `protnone.s` — `mprotect` a known-backed page to `PROT_NONE`, store to it, fault. The guest's own
  handler reports the signal number and `si_addr` through distinct exit codes.
- `protrestore.s` — **the same guest, ordered protect-then-restore.** This is the gate that proves the
  TLBI actually invalidated: without a flush the stale RW entry lets the first store succeed, and a
  restore-only test would pass vacuously. The ordering *is* the test.
- `protnone_mach.s` — the same through `mach_vm_protect`, which is a separate arm and would otherwise
  be covered by nothing.
- A fail-loud negative: `PROT_NONE` over an uncommitted reservation page asserts, pinning M13-split's
  invariant.
- An `ipa_prot`-style accessor reading the live leaf descriptor (the shape `ipa_is_exec` already
  establishes), so a test can prove the stamp landed rather than inferring it from a fault.

**End-to-end.**

- `protnone_rust_e2e` — the headline, as specified in the exit criterion. Note that libstd installs
  `SIGSEGV`/`SIGBUS` handlers at startup (M11 measured flags `0x41`), so this guest also exercises
  M12's delivery: the fault is delivered to libstd's handler, which compares `si_addr` against its
  guard range, finds no match, resets to `SIG_DFL` and returns, and the re-executed store kills the
  guest — the `segvy` shape, now reached through a *permission* fault instead of a translation fault.
- `stackoverflow_rust_e2e` — **committed `#[ignore]`d**, naming the M8 R3 wall and both measured
  non-fixes. It is the gate that comes green when R3 is fixed.
- `crashy_e2e`, `segv_rust_e2e`, and the four dynamic gates unchanged — the regression that M13
  changed no existing fault's classification. `segv_rust_e2e` matters most here: it asserts
  `sig == 11`, and a wrong edit to `signal_of_esr` would flip it to `SIGBUS` and be caught.
- A reverse-debug seek to the delivery landmark, showing "rewind to the moment the protected page was
  touched" works — M12 made delivery seekable, so this tests the payoff rather than claiming it.

**Why the headline alone is insufficient**, stated so the mechanism gates are not treated as optional:
it never exercises the **restore** direction, because nothing in it unprotects; it reaches
`mprotect` but never `mach_vm_protect` or the `mmap` `prot == 0` path; and without the DFSC assertion
it would pass on a translation fault produced for an accidental reason.

## Risk register

| # | Risk | Mitigation |
|---|---|---|
| R1 | ~~`signal_of_esr`'s permission row is the Linux answer~~ | **RETIRED by measurement.** `spikes/protnone.c` measured `(SIGBUS, BUS_ADRALN)`, reproduced independently. Unmapped control still `SIGSEGV`, so `crashy_e2e` is safe. Task 3 fixes the row |
| R2 | ~~libstd's guard page lands outside the guest stack backing → `protect_none`'s assert fires~~ | **RETIRED by measurement.** Every PROT_NONE'd page is backed in both `hello_rust` and `hello_dyn` (libstd mmaps the page RW before mprotecting it), so the assert cannot fire on the dynamic gates |
| R8 | The guard sits 7.73 MiB below the real stack, so the stack-overflow headline cannot work (M8 R3) | **ACCEPTED, not mitigated.** Both fixes were already measured and rejected by M8 (8 MiB backing ≈ 1.7x; `getrlimit` ignored by libpthread). Headline swapped to `protnone_rust_e2e`; `stackoverflow_rust_e2e` parked `#[ignore]`d at this wall |
| R3 | A missing or ineffective TLBI leaves a stale permissive entry, so the guard silently fails to guard | `protrestore.s`, ordered protect-then-restore, which cannot pass vacuously |
| R4 | ~~dyld `mach_vm_protect`s something to `PROT_NONE`~~ | **RETIRED by measurement.** 47 calls in `hello_rust`, `prot` only `0x1`/`0x3`/`0x13`, never `0`. Task 9 is a dormant change |
| R5 | The headline passes on the printed message while the trace carries the wrong signal | Assert the signal number and `si_addr` in the trace, not the string |
| R6 | The headline guest's protected store is optimized away | `write_volatile` through the protected pointer; assert the guest actually faults rather than exiting 0 |
| R7 | Growing the stack-adjacent protected region slows the per-syscall memory diff (M8's measured lesson) | The guard is one page; watch the gate's wall-clock and say so if it moves |

## Components

| Crate | Change |
|---|---|
| `retrace-arch` | `signal_of_esr`'s permission row, if R1 calls for it, and its golden test |
| `retrace-box` (`lib.rs`) | `ATTR_NONE`; `noaccess` + accessor; `protect_none`/`unprotect`; hooks in `map_mmap_region`, `guest_mprotect`, `guest_munmap`; `set_region_attr` rename; `subtract_range` extraction; `ipa_prot` observability; `BoxState`/`checkpoint`/`from_checkpoint` |
| `retrace-core` | `mach_vm_protect` record + replay arms routed into `guest_mprotect` |
| `retrace-guest` | `protnone.s`, `protrestore.s`, `protnone_mach.s`, the fail-loud negative, `rs/overflow.rs` |
| `retrace` | `stackoverflow_rust_e2e`, the reverse-debug seek gate, the measurement harness |
| `spikes` | `protnone.c` — the Darwin signal/`si_code` probe |

## Open questions for implementation planning

1. Confirm against a live ESR that a `PROT_NONE` **load** and a `PROT_NONE` **store** both report
   DFSC `0x0f`. M13-attr argues they must, since the table indexes on DFSC and not on `WnR`, but that
   argument is read off the architecture rather than measured — and the whole permission row is
   unexercised (R1), so a wrong assumption here is invisible until a guest hits it.
2. Should `unprotect` restore `ATTR_DATA` unconditionally, or restore whatever the page held before it
   was protected? Only `ATTR_DATA` pages can be protected today (code pages are RO and no guest
   protects them), so the two are equivalent now — but the choice should be made deliberately, since
   the second is what a general prot map would eventually need.
3. `commit_reserved_page` currently returns `false` for an already-backed page, treating a re-fault as
   an unfixable bug that goes to the fatal path. A protected page inside a reservation would be backed,
   so it reaches that arm. Is the resulting diagnostic adequate, or does it need to name the protection?
4. Does the seeded swarm (`retrace-sim`) generate protection calls, and if not, should M13 teach it to?
5. Does any currently-green gate `mprotect` a range to `PROT_NONE` today and get away with it? (R4
   generalized beyond dyld.)
