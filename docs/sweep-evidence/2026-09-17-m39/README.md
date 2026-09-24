# Sweep evidence — M39 Task 7, run 2026-09-21

The directory is named for the milestone's date (`2026-09-17`, the spec and plan's), as M36–M38's
are; **the run itself was taken 2026-09-21**, which is what `sweep.log`'s header line says
(`commit=123cb97 date=2026-09-21 20:21:19 EDT`). The prose below states the run date.

One measurement: the full 54-entry corpus swept once on the close's binary and diffed row by row
against M38's run, per spec §6 ("PASS count ≥ 44, every moved row explained by name, no row moved
by anything this spec does not name").

**Result: `TALLY pass=44 fail=10 skip=0` — identical to M38's, and no row changed its label, its
result, its `rc` or its `rp`.** Nine rows differ in a field that is not a label, and each of the two
reasons is measured below; neither is behavioural and neither is M39's.

## Method

`tools/apple-sweep.sh`, run detached from the worktree root, wrapped in a three-line script that
prints the wrapper's `pidstart` and the binary's hash/commit/date first (the M38 shape). The
close's `target/aarch64-apple-darwin/debug/retrace` was copied to the session scratchpad and
ad-hoc signed there with `retrace.entitlements` — sha256 after signing
`7c514a7f607e9699811351ea8d10f0af942141f9c05fa9bb1b078ea962d2636f` — so that the background gate's
concurrent `cargo test` could not swap the binary under the sweep.
`RETRACE_SWEEP_KEEP=…/sweep` kept every non-clean row's `rec.err`/`rp.err`/`rp.out`/`bin`; the
`.bin` traces were removed unread and none is committed (the M37/M38 rule — `yes.bin` alone is
348 MB). Log: `sweep.log`.

