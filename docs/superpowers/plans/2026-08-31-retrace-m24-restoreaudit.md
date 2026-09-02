# M24-restoreaudit Implementation Plan

Spec: `docs/superpowers/specs/2026-08-31-retrace-m24-restoreaudit-design.md`

**t1 has already landed** as `81d32ef` (the four asymmetries + `tests/restoreparity.rs`), ahead of
this plan — recorded in the spec under "Why this spec is retroactive" rather than back-dated here.
This plan covers t2–t4.

## Global Constraints

- **The gate is chunked.** `just gate` exceeds the tool ceiling. Chunk it, `--no-fail-fast`,
  `--test-threads=1`, and capture cargo's exit code **before any pipe**. This shell is **zsh**:
  the idiom is `$pipestatus[1]`, not bash's `$PIPESTATUS[0]`, which expands to empty here and
  silently reports nothing. Do not omit the `--bins` chunk.
- **Reconcile the total file-by-file**, not by trusting a sum. Baseline: M21's close, 504/0/2 over
  111 binaries. t1 adds 5 tests and 1 binary.
- **Every new test must be demonstrated able to fail** before it counts. Revert the fix, confirm
  red, restore. A parity assertion that cannot fail is worse than no assertion, because it reads as
  coverage.
- **Symmetry rule 1 applies to anything touching dispatch.** t3 does; t2 does not.
- No new `#[ignore]` and no un-`#[ignore]`. M24 buys a guarantee, not a capability.

### Task 2: Deepen the guard where it is cheap, and only where it is honest

The spec's "Coverage" section names 14 uncompared fields. Three are worth pinning now; the rest are
not, and the distinction is the task.

**Do:**
- Compare `backings` — count, and the `(ipa, len)` set. Load builds them from the Mach-O, restore
  from `mem`; they are expected equal and nothing checks it.
- Compare `next_l3`. Both paths derive it from `backings`, by *different code* (`restore` and
  `from_checkpoint` each compute it independently). Two derivations of one value is exactly the
  shape that drifts.
- Deepen the `threads` comparison from `ctx_of(0).regs.pc` to the full thread-0 `ThreadCtx` plus the
  thread count.

**Do not**, and say so in the test file rather than silently omitting: the ten
default-on-both-sides fields (`noaccess`, `bps_armed`, `wps_armed`, `watch_ranges`,
`syscall_watch_hit`, `tlbi_stub_ready`, `fds`, `sigtable`, `thread_start_pc`, `wq_thread_pc`,
`pthread_size`). Asserting `Default == Default` at landmark 0 is a test that passes for a reason
unrelated to its name, and it would grow the guard's apparent authority without growing its actual
reach. Add them **if and when** a load path starts setting one before the first landmark — the
obligation text already requires exactly that.

**Acceptance:** each of the three new comparisons mutation-verified (perturb the restored value,
confirm the named assertion fires). The "do not" list written into `restoreparity.rs` as a comment
with this reasoning, so the next reader does not mistake the omission for an oversight.

### Task 3: Close F4 properly — bump `TRACE_MAGIC`

M23 t1 changed snapshot *content* (vector padding `UDF #0` → `hvc #1`) without bumping
`TRACE_MAGIC`, so a pre-M23 recording still opens and restores its old padding. t1's L1 assert
turned that from a silent wrong replay into a loud refusal, which is strictly better and still the
wrong layer: a format break belongs at `open_checked`, not in an assert deep inside `restore`.

**Do:** bump `TRACE_MAGIC` (`RT\x00\x08` → `RT\x00\x09`). Verify a stale trace is rejected at open
with the format-mismatch path, not by L1's assert.

**Keep L1 anyway.** It is not made redundant: the magic guards the *file*, L1 guards the *box
construction*, and they fail at different layers for different callers. A future change to
`build_vector_table()` that does not touch the format is caught only by L1.

**Acceptance:** every e2e gate re-records and passes (they record fresh, so the bump is transparent
to them); a deliberately stale trace is refused at open. Note in the status-log that F4 is closed
and by which mechanism.

### Task 4: The gate and the two documents

1. Full chunked gate + clippy, exit codes captured per the constraint above.
2. Reconcile file-by-file against 504/0/2 over 111. Expected after t1–t3: **509/0/2 over 112**
   (t1 +5 in `retrace-box`; t2 added five assertions inside the shared parity helper and **no new
   `#[test]` functions** — an earlier draft of this line said +3 and was wrong; t3 adds none). Any
   deviation gets chased, not accepted.
3. **README** — edit in place. "Known limits": remove F5, remove F4, and rewrite the
   restore-asymmetry entry to say what is now true: the `load`↔`restore` path is guarded by a
   standing test, the `from_checkpoint` path is **not**. Do not write that the class is closed.
4. **`docs/status-log.md`** — append a new M24 section. Append only; M23's section stays as written,
   including its F4/F5 entries, with the forward pointer this section provides.
5. Record the successor milestone (`from_checkpoint` parity) where the next session will find it.

**Acceptance:** gate green, ignored count still 2, both documents updated, neither restating the
other.

## Self-Review

- **Is t2 padding?** It adds three comparisons and explicitly declines ten. If the declined ten were
  added instead, the guard would look twice as thorough and be no more capable of catching anything.
  The task is as much about the refusal as the additions.
- **Does t3 risk invalidating existing traces?** Yes, deliberately — that is what a format break is
  for, and every e2e gate records fresh. The only casualty is a hand-kept trace file from before
  today, which is the thing F4 exists to reject.
- **Is the biggest finding being deferred?** Yes, and named: `from_checkpoint` parity. It is out of
  scope because it needs a mid-run fixture and a judgement about what *should* differ at a mid-run
  landmark — that is a milestone, not a task, and pretending otherwise inside M24 would produce a
  shallow version of the guard that has bitten this codebase five times.
