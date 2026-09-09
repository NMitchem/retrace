# M31-checkpointparity — a guard on the path this class has bitten most

**Status:** design, 2026-09-08.
**Predecessors:** M24 (built the `load`↔`restore` parity guard and named this milestone as its
successor), M4 (built `from_checkpoint` and `checkpointed_seek`), and the milestones whose bugs
are the evidence: M7 t6, M8, M9 t3, M10, M11, M13, M14, M18, M23 t1.

## Why this milestone

`Box_` has three construction paths. `load`/`load_dynamic` run on the **record** side; `restore` and
`from_checkpoint` run on the **replay** side. Anything a load path establishes that a replay path
does not re-establish is a record/replay asymmetry whose signature is *a passing record followed by a
diverging replay*.

The determinism oracle cannot see this class. It compares replay against record's **trace**, never
against record's **box**, so when both replay paths are wrong the same way there is nothing to
disagree with. That is why the class keeps shipping: by the README's own count it has shipped seven
times (M9 t3, M10, M11, M14, M18, M21, M23), each fixed individually, none leaving behind anything
that would catch the next.

M24 closed half of it. `crates/retrace-box/tests/restoreparity.rs` diffs a `load` box against a
`restore` box built from that box's own snapshot — 15 of `Box_`'s 27 state fields plus two sysregs
and the 0x800 vector table — and states an obligation: a new field must be either covered there and
equal, or named in `normalise()` citing the mirrored replay mechanism by file and line.

**`from_checkpoint` has no such guard.** It is the path that restores far more state than `restore`
does, runs mid-run where nothing sits at a default, and has the longer bug history. M24's own closing
comment names this milestone and its real difficulty:

> That guard needs a mid-run fixture and a judgement about what *should* legitimately differ at a
> mid-run landmark; it is the successor milestone, not something this file quietly covers.

This milestone builds that guard and makes that judgement.

## Goals

1. A standing parity guard on the `from_checkpoint` path, with the same written obligation
   `restoreparity.rs` carries, so a new field has to get past a **test** rather than a reviewer.
2. Enough **reach** that the guard covers the fields this class has actually broken — which today
   no accessor can even observe.
3. A measured judgement about what legitimately differs at a mid-run landmark, recorded with its
   mechanism rather than asserted.
4. Fix every asymmetry the guard catches that a real guest can reach; park the rest honestly.

## Non-goals

- **Evolution after construction.** This guard compares two boxes at a landmark, not their behaviour
  afterwards. `crates/retrace/tests/checkpoint_seek.rs` already covers the behavioural axis
  (`checkpointed_seek_same_and_earlier_window_hits_match_cold`,
  `checkpointed_seek_matches_cold_across_a_neon_window`); this milestone does not rebuild it.
- **Closing M24's two structural blind spots.** They carry over unchanged and are restated under
  *Stated limits* rather than left to look covered.
- **A `BoxState` round-trip test.** Rejected on the merits — see below.

## The rejected approach, and why it is written down

The cheap version of this milestone is round-trip idempotence: `checkpoint()` →
`from_checkpoint()` → `checkpoint()` again, compare the two `BoxState`s. No new accessors, a pure
data comparison, and it would look like a parity guard.

**It is structurally blind to the exact class it would guard.** A field `BoxState` does not carry is
absent from *both* captures, so it compares equal and the test passes. Every historical instance of
this class was a field that was not carried — that is what the fix was, in each case: adding the
field. This approach would have caught **none** of them.

It is recorded here so that a later reader reaching for the cheaper instrument finds the reason it
was not taken, rather than the absence of one.

## Component 1 — the instrument

`crates/retrace-box/tests/checkpointparity.rs`, sibling to `restoreparity.rs` and deliberately the
same shape:

```
drive a guest forward to a mid-run landmark   (run() / forward_and_diff loop)
capture the live box's observable state
let state = b.checkpoint();
drop(b);                                       // HVF: one VM per process
let r = Box_::from_checkpoint(&state);
compare r against the captured state
```

The `run()`/`forward_and_diff` driving loop is the pattern `crates/retrace-box/tests/truncguard.rs`
already uses; `retrace-box` cannot depend on `retrace-core`, so the fixture drives the box directly
rather than through the recorder. `fdtable.rs` and `threads.rs` establish that fd and threaded
mid-run states are reachable in-crate.

The file carries its own `normalise()` and restates M24's obligation verbatim in shape: **covered and
equal, or named in `normalise` with the mirrored mechanism cited by file and line. There is no third
option that is safe.**

## Component 2 — reach, which is most of the honest work

`Box_::dbg_internal_state()` (`crates/retrace-box/src/lib.rs:5287`) exposes nine scalars:
`reservations`, `mmap_next`, `bootstrap_port`, `cache_installed`, `last_far`, `synthetic_tsc`,
`cache_refault_ipa`, `cache_refault_count`, `pac_enabled`. `restoreparity.rs` supplements it with
accessors for stack geometry, the two thread-pointer sysregs, `ctx_of(0)`, thread count,
fall-throughs, the vector table and the backing map.

