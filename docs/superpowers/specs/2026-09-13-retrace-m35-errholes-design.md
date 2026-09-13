# M35-errholes — a failing syscall writes after all, and a clamp that hid the proof

**Charter entry:** `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3, "M35 — `errholes`: the two holes M27 and M28 left". Written autonomously from the charter
under §5's authority, on the operator's 2026-09-13 instruction to run the queue unattended
through M38. §9's contract is met in §1 (wall), §4 (measurement), §8 (symmetry), §6 (positive
controls) and §7 (deliberately not done).

**The premise was measured before this spec was written, and it moved the milestone from
"measure and decide" to "fix".** The charter gave M35 medium certainty because "whether either
[fixture] *reaches* the `if !err` gate with a pointer argument worth banding is a measurement M35
still owes." §4 takes that measurement on the M28 fixture that already exists and finds the gate
is not a hypothetical hole: **`failsysctl` records cleanly and its replay diverges**, at the
`oldlen` cell, because the kernel writes `*oldlenp` on the `ENOMEM` path and the `if !err` gate
throws that write away. M28's "the kernel wrote nothing, before or after" was true of the one
buffer its test read and false of the call; the README has carried the true-of-one-buffer sentence
as if it were about the call since M28. So this milestone takes the charter's Branch A — hoist
the capture out of the skip — on evidence, and lands the other hole (`diff_memory`'s replay-side
`.min(avail)`) as the fail-loud it was always meant to be.

> **Ruling 1: M35 is a fix milestone, not a measurement milestone.** The charter's stated
> uncertainty ("whether either reaches the gate with a pointer argument worth banding") is
> resolved by §4's measurement in the direction that makes the fix mandatory: an existing fixture
> replays with a divergence exit (rc 3) on the pre-M35 tree. Under the charter's §1 argument
> (soundness before breadth) a known bit-for-bit replay failure on a repo-owned guest is the one
> thing the soundness phase exists to remove. Cost if wrong: none identified — the fix is three
> lines of control flow whose symmetry argument (§8) holds by construction.

## 1. The wall, located

Both holes are in `crates/retrace-box/src/lib.rs` at the M34 merge:

| hole | line | code today | what it costs |
|---|---|---|---|
| **H1** replay-side clamp | `:3930`, inside `diff_memory` (`:3924`) | `let n = r.bytes.len().min(avail);` | a recorded region longer than its replay backing is compared only up to the backing and the excess is **silently accepted** — the one place the terminal full-memory oracle can return `None` on a real mismatch |
| **H2** the error-path skip | `:3439`, inside `forward_and_diff` (`:3169`) | `if !err {` around the whole post-diff capture loop (`:3439–3559`) | a kernel write made by a **failing** syscall is never captured, so replay never applies it; the guard band inside the same block is never evaluated on that path either |

H1's comment history: flagged in M1's own branch review, deferred at M2 "where only the clamp
half was paid" (`docs/status-log.md:5116–5118`), carried by M27, M28, M30, M33 and M34 as
"still unpaid". H2's comment at `:3436–3437` states the assumption as a fact: "A failed syscall
(carry set) wrote nothing to the guest's buffers, so skip the post-diff write capture entirely."
M27 narrowed it (`ps`'s 83 `sysctl`s all `err=false`), M28 measured one case and found `buf`
unchanged, and nothing since has touched it.

The mirror of H1 on the *apply* side is already loud: `write_guest` (`:3639–3643`) asserts
`bytes.len() <= avail` with "overruns backing". `diff_memory` is the odd one out.

## 2. Scope

Two code changes in one function each, both in `retrace-box`; one existing test whose assertion
inverts by its own message; one new fixture; one new e2e; the docs. No `retrace-core` edit, no
`retrace-arch` edit, no trap arm, no `TRACE_MAGIC` bump (§8). The Apple sweep is re-run and any
binary that moves becomes an M36 row (charter §3, the M33 rule).

## 3. Design

### 3a. H1 — `diff_memory` refuses what it cannot compare

```rust
let (hp, avail) = match self.host_span(r.ipa) { … };
if r.bytes.len() > avail {
    return Some(format!(
        "recorded region at ipa {:#x} is {} bytes but its replay backing holds only {} \
         from that address — the recording and the replay disagree about the guest's memory \
         layout, which no byte compare can settle",
        r.ipa, r.bytes.len(), avail));
}
let cur = unsafe { std::slice::from_raw_parts(hp, r.bytes.len()) };
```

`n` disappears; the compare runs over the whole recorded region or not at all. The message names
the three numbers. This is the shape `write_guest` already has, moved from a panic (apply side,
where continuing is impossible) to a returned divergence (compare side, where the caller already
turns `Some` into the exit-3 path).

**Reachability, stated honestly.** On record, every captured region lies inside one backing
(`host_span` bounds every window by `avail`), and replay rebuilds the same backings from the same
snapshot and the same deterministic demand-paging, so on a *correct* replay `r.bytes.len() <= avail`
always and this branch never runs. It exists for the incorrect replay — a layout drift, a
checkpoint restored against a different backing set, a future edit to `page_in_cache` — and its
value is that such a drift now reports itself instead of passing. §6's Control 1 proves the
branch fires by handing `diff_memory` a region built to overrun; nothing in the corpus reaches it,
and the spec says so rather than implying otherwise.

### 3b. H2 — the capture runs on both paths

Delete the `if !err {` and its closing brace; the loop body is unchanged. Everything inside it is
already correct on the error path, and each reason is a measured fact of the tree, not a hope:

- **The pre-image and the canary fill are unconditional** (they happen before `host_svc`), so the
  post-image compare and the band check have exactly the same inputs on both paths.
- **The restore already runs on the error path** (M30, `:3572–3577`: "36 error-path restores in
  that same `jq` recording"), so unfilling is not new work; what is new is that the band is
  *checked* before it is restored, on that path too. The M30 comment's sentence "Nothing is CHECKED
  there for the same reason nothing is captured" is deleted with the reason.
- **Replay already applies `writes` on an `err = true` landmark.** `ReplaySession::advance`'s
  generic arm calls `apply_and_return(*ret, *err, writes)` (`crates/retrace-core/src/lib.rs:1844`
  and its siblings) with the recorded `err`, and `apply_and_return` (`retrace-box:3612–3629`)
  applies every region before setting `x0` and the carry. No replay edit is needed: the arm has
  handled a failing landmark with writes since M0 — it simply never received one.
- **The `Event::Syscall` shape does not change.** `writes: Vec<Region>` was always allowed to be
  non-empty beside `err: true`; a pre-M35 trace has empty `writes` there and replays exactly as it
  did (§8).
- **The band assert becomes live on the error path.** That is a strengthening, and it is the one
  place this milestone could turn a passing binary red: a failing syscall that writes past its
  window would now panic the recorder where before it was silently unrecorded *and* uncaptured.
  §4's xnu reading finds no such case among the failing-path writers it names (each writes inside
  the buffer the guest handed it, or the 8-byte `*oldlenp`); the sweep (§10) is the measurement.

The comment at `:3436–3437` is replaced by one that states what was measured (§4) and why the
capture is unconditional; the M30 restore comment's point 2 is rewritten to drop "wrote nothing".

### 3c. What consults `err` elsewhere, and stays

`:3599` (`allocates_fd`) and `:3605` (`close` retirement) gate **fd bookkeeping** on `!err` — a
failed `open` allocates no slot, a failed `close` retires none — which is the kernel's contract,
not an assumption about memory. Untouched.

`translate_fds` and `translate_mwl_regions` return `(e, true, Vec::new())` before the kernel is
reached; nothing was written, the empty `writes` is correct. Untouched.

## 4. Measurement — taken before any edit, and what it found

### 4a. The existing fixture replays with a divergence (the premise)

Binary: the main checkout's build at `e194b68` + the M34 docs commits (pre-M34 code; the gate is
identical at M34's merge), copied to the scratchpad and ad-hoc signed. Guest: the M28 fixture
`failsysctl` (`crates/retrace-guest/asm/failsysctl.s`: `sysctl(kern.ostype)` into a 2-byte
buffer, then `write(1, buf, 2)`, then `exit(0)`).

```
retrace record  <OUT_DIR>/failsysctl -o failsysctl.bin    → rc=0, stdout = 00 00
retrace replay  failsysctl.bin                            → rc=3
DIVERGENCE at landmark 4 pc=0x1000003ec: memory divergence at ipa 0x100004010: replay=0x02 recorded=0x00
```

`0x100004010` is `oldlen` (`mib: .space 16` at `0x100004000`, `oldlen: .space 8` at
`0x100004010`, `buf: .space 64` at `0x100004018`). The recording's final snapshot has
`*oldlenp = 0`; the replay still has the `2` the guest stored. **The failing `sysctl` wrote eight
bytes of guest memory and the `if !err` gate dropped them.** M28's `buf_changed=false` is still
true — `buf` is untouched — and its conclusion about the call was wrong, because its test
(`crates/retrace-box/tests/failwrite.rs:20–36`) reads `args[2]` (`buf`) and never `args[3]`
(`oldlenp`).

### 4b. Why, from xnu (`bsd/kern/kern_newsysctl.c`, apple-oss-distributions `main`)

- `sysctl_old_user` (`:1637`): `if (req->oldlen - req->oldidx < l) return ENOMEM;` **before** the
  `copyout` and before `req->oldidx += l`. So the data buffer is untouched and `oldidx` stays 0 —
  M28's datum, correct as far as it went.
- `userland_sysctl` (`:2175`): `if (error && error != ENOMEM) return error;` — `ENOMEM` falls
  through — then `*retval = req2.oldidx > req2.oldlen ? req2.oldlen : req2.oldidx`, i.e. `0`.
- `sysctl` (`:2014`, the syscall entry): the same `ENOMEM` pass-through, then unconditionally
  `suulong(uap->oldlenp, oldlen)` — **`*oldlenp` is written back on the `ENOMEM` path**, with the
  value the handler left in `oldidx`.

So every `ENOMEM` from `sysctl` writes `*oldlenp`. That is the `DerefU64(3)` pointer M29 put in
the table for exactly this syscall — the argument M29 refused to clamp because "the kernel also
writes it back" — and the one argument M28's measurement did not read.

### 4c. The data half, measured on the host (`kpa.c`, a plain process, no retrace)

Handlers that emit through repeated `SYSCTL_OUT` calls write what fits and *then* fail.
`kern.proc.*` (`bsd/kern/kern_sysctl.c`, `sysdoproc_callback` `:810–841` copies out each
`kinfo_proc` while `buflen >= sizeof_kproc`; `sysctl_prochandle` `:845–945` returns `ENOMEM`
when `needed > oldlen`, before `req->oldidx += req->oldlen`):

```
sysctl({CTL_KERN, KERN_PROC, KERN_PROC_ALL}, 3, buf, &len=648, NULL, 0)
sizeof(kinfo_proc)=648 ret=-1 errno=12(Cannot allocate memory) oldlen_after=0
bytes_changed_in_first_648=648 bytes_changed_past_648=0
```

A failing call that writes **648 bytes of data** into the guest's buffer, plus `*oldlenp` → 0,
and nothing past the buffer. This is the data-half positive control (§6, Control 3): the pre-M35
tree captures neither write; with the hoist both are captured and replay applies them.

A third family exists in source and is **not** measured here: `csops`' blob operations
(`kern_proc.c` `csops_copy_token` `:3511–3535`) copy out an 8-byte length header and return
`ERANGE` when `0 < usize < length`; a probe on an unsigned binary took the no-blob branch and
wrote nothing, so no control is built on it. M36's sweep rows are where it would show.

### 4d. What the corpus does on the error path today

Not censused by this spec. M30 measured **36 error-path restores on one `jq` run** — thirty-six
failing syscalls with at least one pointer argument in a mapped backing, every one of them with
its capture skipped. Which of those 36 the kernel wrote through is what the hoist will *record*
rather than what this spec predicts; the e2e gate (every record/replay pair in the tree) and the
sweep are the measurement, and §10 states what "unchanged" and "changed" each mean.

## 5. What must change

### 5a. `crates/retrace-box/src/lib.rs`

- `diff_memory` (`:3924–3940`): the §3a branch replaces the clamp.
- `forward_and_diff` (`:3436–3439` and the matching `}` at `:3559`): the `if !err` removed; new
  comment above the capture loop citing §4a/§4b (the `oldlenp` write-back on `ENOMEM`, the
  `kern.proc` partial copy) as the measurements that retired the assumption.
- The M30 restore comment (`:3572–3577`), point 2: "A failed syscall wrote nothing — which is
  exactly why the capture loop skips it" → the restore is on both paths because the fill is; the
  band is now also *checked* on both paths.
- `read_bytes_for_test`'s doc (`:3042–3044`): "which is exactly what the `if !err` measurement
  needs, since that path captures nothing" → since M35 that path captures; the seam stays because
  the test still wants a view independent of the capture.

### 5b. `crates/retrace-box/tests/failwrite.rs`

`a_failing_sysctl_is_measured_for_writes` becomes the record of what changed, by inverting the
assertion whose message already anticipated this milestone ("the `if !err` skip is no longer
skipping"):

- `assert!(err)` stays; `assert_eq!(before, after)` on `buf` stays (still true, §4b).
- `assert!(writes.is_empty())` → `writes` must contain exactly one region at `args[3]` of 8 bytes
  equal to `0u64.to_le_bytes()` (the `ENOMEM` write-back of `oldidx = 0`), and a second read
  through the seam, `read_bytes_for_test(args[3], 8)`, must show the same eight bytes — the seam
  and the capture agreeing is the point.
- The header comment is rewritten: M28 measured `buf`, M35 measured the call.

### 5c. `crates/retrace-guest/asm/failproc.s` (new) and `build.rs` / `lib.rs`

The data-half fixture: `sysctl({1, 14, 0}, 3, buf, &oldlen=648, NULL, 0)` with `buf: .space 648`
exactly, then `write(1, buf, 8)` — the first eight bytes of the first `kinfo_proc` (its
`p_starttime`; any bytes would do, the point is that they are *output* — the `bigread` shape,
where a lost write is visible in stdout and not only at the terminal compare) — then
`exit(0)`. Constant `FAILPROC`. Built exactly as `failsysctl` is (`build.rs:88–94`).

### 5d. `crates/retrace/tests/failsys_e2e.rs` (new)

Two tests, both record-and-replay through the CLI via `util::record` / `util::replay`:

- `a_failing_sysctl_replays_bit_for_bit` on `FAILSYSCTL`: record rc 0, replay rc 0, **and** the
  trace's `Event::Syscall { num: 202, err: true, writes, .. }` carries a region **covering**
  `oldlen`'s eight bytes with value 0 (a `Ptr` argument's window runs from the pointer to
  `PTR_WINDOW_CAP.min(avail)`, so the covering region may start at `mib` or at `oldlen` and be
  longer than 8) — assert on the write the milestone makes recordable, not on the exit code
  alone (CLAUDE.md: a weaker failure — a replay that never applies anything — also exits 0 on a
  guest whose divergence the compare cannot see).
- `a_failing_proc_list_replays_bit_for_bit` on `FAILPROC`: record rc 0; stdout is 8 bytes; the
  trace's landmark for `202` has `err: true`, a captured region covering the 648 bytes at `buf`
  (the `DerefU64(3)` window is exactly `*oldlenp` = 648) whose first eight bytes equal the
  guest's stdout, and one covering `oldlen` with value 0; replay rc 0. (The 648 bytes are nondeterministic host state — a
  `kinfo_proc` of whatever process the kernel iterates first — recorded and replayed as
  `task_info`'s audit token is: forwarded-and-recorded, never regenerated.)

### 5e. Docs

- **README**, the paragraph at `:684–695`: "Two holes stay open and unmeasured" → both closed at
  M35, with the §4a divergence line quoted and M28's datum re-described as true of `buf`; the gate
  paragraph (`:329–356`) moves to M35's figures with the reconciliation.
- **`docs/status-log.md`**: a new M35 section (append-only; M28's section stands with a forward
  pointer added *only* in the new section's text, never edited in place).
- This spec's §11 at close.

## 6. Positive controls

1. **H1 fires.** A unit test in `crates/retrace-box/tests/truncguard.rs` (beside the M29 pins):
   load any static guest, take `dest = STACK_TOP_IPA − 64` (64 bytes of backing, the M34
   `the_clamp_reaches_proc_info` precondition), read the 64 bytes actually there through
   `read_bytes_for_test(dest, 64)`, append 64 more zero bytes, and build
   `Region { ipa: dest, bytes }` (128 bytes, the first 64 of which match the guest exactly).
   Assert `diff_memory(&[region])` returns `Some(msg)` with `msg` containing "128 bytes" and
   "holds only 64". Mutation: restore the `.min(avail)` — the test goes red with `None`, because
   the clamp compares only the 64 bytes that match and never looks at the 64 that have no backing
   to compare against: precisely the silence the hole produced, reproduced on purpose.
2. **H2's `oldlenp` half.** `failwrite.rs` as rewritten in §5b, plus `failsys_e2e`'s first test.
   Mutation: reinstate `if !err {` — `failwrite.rs` goes red at "writes must contain the
   `oldlenp` write-back" and the e2e goes red with **replay rc 3 and the §4a divergence line** —
   the milestone's own premise reproduced by its own test.
3. **H2's data half.** `failsys_e2e`'s second test on `FAILPROC`. Mutation: the same reinstated
   gate — red with rc 3 at the terminal compare and, if the guest's eight output bytes are
   nonzero, a stdout mismatch between record and replay too.

Each control is run red then green in its task and the red output is pasted into the task report.

## 7. What this milestone deliberately does not do

- **No census of the corpus's failing syscalls.** M30's 36 error-path restores on `jq` say the
  path is busy; which calls wrote is what the hoist records. A census would be a second instrument
  for a question the fix answers directly.
- **No `csops` `ERANGE` control** (§4c): the probe did not reach the branch; building a guest to
  reach it needs a signed binary with a blob and a pid that survives §4b of M34, which is M36's
  measurement and M37's fix, not this one.
- **No widening of the band**, still (M27 → M34's owed lists). The band is now evaluated on both
  paths; how much of the backing it looks at is unchanged.
- **No change to what `diff_memory` compares on a correct replay.** H1 adds a branch that a
  correct replay never takes.
- **Nothing in `retrace-core`.** §3b shows the replay arm already handles the new landmarks.

## 8. Symmetry obligation

Rule 1 (record arm ↔ replay arm) is satisfied without an edit: the record side now emits
`Event::Syscall { err: true, writes: non-empty }` and the replay side's generic arm has always
applied `writes` before feeding `(ret, err)` — the identity holds because both sides use the same
`Region` and the same `write_guest`. Rule 2 is not engaged (no emulation). `TRACE_MAGIC` is not
bumped: the `Event` shape is unchanged and the meaning of a snapshot's bytes is unchanged. A
pre-M35 recording replays under M35 code exactly as it did under M34 (its failing landmarks carry
empty `writes`; the same divergence, if any, at the same place) — and an M35 recording is not
readable by M34 code only in the trivial sense that M34 has no code to reject it either; it would
apply the writes. This is the M2-taskinfo posture (forward-and-record), not a format change.

## 9. Rulings

- **Ruling 1** (above): fix, not measure.
- **Ruling 2: `diff_memory` returns a divergence rather than panicking.** `write_guest` panics
  because an apply that cannot complete leaves the guest half-written; a compare that cannot
  complete has a caller (`ReplaySession`'s three terminal arms) that already turns `Some` into the
  exit-3 path with the message printed. Same loudness, the existing channel. Cost if wrong: none —
  a returned `Some` is never swallowed by any of the seven callers.
- **Ruling 3: the band assert goes live on the error path with no exemption list.** The
  alternative — hoist the capture but keep the band check under `!err` — would carry M27's
  detector at half strength on the path M30 measured as busy, for no measured reason. If the sweep
  (§10) turns a binary red on an error-path band hit, that is the detector finding a write past a
  window M27–M34 never looked at; it becomes an M36 row, not an M35 exemption.

## 10. Gate

The full chunked gate in CLAUDE.md's shape, on the `m35-errholes` branch, exit codes captured
before pipes, logs sanitised, `--bins` never omitted, `#[test]` reconciled file-by-file against
M34's 572 / 0 / 2 over 124.

**Prediction:** `truncguard.rs` +1 (Control 1), `failsys_e2e.rs` +2 (one new binary), no other
count moves → **575 / 0 / 2 over 125**. Every existing record/replay e2e must still pass with the
capture live on the error path; a new divergence there is a measurement (the hoist recorded a
write the old tree dropped and the replay now applies it — that *fixes* divergences, it does not
create them), so the predicted direction of any change is red → green, never green → red, except
through the band assert (Ruling 3).

**Sweep:** `tools/apple-sweep.sh` re-run; M34's baseline is 46 / 8 with the M33 FAIL set and the
`dddiagnose` intermittent. Any binary that moves in either direction is entered in the status-log
with its reason and becomes an M36 row.

## 11. Outcome

Closed 2026-09-13 on branch `m35-errholes`, five code commits — `0beb06d` / `955d687` (Task 1
and its fix round), `a11e398` / `8a53c53` (Task 2 and its fix round), `12ac4e7` (Task 3) — and
one docs commit. Everything §5 said would land, landed; three statements in this spec's own text
were wrong on measurement and are corrected below rather than edited away. The full record is the
M35 section of `docs/status-log.md`; this section is the spec's own reconciliation against it.

### What landed against §5

- **§5a** — both changes, in `crates/retrace-box/src/lib.rs`. This spec's line numbers are at
  the M34 merge (`8854146`) and have moved; at `12ac4e7`: `diff_memory` is `:3933` and the §3a
  branch `if r.bytes.len() > avail` is `:3948` (was `:3924` / `:3930`); `forward_and_diff` is
  `:3167` (was `:3169`), the `if !err {` is a bare `{` at `:3447` (was `:3439`) under a new
  comment at `:3434–3445`, the loop body untouched and not re-indented; the M30 restore comment's
  rewritten opening clause is `:3569` (was `:3572–3577`); `read_bytes_for_test`'s doc is
  `:3042–3046` with the seam at `:3047` (was `:3042–3044`); the §3c fd gates are `:3608` and
  `:3614` (were `:3599` / `:3605`); `write_guest`'s "overruns backing" assert is `:3648–3652`
  (was `:3639–3643`). Two comment edits §5a did not list were found by the Task 2 review and
  made in `8a53c53`: `forward_and_diff`'s M0-era rustdoc ("On error (`err`) no writes are
  captured — a failed syscall wrote nothing"), which M10 had left glued onto the top of
  `translate_fds`'s doc block — a false contract on the wrong function, rendered by `cargo doc`
  — deleted whole (it was `:3092–3095` before that commit); and the M30 comment's "outside the
  `if !err`" clause, which named a gate that no longer existed. After `8a53c53`,
  `grep -nE 'wrote nothing|if !err' lib.rs` hits only the retraction inside the new comment and
  the two fd gates.
- **§5b** — `failwrite.rs` rewritten in place (1 `#[test]` → 1), asserting the call and not the
  buffer. **Correction to §5b's text:** "`writes` must contain exactly one region at `args[3]` of
  8 bytes" is not what the capture produces and never has been — regions are window-sized
  post-images, and on this fixture the landmark carries **two**: `mib`'s `Ptr` window at
  `0x100004000` (16,384 bytes, the whole `__DATA` page, so it also covers `oldlen`) and
  `oldlenp`'s at `0x100004010` (16,368). The plan relaxed the assertion to "some captured region
  covers `args[3]..+8` and carries the seam's bytes", which is what the test asserts. Its two
  measurement assertions (`buf` unchanged; `*oldlenp == 0` through the seam) **passed on the
  unmodified tree** — §4b confirmed in-process before the edit — and only the capture assertion
  was red.
- **§5c** — `crates/retrace-guest/asm/failproc.s` (60 lines), `build.rs:96–104`, `FAILPROC` at
  `src/lib.rs:140`, exactly as specified. **Plan defect, recorded by Task 2:** the plan's Step 5
  smoke script exits 126 on every line as written, because `mktemp -t` pre-creates the file
  `0600` and `cp` onto it keeps that mode; one `chmod +x "$BIN"` between the `cp` and the
  `codesign` fixes it (`tools/apple-sweep.sh` copies to a path `mktemp -d` did not pre-create,
  which is why its pattern works). The reviewer's independent re-run needed the same.
- **§5d** — `crates/retrace/tests/failsys_e2e.rs` (86 lines): `captured` `:23`, `the_sysctl`
  `:32`, `a_failing_sysctl_replays_bit_for_bit` `:46`, `a_failing_proc_list_replays_bit_for_bit`
  `:65`; both passed on the first run. **Correction to §5d's text:** "the `DerefU64(3)` window is
  exactly `*oldlenp` = 648" is **wrong**. `Box_::diff_window` (`:3084–3090`) computes
  `base = avail.min(window_cap)` and then, for a `Dest`, `base.max(clamp_count(avail, len))` — a
  table length only *widens* a window past the flat cap and never narrows it, and here `base` =
  16,360 already exceeds 648. The `failproc` landmark carries three regions, all ending at
  `0x100008000`: `0x100004000` / 16,384 (`args[0]`, `mib`, `Ptr`); `0x100004018` / 16,360
  (`args[2]`, `buf`, the `Dest(DerefU64(3))` window itself); `0x100004010` / 16,368 (`args[3]`,
  `oldlenp`, `Ptr`). The conclusion — the region covers the 648 bytes — holds; the supporting
  fact did not, the charter's class inside the spec that cites the charter's warning. (Task 3's
  report labelled the `0x100004018` region "a third `Ptr` window"; it is `args[2]`'s own — the
  review corrected it.)
- **§5e** — README (the two-holes paragraph replaced; the gate paragraph moved to M35's figures;
  the `dddiagnose` entry under Known limits carries the sweep finding below), the status-log
  section (append-only; M28's section untouched, superseded by pointer), and this §11.

### The controls (§6), run red then green

- **Control 1** (Task 1 Step 2, the `.min(avail)` clamp in place): RED at the `expect` —
  `diff_memory` returned `None`, "a 128-byte recorded region over a 64-byte backing must be
  reported as a divergence, not compared up to the backing and passed — that silence is the M1
  hole this test closes". Green after §3a: `truncguard` 22 passed, `checkpoint` 1 passed.
- **Control 2, unit half** (Task 2 Step 2, gate in place): `failwrite.rs` RED at the capture
  assertion **only** — "forward_and_diff captured no write covering *oldlenp on a failing
  syscall: the `if !err` skip is dropping a real kernel write (writes captured: 0)", `left: None`,
  `right: Some([0, 0, 0, 0, 0, 0, 0, 0])`. Green after the hoist; the whole `retrace-box` chunk
  beside it `binaries=37 passed=271 failed=0 ignored=0`.
- **Controls 2 (e2e half) and 3** (Task 3 Step 5, gate reinstated by hand, then reverted with
  `git checkout`): `a_failing_sysctl_replays_bit_for_bit` RED at "the landmark must carry the
  kernel's write-back of *oldlenp; without it replay keeps the guest's 2 and diverges at ipa
  0x100004010 — the pre-M35 measurement. writes: 0"; `a_failing_proc_list_replays_bit_for_bit`
  RED at "the landmark must carry the 648-byte record the kernel copied out on its way to
  ENOMEM"; `failwrite` RED as above; and a hand-run replay of a `failsysctl` recording made under
  the mutation: `DIVERGENCE at landmark 4 pc=0x1000003ec: memory divergence at ipa 0x100004010:
  replay=0x02 recorded=0x00`, exit 3 — §4a's line, byte for byte. Green after the revert
  (2 passed; 1 passed).

### What the hoist recorded (§4d, measured by the fix rather than censused)

`hello_dyn` and `jq --version` through a signed copy of the Task 2 CLI, the band live on the
error path for the first time: all four exits 0, `[M30 CANARY]` 0 and 0 (Ruling 3's assert did
not fire), reproduced independently by the reviewer. On those recordings **0 of 34** (`jq`) and
**0 of 32** (`hello_dyn`) `err = true` landmarks carry writes — per-recording counts, not
properties of the binaries (the reviewer's recordings from another cwd: 0 of 30 and 0 of 27); the
zero is the reproducible conclusion, and it was proven a measured zero by `failsysctl` through
the same CLI: replay rc **0** (§4a measured 3), one `err = true` landmark with the two regions
above.

### Sweep (§10)

`TALLY pass=45 fail=9 skip=0` on the `12ac4e7` binary: M33's eight plus `/usr/bin/dddiagnose`
("replay diverged"), exactly M34's run 1. **No binary moved in either direction** — §10's
"unchanged" case; no M36 row from the hoist, and the band assert reddened nothing. The
`dddiagnose` row was probed twice with traces kept, and the result corrects M34's account: with
recorder pids **outside** the collision range, **10 of 10 FAIL on both the pre-M34 and the M35
binary**, every run `rc=4 rp=3`. The recorder's stderr shows two lines and only the second is a
wall: first M23's *serviced* refusal `refusing mach_msg2 message-queue send (… dest <a port name
that varies per run> …): the box hosts no message-queue receivers` (`Route::RefuseMqSend`,
`crates/retrace-core/src/machmsg.rs:104–107`, `lib.rs:508–525` — `MACH_SEND_INVALID_DEST`
returned, an `err: false` landmark appended, the guest continues), then `RECORD ERROR:
unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the
send+rcv RPC shape` (`Route::Unsupported`, `machmsg.rs:108–109`, `lib.rs:557–559`) — a
*receive*-shaped message-queue call, `MACH64_SEND_MQ_CALL | MACH64_RCV_MSG` with no
`MACH64_SEND_MSG`, which `machmsg.rs:100–103` leaves fail-loud as "never been observed"; this
probe is its first sighting. A **refused recording** the sweep labels "replay diverged", the
replay correctly running out of events at the next syscall. With pids **inside** the range (after
the gate advanced the counter), 2 PASS (`rc=139 rp=139`, an identical crash the sweep counts
without a note) and 3 FAIL: the same serviced refusal, survived, then a `brk` at
`pc=0x18035f084` — M23's "other four `brk`" class (`machmsg.rs:97–99`), a second post-refusal
path. Ruling (controller, ledgered): not an M35 regression, **not class E2** — record and replay
agree within every run; the guest's path varies with its inputs — but **class B,
known-unmodelled** (two walls: the RCV-only message-queue shape, and the post-refusal `brk` M23
parked) with an environment-driven appearance; the serviced refusal is not a wall. The numbers
file first read the `refusing` line as the wall, "unmodelled since M2-mach" — wrong, caught by
the Task 4 review against `machmsg.rs` and the kept traces, corrected in all three documents
before merge. M34's "10/10 PASS" measured that morning's guest state, not a
property of the binary; its "a pid in range does not by itself produce the divergence" stands,
but the inverse is not established either — the pid hypothesis is neither confirmed nor refuted,
and M36 records the pid beside each row. Two sweep-harness defects go to M36 as **E1** rows: a
record exit of 4 reported as "replay diverged", and an identical crash on both sides counted PASS.

### Gate (§10)

**575 passed / 0 failed / 2 ignored across 125 test binaries** — the prediction, matched exactly.
Chunks `ws` 152/26, `box` 271/37 (M34: 270), `e2e1`–`e2e4` 46 + 30 + 64 + 1 over 20 + 20 + 20 + 1
(61 targets, so a fourth group of one; `failsys_e2e` in `e2e1`, both `ok`), `bins` 11/1, clippy
clean; every exit code 0, captured before any pipe; zero `SKIPPED` lines; 12:39:28 → 12:58:12
EDT. Reconciled file-by-file against M34's 572 / 0 / 2 over 124: `truncguard.rs` 21 → 22,
`failsys_e2e.rs` new at 2, `failwrite.rs` 1 → 1, `lib.rs` 13 → 13, `build.rs` 0 → 0,
`retrace-guest/src/lib.rs` 9 → 9; binaries 124 → 125, `--bins` 11 → 11; the tree holds 575
attributes = 573 + 2 ignored, the run reports 575 = 573 + the 2 census tests that run twice; bare
`grep -r '#\[test\]' crates | wc -l` 573 → 576. The two ignored are the M21 and M19 walls, as at
M34; nothing parked, nothing un-parked.

### Rulings the run added (all in `progress.md`)

- Task 1's and Task 2's fix-round re-reviews were each replaced by a mechanical check by the
  controller: the Task 1 fix was a pure relocation (removed lines = added lines as multisets,
  34/34, one file), and the Task 2 fix a 9-line comment diff checked line by line against the
  review's three items. Cost if wrong: none identified; the final whole-branch review saw both.
- The `dddiagnose` ruling above (class B, not E2; two E1 rows to M36).

### Rulings 1–3, as they held

Ruling 1 held: the fix was mandatory and three lines of control flow. Ruling 2 held: the Task 1
review checked all seven `diff_memory` callers and none treats `Some` as anything but divergence.
Ruling 3 held with nothing to exempt: the band assert, live on the error path across the whole
gate, the two real-guest recordings and the 54-binary sweep, fired zero times.
