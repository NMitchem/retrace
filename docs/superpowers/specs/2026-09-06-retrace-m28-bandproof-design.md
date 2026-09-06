# M28-bandproof — design

**Goal:** Make the M27 guard band **trustworthy**. It currently asserts something nothing proves it
can detect, rests on a premise nobody has written down, and is silently disabled on one whole class
of syscall. M28 pays those three debts, in that order, and does **not** touch the band's coverage.

**Predecessor:** M27 (`fb79298`), which landed the band, measured its blast radius across the whole
gate (zero firings), freed `/bin/ps` via `retrace_arch::dest_buffer`, and — the finding that makes
this milestone exist — measured its own detector to have a **false negative on its headline case**.

## The problem M27 left

Three distinct gaps, each named by M27's own final review and deliberately deferred there rather
than guessed at.

### 1. Nothing proves the band can fire

`truncguard.rs` unit-tests `Box_::overran_window`, which is `!pre.is_empty() && pre != post` — the
one part that cannot be wrong. Everything that *can* be wrong is untested: the `hp.add(win)` offset,
the `GUARD_BAND.min(avail - win)` sizing, and whether the `assert!` is reached at all.
`bigread_e2e` and `memdiff`'s M26 guard both drive the plumbing, but both are **negative controls** —
they prove it does not false-fire.

**The M27 band has never been observed to fire on any real syscall.** M26's "fired exactly once, on
the real culprit" was the *tail-of-window prototype*, a different detector. The one real overrun the
repo owns (`/bin/ps`, 139,880 bytes) was measured **not** to trip it. So `let band = 0;` — or an
off-by-one in the offset — passes the entire 523-test gate identically to the code that shipped.
By the repo's own rule, the band's green is currently indistinguishable from a band that does
nothing.

### 2. The "proof" claim rests on an unstated premise

The soundness argument is airtight for *"a kernel write happened in this range"*: the guest vCPU is
halted across `host_svc` and `clippy.toml` bans recorder threads, so nothing else could have touched
those bytes. It does **not** reach the claim M27 originally made — that the write ran past *this
argument's* window and is captured in no `Event`.

`forward_and_diff` snapshots a window for **every** argument that looks like a mapped pointer,
including non-pointer arguments whose numeric value collides with a mapped IPA (the code's own
`dyld's pread count 0x4000 collides with the trampoline IPA` case). If a syscall's second output
pointer lies inside `[ipa+win, ipa+win+GUARD_BAND)`, the kernel's legitimate and **fully captured**
write to it changes the band and panics a correct recording, with a message that misdirects the
reader to `dest_buffer`.

M27 softened the prose. M28 makes the strong claim **true**, which is also the prerequisite for ever
sampling wider.

### 3. The band is disabled on the `if !err` path

The band comparison lives inside `if !err { … }`, so a syscall that returns an error is neither
diffed nor band-checked. The README already names that gate as an open hole and names the exact
suspect — `sysctl` with an undersized buffer returns `ENOMEM` **and copies out what fits** — but
nothing has measured it. The detector is off on precisely the path the repo already suspects.

## Components

### 1. A positive control, via a test seam on the window cap

`PTR_WINDOW_CAP` has exactly one production use site (`Box_::diff_window`'s
`let base = avail.min(PTR_WINDOW_CAP);`). It becomes the **default** of a `Box_` field, with a
test-only setter:

```rust
/// Test seam (M28). Production never calls this: `Box_::load*` sets the field to
/// `PTR_WINDOW_CAP` and nothing else writes it. It exists so the guard band can be given a real
/// kernel overrun to detect — see `truncguard.rs`. Shrinking the cap does NOT weaken any
/// `dest_buffer` widening, which is a `max` over this base.
pub fn set_window_cap_for_test(&mut self, cap: usize);
```

The test loads the **existing** `fileio` guest (no new guest needed), shrinks the cap well below
what `fstat` writes, and drives to its `fstat` (189). 64 is the worked example throughout this
document; the plan pins the actual value from a measurement of the real write length (see R4)
rather than from this number:

- `fstat` writes a `struct stat` of ~144 bytes — comfortably past a 64-byte window.
- `fstat` is deliberately **absent** from `dest_buffer` (its length is not in a register), so the
  widening does not rescue it. This is load-bearing: the same test written against `read` would be
  vacuous, because `dest_buffer` widens `read`'s window to the full count regardless of the cap.

So the window is genuinely computed, the kernel genuinely overruns it, and the `assert!` is reached
through the real code path. Asserted with `#[should_panic(expected = …)]`.

**The bar this must clear:** `let band = 0;` must FAIL this test. That mutation passes the whole
current gate, and it is the specific defect the control exists to exclude. The plan must verify the
mutation fails, not merely that the test passes.

### 2. Shrink the band to what no other window covers

After computing `band`, shrink it so that the band range `[ipa+len, ipa+len+band)` contains no byte
lying inside any **other** entry's window range `[ipa_j, ipa_j+len_j)`. Concretely: for every other
entry whose span intersects the band, clamp `band` to end where that intersection begins, and take
the minimum across all entries. If nothing is left, no comparison happens.

Note the test is **span intersection, not start position**: a window that begins *before* `ipa+len`
but extends into the band overlaps it just as much as one that begins inside it, and a rule phrased
on start position alone would miss exactly that case.

