# M36-sweepmeasure — nine rows read off kept evidence, and a parked gate for every wall

**Charter entry:** `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3, "M36 — `sweepmeasure`: measurement, and the gates the README already owes"; §6, the
`root_cause_class` enum; §5's exception for the gates M36 may park. Written autonomously from the
charter under §5's authority, on the operator's 2026-09-13 instruction to run the queue unattended
through M38. §9's contract: §1 (wall — here, the harness's blindness), §4 (the measurement is the
deliverable, and a first reading was taken before this spec), §8 (symmetry: none engaged), §6
(positive controls), §7 (deliberately not done).

**No behaviour change.** The charter forbids it and this spec repeats it: retrace's production
code is not edited. The edits are the sweep *harness* (`tools/apple-sweep.sh` — the charter's E1
routing is "fix or document", and the harness is not retrace), committed evidence, `#[ignore]`d
gates, and docs. A `crates/*/src` hunk in the branch diff is a defect.

## 1. The wall, located — the harness cannot say why a binary fails

`tools/apple-sweep.sh` at the M35 merge (`44d302a`):

| line | what it does | what it loses |
|---|---|---|
| `:107` | `run_timeout "$BIN" record-dyn "$g" -o "$TMP/t.bin" … 2>"$TMP/rec.err"; rc=$?` | `rc` is used only as a number to compare against `rp`; a `RECORD ERROR` exit (4) is never printed |
| `:119–122` | `grep -qa "panicked at" rec.err` → `FAIL … (recorder panicked)` | the panic's own message (which assert, which syscall) |
| `:131–132` | `grep -qa "DIVERGENCE at landmark" rp.err` → `FAIL … (replay diverged)` | that a replay of a recording which ended at a `RECORD ERROR` *always* prints a `DIVERGENCE` line (the trace has no terminal event, so the replay runs out of events, or reports the same exception the recorder could not record) — M35 measured this on `dddiagnose`, and §4 below measures it on all five "replay diverged" rows: **none of the five is a replay divergence** |
| `:135–136` | `rc == rp && cmp rec.out rp.out` → `PASS` | that `rc = rp = 139` is an identical *crash* counted as a pass with no note — M35 found those crashes on `dddiagnose` are retrace-induced (M34 §4b's mis-translated pid) |
| `:38–39` | `TMP=$(mktemp …); trap 'rm -rf "$TMP" …'` | every trace and both stderr files, for every binary, so nothing the sweep observed survives it |
| — | (absent) | the recorder's pid, which M34 §4b and M35 showed decides what several Apple binaries do |

That is the wall the charter names: "what the sweep reports is not why they fail". The five
`replay diverged` rows have been *believed* since M23 to reach a `brk`; nothing committed shows it.

## 2. Scope

1. **Harness** (`tools/apple-sweep.sh`): print the record and replay exit codes, the recorder's
   pid, the first `RECORD ERROR:` / `panicked at` line and the first `DIVERGENCE at landmark` line
   for every row; label an identical fault as `PASS … (identical fault, rc=N)` and a refused
   recording as `FAIL … (record error, rc=4)` rather than `replay diverged`; when
   `RETRACE_SWEEP_KEEP=<dir>` is set, copy each failing (and identical-fault) binary's `rec.err`,
   `rp.err` and trace into it. One machine-readable `ROW` line per binary beside the human line.
2. **Measurement**: the sweep run twice on the post-M35 tree with the recorder's pid steered
   *outside* and then *inside* `[0x4000, 0x10000)` (M34 §4b's collision range), evidence kept, and
   the `brk` pc symbolicated once against the host's copy of the shared cache.
3. **The table**: one row per binary that fails on either run, plus `dddiagnose` whether or not it
   fails (charter), classified from the kept evidence only.
4. **Committed evidence**: the decisive stderr lines per row under `docs/sweep-evidence/2026-09-13-m36/`
   (small text files; traces stay in the SDD workspace and are named by path).
5. **Parked gates**: one `#[ignore]`d end-to-end test per class-B or class-C row in a new
   `crates/retrace/tests/apple_walls_e2e.rs`, each reason carrying the measured evidence and
   naming what un-parks it. No gate for class D.
6. **Docs**: README "Known limits" (the sweep paragraph rewritten around the table), the status-log
   section, spec §11.

## 3. Design

### 3a. The harness

Per binary, after the record phase and (if it ran) the replay phase, the script derives:

- `recpid`: the recorder's pid, captured by wrapping the record invocation in
  `sh -c 'echo "recpid=$$" >&2; exec "$0" record-dyn …'` — M34's probe shape — so it appears in
  `rec.err` before anything the recorder prints.
- `rec_reason`: the first line of `rec.err` matching `RECORD ERROR:|panicked at`, or empty.
- `rp_line`: the first line of `rp.err` matching `DIVERGENCE at landmark`, or empty.
- `landmark`: the integer after `DIVERGENCE at landmark `, or `n/a`.

and prints two lines:

```
<PASS|FAIL|SKIP> <path> (<reason>)                         # the human line, as today, reasons below
ROW\t<path>\t<result>\t<rc>\t<rp>\t<recpid>\t<landmark>\t<rec_reason>\t<rp_line>
```

Result labels (the human line's parenthetical), in evaluation order:

| condition | label |
|---|---|
| record timed out | `FAIL … (timed out after Ns recording)` — unchanged |
| `rec.err` has `panicked at` | `FAIL … (recorder panicked: <line>)` |
| `rc = 4` (the CLI's `RECORD ERROR` exit) | `FAIL … (record error, rc=4: <line>)` — **new**; replay is still run so the `DIVERGENCE` line is captured, but it is not the label |
| replay timed out | unchanged |
| `rp.err` has `DIVERGENCE at landmark` and `rc ≠ 4` | `FAIL … (replay diverged at landmark N)` — the landmark added |
| `rc = rp ≠ 0` and stdout equal | `PASS … (identical fault, rc=N)` — **new**; still counted in `pass` so the tally is comparable with M33–M35's, and the note is on the line |
| `rc = rp = 0` and stdout equal | `PASS …` |
| otherwise | `FAIL … (record=rc replay=rp)` — unchanged |

`TALLY` gains nothing; the counts stay comparable with M33's 46/8 baseline. With
`RETRACE_SWEEP_KEEP=<dir>`, every non-clean row's `rec.err`, `rp.err` and `t.bin` are copied to
`<dir>/<basename>.{rec.err,rp.err,bin}` before the loop's next `rm -f`.

### 3b. The measurement

Two full runs of the sweep, evidence kept, on the M35-merge tree's binary:

- **Run O** (pid outside the range): the recorder's pid is whatever the machine's counter gives
  if it is outside `[0x4000, 0x10000)`; if it is inside, a spawn loop (`/usr/bin/true` in a
  `while` loop, ~50 ms per thousand) advances it past `0x10000` first.
- **Run I** (pid inside the range): the counter advanced into the range the same way (past
  `PID_MAX` = 99998 it wraps to a low number, so the loop is at most ~100k spawns).

Each run records its own pid range in the log. Rows whose result differs between O and I are the
§4b-confounded rows; the table records both.

The `brk` pc (§4: `0x18035f084`, the same on all five "replay diverged" binaries and on
`dddiagnose`'s in-range failures) is symbolicated once: the guest maps the shared cache at
slide 0 (`crates/retrace-box/src/lib.rs:1387`, `walk_page(…, 0 /* slide 0 */, …)`), so the pc is
an unslid cache address; `lldb -b -o 'process launch -s' -o 'image list -o -f' -o 'image lookup
-a <pc + host slide of libxpc>' /usr/bin/true` (or any equivalent) names the function. The
symbol goes into the table and the gate reasons.

### 3c. The table, and how each class is decided from evidence

The charter's six classes, with the *evidence test* this milestone uses for each:

| class | decided when |
|---|---|
| **A** retired-by-soundness | the row PASSes on both runs with `rc = rp = 0` — it failed at M31 and does not now |
| **B** known-unmodelled | `rec_reason` names a retrace fail-loud that a table entry or an already-designed arm closes: the M33 `dup2` assert (`crates/retrace-core/src/lib.rs:1140`); M34 §4b's pid mis-translation (the twelve self-pid `ESRCH` answers, M35) |
| **C** new-subsystem | `rec_reason` is the post-refusal `brk` in libxpc after the serviced `RefuseMqSend` (M23 t5: "the box hosts no message-queue receivers"), or the RCV-shaped message-queue `mach_msg2` `Route::Unsupported` — both need message-queue receivers / an XPC pipe the tree does not have |
| **D** not-a-defect | `/usr/bin/yes`: never terminates; the sweep kills it at 30 s on both sides by design |
| **E1** harness-nondeterministic | the label moved between two runs while `rec_reason`/`rp_line` did not (a labelling artefact); or a label the harness prints that its own evidence contradicts |
| **E2** retrace-nondeterministic | two recordings of the same binary at the same pid regime with different landmark sequences, or a replay that diverges from a *complete* recording (one with a terminal event) at different landmarks on repeated replays |

**On "Park + HALT" for class C (Ruling 1).** The charter's §6 routing for C is "Park + HALT. Do
not attempt." and §3's M37 scope is "whatever M36's table routes to class B". This spec reads
the two together: a C row is parked (its gate stands with the measured reason) and is **not
routed** to M37; the queue continues on the B rows. What halts is any attempt to *fix* a C row,
not the run. If every row were C, M37 would be empty and the run would end at M36 (charter §3).
Cost if wrong: the operator wanted the run to stop at the first C row; instead M37 runs on B rows
only, which are by construction the safe ones, and the C parks are exactly what a stop would have
left behind.

### 3d. The gates

`crates/retrace/tests/apple_walls_e2e.rs`: one `#[test]` per class-B or class-C row, every one
`#[ignore = "…"]`, each body the same shape — `util::record_dynamic(<path>)` then `util::replay`,
asserting record exit 0, replay exit 0 and byte-equal stdout, i.e. the statement that becomes true
when the wall falls. The reason string is the gate's documentation and carries, per the charter's
condition 2, the **measured** evidence: the run's label, `rc`/`rp`, the recorder-pid regime, the
`rec_reason` line (with the `brk` symbol), the landmark, the evidence file's path, the class, and
"UN-IGNORE when <what un-parks it>". Condition 3 (never park a passing test) is satisfied by
construction — every test is new and fails today; §6 runs each once with `--ignored` to show it.

`/usr/bin/yes` (class D) gets no gate: a test for "records to a clean exit" of a program that
never exits would be a parked lie.

`dddiagnose` gets one gate, parked at the *first* of its three walls in fix order (§4b), whose
reason names all three and the order.

## 4. Measurement — a first reading, taken before this spec was written

On 2026-09-13 after the M35 merge, the M35 branch's binary (`12ac4e7`, identical code paths to
`44d302a`) recorded and replayed the five "replay diverged" binaries and `csh`, with stderr kept
(scratchpad `m36pre/`; recorder pids `0x10806`–`0x10887`, **outside** the collision range):

| binary | rc | rp | `rec_reason` (first line) | `rp_line` |
|---|---|---|---|---|
| `/bin/launchctl` | 4 | 3 | `RECORD ERROR: non-syscall exit: exception (EC=0x3c ISS=0x1 FSC=0x1) far/ipa=0x0 (UNMAPPED) pc=0x18035f084 elr=0x1804af110` | `DIVERGENCE at landmark 327 pc=0x18035f084: non-syscall exit: exception (EC=0x3c …)` |
| `/usr/bin/automationmodetool` | 4 | 3 | same line, same pc | landmark 334 |
| `/usr/bin/desdp` | 4 | 3 | same | landmark 364 |
| `/usr/bin/dyld_info` | 4 | 3 | same | landmark 366 |
| `/usr/bin/flex` | 4 | 3 | same | landmark 364 |
| `/bin/csh` | 101 | 3 | `thread 'main' … panicked at crates/retrace-core/src/lib.rs:1140:17` (the M33 `dup2` assert) | `DIVERGENCE at landmark 274 pc=0x1804b67bc: expected recorded syscall, got None (truncated=false)` |

So: **all five "replay diverged" rows are the same `brk`** — `EC=0x3c` at `pc=0x18035f084`,
`elr=0x1804af110`, the recorder exiting 4 with `RECORD ERROR`, and the replay reporting the same
exception at the same pc as its "divergence" — the M23 belief confirmed by measurement, and the
sweep's label wrong for all five (an E1 defect, §1). `dddiagnose`'s in-range failures (M35,
`ddd-keep-inrange/`) hit the identical pc. What the `brk` is, by symbol, is §3b's job; that it
follows M23's serviced `RefuseMqSend` is in every `rec.err` (`[retrace] refusing mach_msg2
message-queue send … the box hosts no message-queue receivers` precedes the `RECORD ERROR`).

`csh`'s "replay diverged" is likewise an artefact: the recorder panicked, the trace has no
terminal event, and the replay ran out of events — the harness already labels this one
"recorder panicked" because it checks `panicked at` first; the `rc=4` path has no such check.

This first reading is not the deliverable — §3b's two full runs are — but it fixes the design:
the labels §3a adds are the ones these six rows needed, and the classes §3c will assign are
visible already (C for the five, B for `csh`/`tcsh`, B-then-C for `dddiagnose`, D for `yes`).

## 5. What must change

### 5a. `tools/apple-sweep.sh`

The §3a derivations and labels; the `recpid` wrapper; the `ROW` line; the `RETRACE_SWEEP_KEEP`
copy; the header comment updated (it currently documents the `rc`/`rp` scoping bug and the
`RETRACE_DEREFLEN`/`RETRACE_CANARY` surfacing — keep both, add this). `sh -n` clean. The
`TALLY` line unchanged in shape.

### 5b. `docs/sweep-evidence/2026-09-13-m36/`

Per row of the table: `<basename>.O.rec.err`, `<basename>.O.rp.err`, and the `.I.` pair for the
in-range run, copied verbatim from the kept evidence (they are ≤ 1 KB each without
`RETRACE_TRACE`); plus `README.md` naming the binary commit, the two runs' pid ranges, and the
`brk` symbolication command and its output. Traces are not committed (30 MB each); the SDD
workspace keeps them and the table's `evidence` column names both paths.

### 5c. `crates/retrace/tests/apple_walls_e2e.rs` (new)

§3d. Uses `crates/retrace/tests/util/mod.rs` (`record_dynamic`, `replay`). Every test
`#[ignore = "<reason>"]`. No test in the file runs un-ignored, so the gate's running count is
unchanged and its ignored count rises by the number of gates.

### 5d. Docs

- README: the sweep paragraph(s) under "Known limits" rewritten around the table — one line per
  row with its class and the gate that stands for it; the "what the sweep reports is not why they
  fail" sentence retired (it is now false: the sweep prints why); the `dddiagnose` entry folded
  into the table's row; the gate paragraph at M36's figures (ignored count up by the gates).
- `docs/status-log.md`: a new section with the table, the two runs, the symbolication, the
  classes with their evidence tests, the E1 defects fixed, the parked gates, the M37 routing
  (the class-B rows in order), what stays owed.
- This spec's §11.

## 6. Positive controls

1. **The harness sees what it used to discard.** A three-binary list (`/usr/bin/true`,
   `/bin/csh`, `/bin/launchctl`) run through the new script prints `PASS /usr/bin/true`,
   `FAIL /bin/csh (recorder panicked: … lib.rs:1140 …)`, `FAIL /bin/launchctl (record error,
   rc=4: RECORD ERROR: non-syscall exit … pc=0x18035f084 …)` and three `ROW` lines with non-empty
   `recpid`; with `RETRACE_SWEEP_KEEP` set, six files appear. Mutation: the *old* script on the
   same list prints `FAIL /bin/launchctl (replay diverged)` — the label this milestone retires.
2. **Every parked gate fails for the measured reason.** `cargo test -p retrace --test
   apple_walls_e2e -- --ignored --test-threads=1` — each test red, each failure message carrying
   the same `rec_reason` its `#[ignore]` reason quotes (the assertion message prints the recorder's
   stderr tail). Recorded in the Task 3 report; this is what makes each reason "measured
   evidence" rather than a belief.
3. **The identical-fault label.** `dddiagnose` on run I, if it takes the crash path: `PASS
   /usr/bin/dddiagnose (identical fault, rc=139)` — the row the old script printed as a bare
   `PASS`.

## 7. What this milestone deliberately does not do

- **No fix of any row.** Not `dup2`, not §4b, not the message-queue receivers, not the RCV-shaped
  `mach_msg2`. The charter forbids it and M37 is where B rows go.
- **No symbolication inside retrace** for cache pcs (M19's parked `cache_symbol_e2e` stays
  parked); the host's `lldb` is used once, by hand, for the table.
- **No new class.** A row that fits none of the six is a halt, not an invention.
- **No gate for class D**, and no gate for a row that passes on both runs (condition 3).
- **No `RETRACE_TRACE=1` sweep**: the per-trap firehose is not needed to classify these rows
  (the `RECORD ERROR`/panic line and the landmark are enough); it stays a per-row tool.

## 8. Symmetry obligation

None engaged: no trap arm, no `Box_` method, no trace-format change. `apple_walls_e2e.rs` drives
the CLI like every other e2e.

## 9. Rulings

- **Ruling 1** (§3c): class C rows are parked and not routed; the run continues on B rows.
- **Ruling 2:** the `identical fault` rows count as `pass` in the tally so the 46/8 ↔ 45/9 series
  stays comparable across M33–M36; the note on the line is the correction, and the status log
  says which PASS rows are faults. Cost if wrong: a reader of the bare tally still over-counts
  passes by the number of identical faults — the same over-count every earlier milestone made,
  now labelled.
- **Ruling 3:** M36 parks gates under the charter's §5 exception and its four conditions; the
  "any need to park a NEW `#[ignore]`" halt rule is not triggered by them. Any gate that fails a
  condition (a row outside the 54-entry corpus; a reason without measured evidence; a passing
  test; no un-park clause) is a halt.

## 10. Gate

The full chunked gate in CLAUDE.md's shape, `#[test]` reconciled file-by-file against M35's
575 / 0 / 2 over 125. Prediction: `apple_walls_e2e.rs` adds N ignored tests in one new binary
(N = the number of B/C rows; from §4, expected 8: `csh`, `tcsh`, `launchctl`,
`automationmodetool`, `desdp`, `dyld_info`, `flex`, `dddiagnose`) → **575 / 0 / 10 over 126** if
N = 8, running count unchanged; the two runs' tallies as measured (§3b), not predicted.

## 11. Outcome

Written 2026-09-13 at the close. Every number below is copied from the SDD workspace's
`sweep-table.md`, the committed `docs/sweep-evidence/2026-09-13-m36/README.md`, the task reports
and the controller's numbers file; none is recomputed here. The status-log section
(`docs/status-log.md`, "Status: M36-sweepmeasure") carries the table verbatim, the symbolication
output, the controls and the rulings; this section is the outcome against what the spec expected.

### 11.1 The outcome against §4's expectation

§4 predicted the classes: "C for the five, B for `csh`/`tcsh`, B-then-C for `dddiagnose`, D for
`yes`". Measured:

| row | §4 expected | measured | evidence |
|---|---|---|---|
| `csh`, `tcsh` | B (`dup2`) | **B** — the M33 assert at `crates/retrace-core/src/lib.rs:1140:17` in all three regimes; 5 / 0 / 5 self-pid `ESRCH` in the kept traces, the wall unchanged | `…/{csh,tcsh}.{O,L,I}.rec.err` |
| `launchctl`, `automationmodetool`, `desdp`, `dyld_info`, `flex` | **C**, "the M23 belief confirmed by measurement" | **B then C** — `dddiagnose`'s shape. Colliding pid: the `brk` (libdispatch `_firehose_task_buffer_init+0x12c`, `elr` = `__proc_info+8`, after `proc_info(2, pid, 17)` → `ESRCH`; 11 self-pid `ESRCH`). Non-colliding pid: the RCV-shaped `mach_msg2` (`options 0x404000102`, `Route::Unsupported`, pc `mach_msg2_trap+8`; 0 `ESRCH`) | `…/<b>.{O,L,I}.{rec,rp}.err`, the symbolication |
| `dddiagnose` | B then C | **B then C**, with three faces: L the RCV shape (63 `err`, 0 `ESRCH`); O the `brk` (75, 12); I an identical malloc crash `pc=0x180302eb0` `mfm_alloc+0x230` `far=0x2000050050` (71, 11), `PASS (identical fault, rc=139)` | `…/dddiagnose.{O,L,I}.{rec,rp}.err` |
| `yes` | D | **D** — `rc=137`, the 30 s watchdog, every run | `…/yes.{O,L,I}.rec.err` |
| A | — | **none** | no M35-failing row passes `rc = rp = 0` on any run |
| E1 | the two M35 items | **fixed in harness** (`fc53853`, `58fb0e7`); both labels visible in the logs | `sweep-{O,L,I}.log` |
| E2 | — | **none** — `dddiagnose`'s crash/`brk` split is between runs whose forwarded `gettimeofday` replies differ before the fork at #249, and record == replay within every run | `keep-{O,I}/dddiagnose.bin` |

The prediction for the five `launchctl`-group rows was wrong, and it was wrong because §4's first
reading was taken at recorder pids `0x10806`–`0x10887` — believed outside the collision range and
in fact inside the wider one (11.3a) — so it saw the `brk` on all five and read it as M23's wall.
The symbol retired that reading (11.3b). Class B is therefore not empty, as §3c's Ruling 1
contemplated it might be: **eight of the nine rows carry a class-B wall**, and M37's scope is the
two B fixes — `dup2` on two rows, §4b on six.

### 11.2 The runs — three, not two

§2 and §3b asked for two runs, O (pid outside `[0x4000, 0x10000)`) and I (inside). Run O was made
as specified — every recorder pid in 73437–75432 (`0x11EDD`–`0x126A8`), above `0x10000` — and
its `dddiagnose` row came out with the in-range signature. The kept trace showed why (11.3a), a
third run **L** was added at pids 812–2851 (`0x32C`–`0xB23`), below `0x4000`, and nothing was
re-run or discarded. Tallies: O `pass=45 fail=9 skip=0`, L `45/9/0`, I (17209–19410,
`0x4339`–`0x4BD2`) `46/8/0`; the same 45 clean rows on all three; every `ROW` pid inside its run's
range; each run about four minutes. The labels that differ between runs: O vs I on `dddiagnose`
only (the `brk` → the identical fault); O vs L on exactly the six `rc=4` rows (the `brk` → the RCV
shape). Every number in §3b's "the two runs" is superseded by these three; §5b's `.O.`/`.I.` file
pairs are `{O,L,I}` — 46 files, 46,778 bytes, committed.

### 11.3 Corrections — two beliefs retired by measurement, one reason corrected

These are stated as corrections with their evidence, not as things always known. The status log
carries them with forward pointers from M23's, M34's and M35's lines, which stand unedited.

**(a) M34 §4b's window.** `[0x4000, 0x10000)`, "roughly half the pid space", was computed from the
fixed trampoline/page-table backings. Measured on 8 binaries × 2 colliding runs (16 of 16 kept
traces): a `mach_vm_map(0x8000, flags 0x49000001)` — tag 73, `VM_MEMORY_OS_ALLOC_ONCE` — is
first-fit-placed at IPA `0x10000` (the gap between `PT_L1_IPA`'s end and the TSD region) before
the guest's first self-pid call (#182 → #200 in `dddiagnose`, #136 → #154 `launchctl`,
#144 → #162 `automationmodetool`, #128 → #146 the other five), on record and replay alike. So the
window at the pid-carrying calls is **`[0x4000, 0x18000)` = pids 16384..=98303, about 82 % of the
pid space**; the non-colliding pids are 1..16383 and 98304..99998 (`PID_MAX` 99999, `nextpid`
reset at `>=`; the upper band inferred from run I's final snapshot — nothing mapped in
`[0x18000, 0x28000)`, everything the guest maps later at ≥ `0x40000`, above `PID_MAX` — no run
used a pid ≥ `0x18000`). It is guest-dependent — whatever the guest maps below `0x100000` before its own
pid-carrying calls — so "outside one band" is not a regime, and M37's §4b precondition is the one
M34's own text names: a `Scalar` argument is never a pointer; its positive control must use a pid
inside `[0x10000, 0x18000)` as well as one inside `[0x4000, 0x10000)`.

**(b) M23's `brk`.** §3c's C test named "the post-refusal `brk` in libxpc after the serviced
`RefuseMqSend`"; §4 called the five rows' `brk` "the M23 belief confirmed by measurement".
`dladdr` on the slid address: `/usr/lib/system/libdispatch.dylib`, `_firehose_task_buffer_init +
0x12c`; the word at the pc is `0xd4200020` = `brk #1`, the last word of an outlined block after the
function's `retab` that loads errno from the TSD's errno slot into a crash-reason store — the shape
of a `DISPATCH_INTERNAL_CRASH(errno, …)`; the `elr` `0x1804af110` is libsystem_kernel
`__proc_info + 8`; and in every `brk` trace (11 of the 12 colliding traces; `dddiagnose` run I is
the crash, not a `brk`) the last landmark is
`proc_info(2, <recorder pid>, 17 = PROC_PIDUNIQIDENTIFIERINFO)` → `ESRCH`. With a non-colliding
pid all six rows reach the RCV-shaped call instead (run L 6 of 6; M35's `dddiagnose` 10 of 10).
**The `brk` has never been observed with a correctly-forwarded pid.** The serviced refusal precedes
it in time and is not its cause. So the `brk` is a §4b consequence, the five rows are B then C,
and the C test's first clause was met by no row. Not claimed: whether a `brk` of M23's kind lies
behind the RCV shape; unmeasurable until the shape is modelled. `dladdr` names exported symbols
only; the instruction words and the `elr` are the tie to `proc_info`.

**(c) M35's "out-of-range → RCV wall".** Right conclusion, wrong reason: M35's out-of-range probe
pids were `0x257f`–`0x2662`, *below* `0x4000`, not above `0x10000` — the same regime as run L, and
not the one run O fell in.

### 11.4 `dddiagnose`'s three faces, and why the split is not E2

L: the RCV shape, 63 `err = true` landmarks, 0 self-pid `ESRCH`, last landmark `issetugid` #378
(the refused call is the stop and is never recorded; replay reports 379, one past the end). O: the
`brk`, 75 = 63 + 12, last landmark `proc_info(2, pid, 17)` → `ESRCH` #378. I: the identical crash,
71 landmarks, 11 `ESRCH` (the trace ends 22 landmarks before L's wall and lacks three of L's
other failures and the twelfth self-pid call), last landmark `csops(pid, 0, …)` → `ESRCH` #356.
The O and I sequences are identical through #248 and fork at #249 inside a `gettimeofday` polling
loop: O 16 iterations (#240–#255, `tv_usec` 327863 → 384503), I 9 (#240–#248, 271784 → 301139),
different `tv_sec` (1789325229 vs 1789326331); nothing traps between iterations, so the only
recorded input in the loop is the forwarded `gettimeofday` reply, which differs before the fork.
Record and replay agree bit-for-bit within each run. §3c's E2 test is not met; the charter's is
not met. Task 3's control added a `far` of `0x6000050040` at pid `0x6d61` against run I's
`0x2000050050`, same pc and esr, and the controller's replay of that trace reproduced it
bit-for-bit — forwarded-input dependence again. Why an in-range run takes the crash rather than
the `brk` stays open, attached to the §4b row.

### 11.5 The harness, the gates, the controls

- **§3a corrections.** The `identical fault` condition is `rc = rp ≥ 128` (a signal death on both
  sides), not `rc = rp ≠ 0`: `/usr/bin/false` exits 1 on both sides by design and was being
  labelled a fault (Task 1 ruling (a)). A panic's `rec_reason` is the `panicked at` line joined
  with the message line after it, not the location line alone (ruling (b)). `rc = 4` is
  evaluated before the replay-timeout marker, as §3a's table orders it, not as the plan's code
  did. `RETRACE_SWEEP_LIST` was added beside `RETRACE_SWEEP_KEEP`. In the final-review fix wave,
  after the gate: the record-error label is structural — it fires on the `RECORD ERROR:` line,
  not on `rc = 4`, which §3a wrote and which the CLI shares with a guest's own exit status — with
  `rc` printed as corroboration; both panic greps anchor on `panicked at crates/`; the pid comment
  states the measured window. `sh -n` clean, Control 1 re-run on the edited script (the log's
  harness subsection has the lines); the harness is not a cargo input, so the gate stands.
- **§6 Control 1.** Old script: `FAIL /bin/launchctl (replay diverged)`, `FAIL /bin/csh (recorder
  panicked)`. New: `FAIL /bin/launchctl (record error, rc=4: RECORD ERROR: non-syscall exit: …
  pc=0x18035f084 …)`, `FAIL /bin/csh (recorder panicked: … lib.rs:1140:17: …)`, three `ROW` lines
  of nine fields with numeric `recpid`. The kept-file count is **five**, not the spec's six:
  `csh.rp.err` cannot exist because `csh`'s replay never ran, and the plan's `cp` would have
  copied the previous row's file under that name — a plan defect, fixed.
- **§3d / §6 Control 2.** `crates/retrace/tests/apple_walls_e2e.rs`: eight `#[ignore]`d gates (55
  lines), `0 passed; 0 failed; 8 ignored` under the gate and `0 passed; 8 failed` under
  `--ignored` (cargo exit 101, recorder pids `0x6d48`–`0x6d7f`, the trampoline regime): `csh`/
  `tcsh` exit 101 at the `dup2` assert, the five `launchctl`-group rows exit 4 at the `brk`,
  `dddiagnose` exit 139 at the identical crash. Every reason carries the label, `rc`/`rp` per face,
  the three pids and regimes, the recorder's line with its symbol, the landmark, the evidence
  file, the class, and `UN-IGNORE when …`. No reason carries "replay diverged".
- **§6 Control 3.** Met by run I: `PASS /usr/bin/dddiagnose (identical fault, rc=139)`.
- **§3b's `lldb` command** could not be used (SIP refused the attach on `/usr/bin/true`; on a
  scratchpad no-op it attached and hung ten minutes at `image list`; `atos -p` hung the same way).
  §7's "the host's `lldb` is used once, by hand" is therefore `dladdr(3)` from a process of my own
  plus a raw read of the instruction words; source and output are in the evidence README.

### 11.6 Rulings

Spec Rulings 1–3 held as written: class C parked and not routed, the run continued on B; the
identical-fault row is counted in `pass` and named in the log; the gates were parked under §5's
exception, and none failed a condition. The run's further rulings, in the ledger and the log:
Task 1's two (the `≥ 128` threshold; the joined panic pair); Task 2's mid-run ruling to keep all
three runs, class `dddiagnose` run I as B and never a pass, and correct M34 §4b with a forward
pointer; the Task 3 pre-dispatch amendment retiring the plan's reason templates; the `far` ruling
(record == replay within the run, forwarded-input dependence, one clause added to the `dddiagnose`
reason); three scoped re-reviews replaced by the controller's own checks; the gate launched on
`0766a76` with Task 4 docs-only; Task 4's amendment (three runs everywhere; corrections stated
as corrections; B → M37, C parked, D retired; gate figures only from the measured section); and,
at close, M37's two acceptance criteria ruled in (`dddiagnose`'s twelve self-pid calls answer `0`
after the §4b fix from any pid; all six rows then sit at the RCV wall) — they follow from the
measurement but are not a measurement.

### 11.7 Routing

Class B → M37: `csh`, `tcsh` (`dup2` in the M10 fd table); `launchctl`, `automationmodetool`,
`desdp`, `dyld_info`, `flex`, `dddiagnose` (M34 §4b — a `Scalar` argument is never a pointer;
positive control at a pid inside `[0x10000, 0x18000)` as well as `[0x4000, 0x10000)`). Class C
(the RCV-shaped message-queue `mach_msg2` behind the same six) — parked, not routed. Class D
(`yes`) — retired, no gate. A, E2 — none. E1 — fixed in harness.

### 11.8 Gate

§10 predicted 575 / 0 / 10 over 126 if N = 8. Measured on `0766a76` (the last code commit; Task 4
is docs only): **575 passed / 0 failed / 10 ignored across 126 test binaries**, every chunk
`EXIT=0` (`ws=0 box=0 e2e1=0 e2e2=0 e2e3=0 e2e4=0 bins=0 clippy=0`), wall clock 15:53:23 →
16:11:58 EDT (18.5 min), zero `SKIPPED` lines; chunks `ws` 152 / `box` 271 / `e2e1`–`e2e4`
43 + 32 + 63 + 3 = 141 over 62 targets (the 8 new ignored in `e2e1`, the 2 old in `e2e3`) /
`bins` 11; clippy clean. Reconciled file-by-file against M35's 575 / 0 / 2 over 125: one new file,
`apple_walls_e2e.rs`, +8 `#[test]` all `#[ignore]`d; every other `.rs` under `crates/` unchanged;
583 attributes = 573 runnable + 10 ignored; 575 passed = 573 + census's 2. The run matched the
prediction exactly. N = 8, as §10 expected.

### 11.9 What the spec got wrong, in one list

§2/§3b "two runs" (three); §3a `rc = rp ≠ 0` (`≥ 128`) and the panic line (joined with its
message); §3b the `lldb` command (`dladdr`); §3c's C test first clause (met by nothing — the `brk`
is B) and its B test's "twelve" (11 or 12, by whether the trace reaches the twelfth call); §4's
"C for the five" and "the M23 belief confirmed by measurement" (retired: B then C, the reading
was taken inside the slab); §5b "two runs' pid ranges" and `.O.`/`.I.` (three, `{O,L,I}`); §6
Control 1 "six files" (five); §7 "the host's `lldb` is used once" (`dladdr`); §3a's "`rc = 4`
(the CLI's `RECORD ERROR` exit)" as the label's condition (the CLI passes a guest's own exit
status through, so the line is the test — fix wave); and §3c's E2 evidence test ("two recordings
… at the same pid regime with different landmark sequences"), which any wall-clock-polling guest
would meet across two runs — the test actually applied (§11.4) is the charter's, record versus
replay varying within a run, and a successor spec should write the charter's. §10's prediction
held.