| `pidstart` | recpid range | regime (M36's window `[0x4000,0x18000)`) | `TALLY` | `SWEEP_EXIT` |
|---|---|---|---|---|
| 29119 | 29138–32207 (`0x71d2`–`0x7dcf`) | inside `[0x4000, 0x10000)`, the trampoline page — M37's regime **I**; one regime (§4b retired since M37, so the pid selects nothing) | `pass=44 fail=10 skip=0` | 0 |

Commit swept: `123cb97` (the head after Tasks 1–4; Tasks 5 and 7a change nothing under `crates/`,
which `git diff --stat 123cb97 HEAD -- crates` confirms empty, so the swept binary is the close's).

## The ten non-clean rows — unchanged from M38

Same ten binaries, same labels, same `rc`/`rp`, same wall syscall numbers:

| row | label | `rc`/`rp` | wall |
|---|---|---|---|
| `/bin/csh` | `FAIL` (record error) | 4 / 3 | `mach_msg2` msgh_id 3403 (`mach_ports_register` ← `fork`), class C |
| `/bin/tcsh` | `FAIL` (record error) | 4 / 3 | 3403, class C |
| `/bin/ed` | `FAIL` (recorder panicked) | 101 / n/a | `openat_nocancel` (464) has no `arg_kinds` row |
| `/bin/ls` | `FAIL` (recorder panicked) | 101 / n/a | `getattrlistbulk` (461), no row |
| `/usr/bin/automationmodetool` | `FAIL` (recorder panicked) | 101 / n/a | `kevent_qos` (374), no row |
| `/usr/bin/desdp` | `FAIL` (recorder panicked) | 101 / n/a | 464, no row |
| `/usr/bin/dyld_info` | `FAIL` (recorder panicked) | 101 / n/a | 464, no row |
| `/usr/bin/flex` | `FAIL` (recorder panicked) | 101 / n/a | 464, no row |
| `/usr/bin/dddiagnose` | `FAIL` (recorder panicked) | 101 / n/a | `statfs64` (345), no row |
| `/usr/bin/yes` | `FAIL` (timed out after 30s recording) | 137 / n/a | never terminates; the watchdog, by design |

No `identical fault` row, no `replay diverged` row, and **no class-E row** — no recorder finished
cleanly and had its replay then disagree, which is the spec's halt condition. Every FAIL is a
`RECORD ERROR`, a recorder panic, or the `yes` watchdog, exactly as at M38.

## Row-by-row diff against M38

Comparison keys on the binary's path and compares `result`, `rc`, `rp`, `landmark`, `rec_reason`
and `rp_line` against `docs/sweep-evidence/2026-09-16-m38/sweep.log`'s 54 `ROW` lines (the table in
that directory's README tabulates them). **45 rows are byte-identical; 9 differ, none in its label.**

| row | field that moved | M38 | M39 | moved by |
|---|---|---|---|---|
| `/bin/ed` | `rec_reason` only | `…lib.rs:944:38: M33: syscall 464 …` | `…lib.rs:946:38: M33: syscall 464 …` | **M38's own close**, not M39 — see below |
| `/bin/ls` | `rec_reason` only | `…lib.rs:944:38: … syscall 461 …` | `…lib.rs:946:38: … syscall 461 …` | same |
| `/usr/bin/automationmodetool` | `rec_reason` only | `…lib.rs:944:38: … syscall 374 …` | `…lib.rs:946:38: … syscall 374 …` | same |
| `/usr/bin/desdp` | `rec_reason` only | `…lib.rs:944:38: … syscall 464 …` | `…lib.rs:946:38: … syscall 464 …` | same |
| `/usr/bin/dyld_info` | `rec_reason` only | `…lib.rs:944:38: … syscall 464 …` | `…lib.rs:946:38: … syscall 464 …` | same |
| `/usr/bin/flex` | `rec_reason` only | `…lib.rs:944:38: … syscall 464 …` | `…lib.rs:946:38: … syscall 464 …` | same |
| `/usr/bin/dddiagnose` | `rec_reason` only | `…lib.rs:944:38: … syscall 345 …` | `…lib.rs:946:38: … syscall 345 …` | same |
| `/bin/csh` | `landmark`, `rp_line` | 336 | 346 | the guest's own run-to-run spread |
| `/bin/tcsh` | `landmark`, `rp_line` | 344 | 341 | the guest's own run-to-run spread |

### The seven `rec_reason` moves are M38's fix commit, measured

Each of the seven is the same two-part difference inside the panic string the harness quotes, and
neither part is behavioural:

1. **The recorder's thread id** — `thread 'main' (35004702) panicked at …` vs
   `thread 'main' (46665839) panicked at …`. Rust prints the panicking thread's id; it is a fresh
   number per process and differs between any two runs of anything. M38's own rows carry seven
   distinct ids for this reason.
2. **`crates/retrace-arch/src/lib.rs:944` → `:946`** — the `forwarded_shape` fail-loud moved down
   two source lines. **M39 did not touch that crate**: `git diff --stat 832a1cb HEAD -- crates/retrace-arch`
   is empty. M38's sweep was run on `911214e`, and M38's own README says so and says it was *not*
   re-run after the final review's fix `cbc75ff`; `git diff --stat 911214e 832a1cb -- crates/retrace-arch/src/lib.rs`
   is `17 insertions(+), 15 deletions(-)`, a net `+2`, which is exactly the shift. So the number
   moved inside M38's close, between the binary M38 swept and the tree M38 merged, and this run is
   the first sweep taken on the merged tree.

The panic **site** (`forwarded_shape`), the syscall number, the label, `rc` and `rp` are identical
on all seven. Nothing about the wall changed.

### The two landmark moves are the documented guest spread

`/bin/csh` 336 → 346 and `/bin/tcsh` 344 → 341 — one row up, one row down. Both stop at the same
wall, `mach_msg2` msgh_id 3403 at the same pc `0x1804adc34`, with the same `rc`/`rp` of 4/3. M37's
audit 3 measured this run-to-run spread on every row and M38's README records it again on these two
(M37 run N had 331 and 329 against M38's 336 and 344). It is the guests' own nondeterminism, not
retrace's; the class-C verdict does not depend on the exact landmark.

## Ruling

**Acceptance met.** `pass=44` is M38's figure and the spec's floor (§6, "PASS count ≥ 44"). No row
moved its label, so there is no row for this milestone to explain by name and no row that moved for
a reason the spec does not name. M39's one wall (`mach_vm_remap`, 4813) is not reached by any corpus
binary — it is issued by libffi on `import ctypes`, and no binary in the corpus loads `_ctypes.so` —
so the sweep was expected to be flat and is. The nine differing rows are a thread id, a source line
number from M38's own post-sweep commit, and two landmarks inside the spread M37 measured.

## Files

- `sweep.log` — the full detached log: the wrapper's two header lines, 54 `ROW` lines,
  `TALLY pass=44 fail=10 skip=0`, `SWEEP_EXIT=0`.
- `sweep/<basename>.rec.err` (and `.rp.err`/`.rp.out` where a replay ran) for every non-clean row:
  `automationmodetool`, `csh` (+ `rp.err`, empty `rp.out`), `dddiagnose`, `desdp`, `dyld_info`,
  `ed`, `flex`, `ls`, `tcsh` (+ `rp.err`, empty `rp.out`), `yes`.
- **No `.bin` trace files are committed.** The ten the sweep kept were removed unread after the
  tally, the M37/M38 rule; nothing in this milestone's diff needed reading off a trace, since no
  row moved.