**Shrink rather than skip.** Several arguments of one call routinely land in the same backing —
`/bin/ps`'s `sysctl` had three, two of them adjacent on the stack — so skipping the band on *any*
overlap would quietly disable the detector in exactly the dense cases where truncation is most
likely. Shrinking preserves every byte of detection that is unambiguous and discards only the bytes
whose attribution would be ambiguous.

With that in place the assert message and the README drop M27's softened wording and state the
strong claim, because it is now true by construction.

### 3. Provoke the `if !err` case rather than wait for it

A new freestanding asm guest calls `sysctl` with a deliberately undersized `oldp` buffer, so the
kernel returns `ENOMEM` and copies out what fits. Deterministic and repo-owned, the way `bigread` is
for M26 — and repo-owned for the same reason: it must not depend on a machine's process count or on
Homebrew being installed.

The measurement: does the kernel write into the guest buffer on that failing call?

- **If it writes** — the `if !err` gate is dropping real kernel writes, and the fix is expected to
  be small: capture writes on the error path too. Replay already applies recorded writes irrespective
  of `err`, and `err` is itself recorded, so this is expected to need no replay-side change. It lands
  with a repo-owned e2e gate asserting the guest records and replays.
- **If it does not write** — that is a legitimate result and gets written as one: the hole narrows to
  "measured on `sysctl`/`ENOMEM`, not cleared in general", and the band's disablement on that path is
  documented rather than quietly carried.

**Do not guess which.** Measure first; the fix, if any, is decided by what the measurement says.

## Rejected alternatives

- **A `cfg(test)` window cap instead of a field + setter.** No public test-only surface, but
  `#[cfg(test)]` applies to *every* lib unit test, silently shrinking the cap for unrelated tests,
  and integration tests in `tests/` cannot see it at all — so the positive control could not live in
  `truncguard.rs`, where a reader looks for it. Rejected: spooky action at a distance, in the file
  whose whole job is being trustworthy.
- **A synthetic unit test of an extracted band helper.** Cheap and pure, but it proves only the
  offset and sizing; the assert's *reachability* through `forward_and_diff` with the real window
  stays unproven, which is the actual gap. It would leave the headline finding half paid.
- **Skipping the band on any overlap** (instead of shrinking). Simpler, but see Component 2 —
  it disables the detector precisely where multiple arguments crowd one backing.
- **Strengthening the band's coverage in this milestone.** See Out of scope.

## Symmetry

Everything here is **record-side**. `forward_and_diff` never runs on replay, and an `assert!`
produces no trace record, so **no dispatch mirror is owed** under symmetry rule 1 and **`TRACE_MAGIC`
does not move** — M28 adds no `Event` variant or field.

Component 3 is the one to watch. If the measurement leads to capturing writes on the error path, that
changes *which writes a recording contains* but not the trace's *shape*. That is still not a format
break, but it is the one change here that alters trace contents, so it must be gated end-to-end
rather than unit-tested alone. If a format change ever seems necessary, that is a spec deviation —
stop.

## Testing

- **Positive control:** `truncguard.rs`'s `#[should_panic]` test above, plus the `let band = 0;`
  mutation verified to fail it.
- **Negative controls stay green:** `bigread_e2e` and `memdiff`'s M26 guard must not fire. A firing
  there means the intersection-shrink is wrong, not that a bug was found.
- **Suppression counting (R1):** the intersection-shrink must report how often it suppresses band
  bytes across the full gate. A number, not an assumption.
- **`if !err`:** the new guest records and replays; whether it also gates a *fix* depends on the
  measurement.
- **Gate:** full chunked run, reconciled file-by-file against M27's close (523 / 0 / 2 over 115).

## Risks

- **R1 — the intersection-shrink suppresses more than expected**, quietly weakening the detector the
  milestone exists to strengthen. Mitigated by counting suppressions across the gate and reporting
  the number rather than assuming rarity. If it is common, that is itself a finding about how often
  arguments crowd a backing.
- **R2 — the test seam is public API that exists only for tests.** Accepted deliberately and
  documented at the definition. The alternative leaks a shrunken cap into unrelated unit tests.
- **R3 — component 3 finds nothing.** A legitimate outcome, written as one. It narrows the `if !err`
  hole to a measured case rather than closing it, and the milestone must not overstate that.
- **R4 — `fstat`'s `struct stat` size is assumed, not measured.** The positive control depends on it
  exceeding 64 bytes. The plan measures the actual returned write length rather than trusting ~144,
  and picks the test cap from that measurement.

## Explicitly out of scope, and why

- **Strengthening the band's coverage** (wider or strided sampling). M27 measured the contiguous
  64-byte band to miss a real 139,880-byte overrun, so the temptation is real. It stays deferred:
  a wider sample is only *sound* once "past the window and not covered by another window of this
  call" exists in code, which is Component 2. Building the sampler in the same milestone that first
  defines its precondition would repeat exactly the unmeasured-design error M27 exists to refuse.
- **`diff_memory`'s `.min(avail)`** — replay-side, unpaid since M1, and mixing it into a record-side
  milestone would make both harder to review. Unchanged from M27's reasoning.
- **The owed `DerefU64` clamp** for `sysctl`'s in-out `*oldlenp`. Still owed, still unmeasured,
  still regressing nothing.
- **The remainder of the audit table** (`getdirentries64`, `recvfrom`, `getfsstat64`, `proc_info`,
  `getattrlist`/`fgetattrlist`, `csops`). Finding those is the band's job; that is the argument for
  fixing the band first rather than hand-auditing them now.