**None of that reaches `fds`, `sigtable`, `thread_start_pc`, `wq_thread_pc`, `noaccess` or
`pthread_size`** — and those are where this class has actually lived. A guard built only from today's
accessors would compare the nine scalars, pass cleanly, and reach almost nothing that has ever
broken.

So this milestone adds `#[doc(hidden)]` test-only accessors for exactly those six, mirroring
`dbg_backings`'s existing treatment (guest-visible state only; host pointers deliberately not
exposed, because two boxes differ there by construction and comparing them is a test that can only
fail).

## Component 3 — the judgement about legitimate mid-run difference

`from_checkpoint` (`crates/retrace-box/src/lib.rs:5162`) leaves these at a hard default:

| field | disposition |
|---|---|
| `cache` | set `None`, then `if state.cache_installed { b.install_cache_pager(); }` on the last line — so it should compare **equal**. Coverage, not a normalise. |
| `window_cap` | M28 documented the reset as deliberate (test-only instrumentation). Cite that comment. |
| `canary_disturbances` | M30 documented the same. Cite that comment. |
| `bps_armed`, `wps_armed`, `watch_ranges`, `syscall_watch_hit` | **Unsettled. To be measured.** |

The debugger four are the load-bearing judgement. They are absent from `BoxState` entirely, so a
restored box forgets them. Either the debugger re-arms them on every seek — in which case the reset
is legitimate and goes into `normalise()` **with that mechanism cited by file and line** — or it does
not, and this is a sixth instance of the class that the guard has just found. The milestone settles
this by reading the seek path, not by assuming.

## Component 4 — the fixture

The guard's reach is bounded by what the captured state actually holds. A mid-run capture of a
simple guest leaves `fds`, `sigtable`, `threads` and `thread_start_pc` at their defaults on **both**
sides, and `Default == Default` passes for a reason unrelated to the assertion's name — the trap
`restoreparity.rs` explicitly calls out as growing a file's apparent authority without its reach.

So the fixture must hold, at capture time: an **open fd**, an **installed signal disposition**, a
**live spawned thread**, and `bsdthread_register` already seen. An existing guest is preferred; a new
one is added only if none covers it.

**Every one of these is asserted as a precondition** — the captured value must be non-default before
the comparison means anything. That assertion is what separates this guard from one that passes
because it cannot see.

## Testing

- **One parity test on the rich fixture**, with a non-default precondition asserted for each of
  the six newly-reachable fields. Tiering (a static and a dynamic variant, as `restoreparity.rs`
  has) is deliberately not done here: the thin tiers are the ones whose interesting fields sit at
  defaults, so they would add test names without adding reach.
- **A positive control.** Mutate `from_checkpoint` to reset one genuinely restored field (e.g.
  `sigtable`), confirm the guard goes **red**, revert. M28 and M30 both established that a guard
  nobody proved can fire is not yet an instrument; M28's own `let band = 0;` mutation passed a
  523-test gate unnoticed before it existed.
- Whatever asymmetries the guard catches get a named regression test each, per the fix policy above.

## Stated limits

These are carried forward from M24 unchanged, and are **not** closed here:

1. **Construction, not evolution.** The guard compares two boxes at a landmark. Divergence that
   develops afterwards is `checkpoint_seek.rs`'s axis, not this one.
2. **Two boxes wrong the same way are invisible** to any test that only diffs them against each
   other. A field both paths get wrong identically passes.
3. **Reach is bounded by the fixture.** A field the fixture leaves at its default is compared, and
   the comparison proves nothing. The preconditions make that visible rather than removing it.

## A documentation inconsistency to reconcile

Three places enumerate this class's `from_checkpoint` instances and **they do not agree**:

- `restoreparity.rs` — "bitten five times (M9 t3, M10, M11, M14, M18)"
- the `BoxState` field comments — number themselves 1..5 as M7 t6, M8, M10, M11, M23 t1
- `from_checkpoint`'s own `TPIDRRO_EL0` comment — "the fourth field here … (M9 t3, M10, M11)"

The README separately counts **seven** for the whole class across both paths (M9 t3, M10, M11, M14,
M18, M21, M23). Some of that is scope (whole class vs. this path) and some of it is drift. This
milestone reconciles the lists and states which count means what, rather than adding a fourth
enumeration. It does not assert in advance which is correct — that is a reading task, and the answer
belongs in the status log.

## Gate posture

`just gate`, chunked per CLAUDE.md — `retrace-box` run **whole-package** so its `Doc-tests` target is
not dropped, and `retrace --bins` present with its 11 unit tests. The expected count is derived
file-by-file from source **before** the run, not reconciled toward it afterwards. Baseline is M30's
**549 passed / 0 failed / 2 ignored over 118 binaries**.
