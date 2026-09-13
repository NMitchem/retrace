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

*(Written at close.)*
