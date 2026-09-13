# M36-sweepmeasure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Apple sweep print *why* a binary fails and keep the evidence; run it twice (recorder pid outside and inside M34 §4b's collision range); classify every failing row from that evidence into the charter's six classes; commit the decisive stderr per row; park one `#[ignore]`d gate per class-B/C row with the measured reason; route the class-B rows to M37. **No retrace behaviour change.**

**Architecture:** Four tasks. The harness (`tools/apple-sweep.sh`, POSIX `sh`) gains a recorder-pid wrapper, three derived fields, two new labels, a `ROW` line and an opt-in evidence directory. The measurement is two full sweeps plus one `lldb` symbolication, producing a table in the SDD workspace and small committed evidence files. The gates are one new e2e file whose every test is `#[ignore]`d with the table's evidence in the reason. Docs close it.

**Tech Stack:** POSIX `sh`, Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, `lldb` (Xcode command-line tools) for one symbolication. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-13-retrace-m36-sweepmeasure-design.md`

## Global Constraints

- **No edit under `crates/*/src`.** The only Rust file this milestone adds is
  `crates/retrace/tests/apple_walls_e2e.rs`; a hunk anywhere in `crates/*/src` is a defect (charter
  §3 M36: "a behavioural change appearing in an M36 diff is a defect in the run").
- **`--test-threads=1`** on every `cargo test`; target pinned by `.cargo/config.toml`.
- **Classes come from evidence** the sweep kept, never from its old category string or M23's `brk`
  belief (charter §3 "one trap"). The evidence test per class is spec §3c; a row that fits none
  is a halt, not a new class.
- **Every parked gate meets the charter's four conditions** (spec §3d, Ruling 3): a corpus binary;
  measured evidence in the reason; never a passing test; "UN-IGNORE when …" named. The `#[ignore]`
  reasons quote the table; the table quotes the files under `docs/sweep-evidence/2026-09-13-m36/`.
- **No gate for class D** (`/usr/bin/yes`).
- **The tally stays comparable**: identical faults still count as `pass` (Ruling 2), labelled.
- **Do not push.** Commit and merge locally only. No `TRACE_MAGIC` bump (nothing touches it).
- The Bash tool ceiling is 10 minutes: a full sweep takes ~10–12 minutes, so Task 2 starts each
  run with `nohup … &` and polls its log with bounded `until` loops; never run a sweep in the
  foreground.

---

### Task 1: The harness sees what it used to discard

**Files:**
- Modify: `tools/apple-sweep.sh` (header comment `:1–10`; the record invocation `:107`; the
  result ladder `:118–139`; the tail `:140–145`)

**Interfaces:**
- Produces: per binary, the human line with the spec §3a labels and one `ROW` line:
  `ROW<TAB>path<TAB>result<TAB>rc<TAB>rp<TAB>recpid<TAB>landmark<TAB>rec_reason<TAB>rp_line`
  (`result` ∈ `PASS|FAIL|SKIP`; `landmark` an integer or `n/a`; the two lines may be empty).
  With `RETRACE_SWEEP_KEEP=<dir>`: `<dir>/<basename>.rec.err`, `.rp.err`, `.bin` for every row
  whose result is not a clean `PASS`. `RETRACE_SWEEP_LIST=<file>` overrides the corpus list (for
  the control and for probes).

- [ ] **Step 1: Read the script once, whole**, then make the edits below in place. `sh -n
  tools/apple-sweep.sh` after each.

- [ ] **Step 2: The list override and the evidence directory** — after `LIST=$ROOT/tools/apple-sweep-binaries.txt`:

```sh
# M36: a caller may sweep a different list (a three-binary control, a single-row probe) and may
# keep every non-clean row's evidence. Both opt-in; the defaults are the committed corpus and
# nothing kept — the EXIT trap still destroys $TMP.
LIST=${RETRACE_SWEEP_LIST:-$LIST}
KEEP=${RETRACE_SWEEP_KEEP:-}
if [ -n "$KEEP" ]; then mkdir -p "$KEEP" || { echo "TALLY ABORTED (cannot create $KEEP)"; exit 2; }; fi
```

- [ ] **Step 3: The recorder's pid** — replace the record invocation

```sh
    run_timeout "$BIN" record-dyn "$g" -o "$TMP/t.bin" >"$TMP/rec.out" 2>"$TMP/rec.err" </dev/null; rc=$?
```

with

```sh
    # M36: the recorder's pid decides what several Apple binaries do (M34 §4b: a pid inside
    # [0x4000, 0x10000) is forwarded as a host pointer by forward_and_diff's per-register probe,
    # so every self-pid csops/proc_info answers ESRCH; M35 measured dddiagnose taking a different
    # wall on each side of that line). Print it into rec.err before the recorder prints anything,
    # from the shell that becomes the recorder — M34's probe shape.
    run_timeout sh -c 'echo "recpid=$$" >&2; exec "$0" record-dyn "$1" -o "$2"' "$BIN" "$g" "$TMP/t.bin" >"$TMP/rec.out" 2>"$TMP/rec.err" </dev/null; rc=$?
    recpid=$(grep -a '^recpid=' "$TMP/rec.err" | head -1 | cut -d= -f2)
```

(`run_timeout` launches `"$@"` and kills the child on the watchdog; with `exec` the shell *is*
the recorder, so the kill still lands on the recorder. Check `run_timeout`'s body to confirm it
runs `"$@"` verbatim; if it wraps the command in another `sh -c`, adjust so `$$` is the
recorder's pid, and say so in the report.)

- [ ] **Step 4: The derived fields and the `ROW` line** — replace the result ladder from
  `if [ -e "$TMP/.timedout" ]; then` (record timeout) through `echo "FAIL $g (record=$rc replay=$rp)"; fail=$((fail+1))` / `fi` with:

```sh
    # M36: derive what the old ladder threw away, before deciding anything.
    rec_reason=$(grep -a -m1 -E 'RECORD ERROR:|panicked at' "$TMP/rec.err" | cut -c1-200)
    rp_line=""; landmark="n/a"; rp="n/a"
    keep_row() { # keep_row <result> — copy evidence when asked; emit the ROW line
        if [ -n "$KEEP" ] && [ "$1" != "PASS" ] || [ -n "$KEEP" ] && [ -n "${2:-}" ]; then
            b=$(basename "$g")
            cp "$TMP/rec.err" "$KEEP/$b.rec.err" 2>/dev/null
            [ -f "$TMP/rp.err" ] && cp "$TMP/rp.err" "$KEEP/$b.rp.err" 2>/dev/null
            [ -f "$TMP/t.bin" ] && cp "$TMP/t.bin" "$KEEP/$b.bin" 2>/dev/null
        fi
        printf 'ROW\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$g" "$1" "$rc" "$rp" "$recpid" "$landmark" "$rec_reason" "$rp_line"
    }
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $g (timed out after ${TIMEOUT_SECS}s recording)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if grep -qa "panicked at" "$TMP/rec.err"; then
        echo "FAIL $g (recorder panicked: $rec_reason)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    run_timeout "$BIN" replay "$TMP/t.bin" >"$TMP/rp.out" 2>"$TMP/rp.err" </dev/null; rp=$?
    rp_line=$(grep -a -m1 'DIVERGENCE at landmark' "$TMP/rp.err" | cut -c1-200)
    case "$rp_line" in
        'DIVERGENCE at landmark '*) landmark=$(printf '%s' "$rp_line" | sed 's/^DIVERGENCE at landmark \([0-9]*\).*/\1/') ;;
    esac
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $g (timed out after ${TIMEOUT_SECS}s replaying)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    # M36: a recorder that exited 4 printed `RECORD ERROR:` and wrote a trace with no terminal
    # event, so its replay ALWAYS prints a DIVERGENCE line (it runs out of events, or reports the
    # exception the recorder could not record). That line is evidence, not the label: M35 measured
    # dddiagnose's "replay diverged" this way, and M36's first reading found all five of the
    # long-standing "replay diverged" rows are the same recorder-side brk. Label the record error.
    if [ "$rc" -eq 4 ]; then
        echo "FAIL $g (record error, rc=4: $rec_reason)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    # Divergence detection is structural, not inferred from exit codes alone: retrace
    # always prints this on a divergence, so check for it directly rather than relying
    # on the exit-code compare below to happen to disagree.
    if [ -n "$rp_line" ]; then
        echo "FAIL $g (replay diverged at landmark $landmark)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if [ "$rc" -eq "$rp" ] && cmp -s "$TMP/rec.out" "$TMP/rp.out"; then
        if [ "$rc" -ne 0 ]; then
            # M36: an identical FAULT on both sides is still counted a pass (the tally stays
            # comparable with M33–M35's), but it is said on the line: M35 found dddiagnose's
            # rc=139 passes are retrace-induced crashes (M34 §4b), not the guest's own.
            echo "PASS $g (identical fault, rc=$rc)"; pass=$((pass+1)); keep_row PASS fault; continue
        fi
        echo "PASS $g"; pass=$((pass+1)); keep_row PASS; continue
    fi
    echo "FAIL $g (record=$rc replay=$rp)"; fail=$((fail+1)); keep_row FAIL
```

Keep the existing `RETRACE_DEREFLEN`/`RETRACE_CANARY`/`RETRACE_BANDSHRINK` surfacing blocks where
they are (they run before the ladder). The `SKIP` branch is unchanged. `keep_row PASS fault`
passes a second argument so an identical fault's evidence is kept too.

- [ ] **Step 5: The header comment** — extend `:1–10` with the M36 lines (labels, `ROW`,
  `RETRACE_SWEEP_KEEP`, `RETRACE_SWEEP_LIST`, the pid), keeping the M29/M30 history.

- [ ] **Step 6: Control 1 — the three-binary list, new script vs old**

```sh
printf '%s\n' /usr/bin/true /bin/csh /bin/launchctl > /tmp/m36-list.txt
cargo build -p retrace
K=/tmp/m36-keep; rm -rf $K
RETRACE_SWEEP_LIST=/tmp/m36-list.txt RETRACE_SWEEP_KEEP=$K tools/apple-sweep.sh 2>&1 | tee /tmp/m36-new.log
ls $K
git show main:tools/apple-sweep.sh > /tmp/old-sweep.sh; chmod +x /tmp/old-sweep.sh
# the old script has no list override: run it on the full corpus is too slow — instead
# temporarily point its LIST at the three-binary file:
sed -i '' "s#^LIST=.*#LIST=/tmp/m36-list.txt#" /tmp/old-sweep.sh
cp /tmp/old-sweep.sh tools/apple-sweep-old.sh   # ROOT is derived from $0's directory
tools/apple-sweep-old.sh 2>&1 | tee /tmp/m36-old.log; rm -f tools/apple-sweep-old.sh
```

Expected new: `PASS /usr/bin/true`; `FAIL /bin/csh (recorder panicked: thread 'main' … panicked
at crates/retrace-core/src/lib.rs:1140:17 …)`; `FAIL /bin/launchctl (record error, rc=4: RECORD
ERROR: non-syscall exit: exception (EC=0x3c …) pc=0x18035f084 …)`; three `ROW` lines with a
numeric `recpid` and `landmark` `n/a`/`274`-ish/`3xx`; `TALLY pass=1 fail=2 skip=0`; `$K` holds
`csh.rec.err csh.rp.err csh.bin launchctl.rec.err launchctl.rp.err launchctl.bin` (six files).
Expected old: `FAIL /bin/launchctl (replay diverged)` — the retired label. Paste both logs into
the report. **Make sure `tools/apple-sweep-old.sh` is deleted and not committed.**

- [ ] **Step 7: Commit**

```bash
sh -n tools/apple-sweep.sh && git add tools/apple-sweep.sh && git commit -m "M36 t1: the sweep prints why a row fails, keeps its evidence, and logs the recorder's pid"
```

---

### Task 2: Two runs, one symbolication, the table, the committed evidence

**Files:**
- Create: `docs/sweep-evidence/2026-09-13-m36/README.md` and per-row `<basename>.{O,I}.{rec,rp}.err`
- Create (SDD workspace, not committed): `sweep-O.log`, `sweep-I.log`, `keep-O/`, `keep-I/`,
  `sweep-table.md`

**Interfaces:**
- Consumes: Task 1's script and `ROW` lines.
- Produces: `sweep-table.md` — the nine-row table in the charter's columns plus `symbol`,
  `pid_regime_O`, `pid_regime_I`, `label_O`, `label_I`, `class`, `gate`, `route` — the single
  source Task 3's reasons and Task 4's docs transcribe.

- [ ] **Step 1: Build and sign nothing by hand** — the script signs its own copy. Record the
  binary commit (`git rev-parse --short HEAD`).

- [ ] **Step 2: Run O (pid outside the range).** Check the counter: `sh -c 'echo $$'`. If it is
  inside `[16384, 65535]`, advance it past 65535 first (`i=0; while [ $i -lt N ]; do /usr/bin/true; i=$((i+1)); done`
  with N = 65600 − current). Then:

```sh
W=<SDD workspace dir>
nohup sh -c "sh -c 'echo pidstart=\$\$' > $W/sweep-O.log; RETRACE_SWEEP_KEEP=$W/keep-O tools/apple-sweep.sh >> $W/sweep-O.log 2>&1; echo SWEEP_EXIT=\$? >> $W/sweep-O.log" >/dev/null 2>&1 &
```

Poll with `until grep -q SWEEP_EXIT $W/sweep-O.log; do sleep 15; done` in bounded calls (≤ 9 min
each). When done: `grep -E '^TALLY|^FAIL|identical fault' $W/sweep-O.log` and the `ROW` lines'
`recpid` range — assert every recpid is outside `[16384, 65535]`; if the counter wrapped into the
range mid-run, say so per row (the `ROW` line has the pid) rather than re-running blindly.

- [ ] **Step 3: Run I (pid inside the range).** Advance the counter into `[16384, 65535]` (from
  above 65535 it wraps at 99998 → ~100, so the loop is ≤ ~50k spawns; from below, up to 16384).
  Same invocation with `sweep-I.log` / `keep-I`. Assert the pid range per row as above.

- [ ] **Step 4: Symbolicate the `brk` pc once.** The five "replay diverged" rows and
  `dddiagnose`'s in-range failures share `pc=0x18035f084 elr=0x1804af110`. The guest maps the
  shared cache at slide 0 (`crates/retrace-box/src/lib.rs:1387`), so these are unslid cache
  addresses. On the host:

```sh
lldb -b -o 'process launch -s' -o 'image list -o -f libxpc.dylib' -o 'image lookup -a 0x18035f084' -o 'image lookup -a 0x1804af110' /usr/bin/true 2>&1 | tail -20
```

If `image lookup` on the unslid address finds nothing, add libxpc's slide (the `-o` column of
`image list`) to both addresses and look those up; paste the exact commands and output into
`docs/sweep-evidence/2026-09-13-m36/README.md`. Expected: a libxpc function (M23 t5 measured the
`brk` group inside libxpc after `MACH_SEND_INVALID_DEST`); if the symbol is in another image, say
which — the table records what `lldb` said, not what M23 believed.

- [ ] **Step 5: The table.** From the two logs' `ROW` lines and the kept files, write
  `$W/sweep-table.md`: one row per binary that FAILed or was an identical fault on either run,
  plus `dddiagnose` regardless. Columns: `binary`, `label_O`, `label_I`, `rc/rp O`, `rc/rp I`,
  `recpid O`, `recpid I`, `first_divergent_landmark` (O and I), `trap` (the `EC`/syscall from the
  `rec_reason`, or the panic's assert), `symbol`, `root_cause_class`, `evidence` (the committed
  path AND the workspace trace path), `gate` (the test name Task 3 will use, or `none (class D)`),
  `route` (`M37` for B; `parked, not routed` for C; `retired` for A/D; `fixed in harness` for E1).
  Apply spec §3c's evidence tests literally and write the one-line justification per row under
  the table. Expected from spec §4: `csh`, `tcsh` → B (`dup2`, `lib.rs:1140`); `launchctl`,
  `automationmodetool`, `desdp`, `dyld_info`, `flex` → C (post-refusal `brk`, symbol from Step 4);
  `dddiagnose` → B then C (§4b first: in-range twelve `ESRCH` then crash/`brk`; out-of-range the
  RCV-shaped `mach_msg2`); `yes` → D. **If a row's evidence does not fit its expected class, the
  evidence wins — write what it says; if it fits no class, stop and report BLOCKED with the
  files.** Any row that PASSes on both runs having failed at M35 is class A — expected none.

- [ ] **Step 6: Commit the evidence.** For each table row copy `keep-O/<b>.rec.err` →
  `docs/sweep-evidence/2026-09-13-m36/<b>.O.rec.err` (and `.rp.err`, and the `.I.` pair); no
  `.bin`. Write the directory's `README.md`: the binary commit, both runs' `TALLY` lines and pid
  ranges, the symbolication command and output, one line per file. `du -sh` the directory (expect
  well under 100 KB). Commit:

```bash
git add docs/sweep-evidence/2026-09-13-m36 && git commit -m "M36 t2: the sweep's evidence, two runs, committed"
```

Report the two `TALLY` lines, both pid ranges, the symbol, and paste `sweep-table.md`.

---

### Task 3: One parked gate per wall

**Files:**
- Create: `crates/retrace/tests/apple_walls_e2e.rs`

**Interfaces:**
- Consumes: `util::record_dynamic(path) -> (RunOut, PathBuf)`, `util::replay(&path) -> RunOut`;
  Task 2's `sweep-table.md` (the reasons are transcribed from it, row by row).

- [ ] **Step 1: The file.** One test per class-B/C row of the table, all `#[ignore]`d. Shape:

```rust
// M36: a parked gate per measured wall in the Apple sweep. Every test here is `#[ignore]`d ON
// PURPOSE, and each reason is the measurement that parks it — the label the sweep printed, the
// exit codes, the recorder-pid regime, the recorder's own RECORD ERROR / panic line with the
// symbol, the landmark, the committed evidence file, the charter class, and what un-parks it.
// The sweep's old category string ("replay diverged") is not evidence and appears in no reason.
//
// Each body is the statement that becomes true when its wall falls: the binary records to a
// clean exit and replays bit-for-bit. The assertion messages print the recorder's stderr tail so
// a run with `--ignored` shows the wall by name (that run is this file's positive control).
mod util;

fn records_and_replays_clean(path: &str) {
    if !std::path::Path::new(path).exists() {
        eprintln!("SKIPPED: {path} is not present on this machine");
        return;
    }
    let (rec, trace) = util::record_dynamic(path);
    let tail = rec.stderr.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    assert_eq!(rec.code, 0, "{path}: record exited {} — the wall this gate is parked at, in the recorder's own words:\n{tail}", rec.code);
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "{path}: replay exited {}: {}", rp.code, rp.stderr.lines().last().unwrap_or(""));
    assert_eq!(rp.stdout, rec.stdout, "{path}: replay stdout differs from the recording");
}

#[test]
#[ignore = "M36 wall, class B (known-unmodelled), route M37. /bin/csh: run O label `recorder panicked: thread 'main' … panicked at crates/retrace-core/src/lib.rs:1140:17` (the M33 fail-loud assert on an unmodelled dup2), rc=101, recpid <O pid> (outside [0x4000,0x10000)); run I identical, recpid <I pid>. Evidence docs/sweep-evidence/2026-09-13-m36/csh.{O,I}.rec.err. UN-IGNORE when the fd table models dup2 (M10's unmodelled-but-fail-loud entry)."]
fn csh_records_and_replays() { records_and_replays_clean("/bin/csh"); }
```

— and likewise `tcsh_records_and_replays` (`/bin/tcsh`), `launchctl_records_and_replays`
(`/bin/launchctl`), `automationmodetool_records_and_replays`, `desdp_records_and_replays`,
`dyld_info_records_and_replays`, `flex_records_and_replays` (each class C: "run O label `record
error, rc=4: RECORD ERROR: non-syscall exit: exception (EC=0x3c …) pc=0x18035f084 …` — a `brk` in
`<symbol from Task 2>` after M23 t5's serviced `RefuseMqSend` (`the box hosts no message-queue
receivers`), landmark <N>; run I <same or as measured>. Class C: needs message-queue receivers /
an XPC pipe the tree does not have — parked, not routed. UN-IGNORE when the box services
message-queue sends to a real receiver.") and `dddiagnose_records_and_replays` (class B then C:
"run O: `record error, rc=4: RECORD ERROR: unsupported mach_msg2 … options 0x404000102 …` — the
RCV-shaped message-queue call `Route::Unsupported` keeps fail-loud, first observed M35; run I:
`identical fault, rc=139` or the post-refusal `brk` — after M34 §4b's probe forwards the in-range
pid as a host pointer and twelve self-pid csops/proc_info answer ESRCH (M35, 75 vs 63 err
landmarks). Fix order: §4b (M37) → then this gate fails deterministically at the RCV shape (class
C) → the brk class. UN-IGNORE when all three land."). Fill every `<…>` from `sweep-table.md`;
no placeholder may survive (`grep -n '<' crates/retrace/tests/apple_walls_e2e.rs` finds only
generics, if any).

- [ ] **Step 2: Compile and confirm nothing runs.**
  `cargo test -p retrace --test apple_walls_e2e -- --test-threads=1` → `0 passed; 0 failed; N ignored`.

- [ ] **Step 3: Control 2 — every gate fails for its measured reason.**
  `cargo test -p retrace --test apple_walls_e2e -- --ignored --test-threads=1 2>&1 | tee /tmp/m36-ignored.log`
  Expected: every test FAILED at `record exited 4` (or `101` for `csh`/`tcsh`) with the stderr
  tail showing the same `RECORD ERROR:`/`panicked at` line its reason quotes (`dddiagnose` may
  instead fail at `record exited 139` — the identical-fault path — depending on the pid; note
  which). Paste the failures list and one full message per class into the report. If any test
  PASSES under `--ignored`, stop: condition 3 (never park a passing test) — report BLOCKED with the
  output.

- [ ] **Step 4: Clippy and commit.**

```bash
cargo clippy -p retrace --all-targets -- -D warnings
git add crates/retrace/tests/apple_walls_e2e.rs && git commit -m "M36 t3: one parked gate per measured wall, each reason the measurement that parks it"
```

---

### Task 4: Docs, the gate, the merge

**Files:**
- Modify: `README.md` (the sweep bullet under "Known limits", currently `:389–460`-ish — the
  whole `dddiagnose` narrative folds into the table; the gate paragraph `:329–370`)
- Modify: `docs/status-log.md` (append one section; never edit an earlier line)
- Modify: `docs/superpowers/specs/2026-09-13-retrace-m36-sweepmeasure-design.md` (§11 only)

**Interfaces:**
- Consumes: `sweep-table.md`, the Task 1–3 reports, the controller's `task-4-numbers.md` (gate
  figures and reconciliation).

- [ ] **Step 1: README, the sweep bullet.** Rewrite it around the table: the two runs' tallies
  with pid regimes; the nine rows as a Markdown table (binary, label O/I, class, gate, route);
  the sentence "What the sweep reports is not why they fail" retired and replaced by what the
  sweep now prints; the M23 `brk` belief stated as measured (symbol, pc); `dddiagnose` as a row
  (its three walls in fix order); the E1 defects named as fixed in the harness; "no parked gate
  standing for it" retired — eight stand. Keep the reconstruction caveat and the `launchctl`/`yes`
  history in one short paragraph.

- [ ] **Step 2: README, the gate paragraph** at M36's figures from the numbers file (running
  count unchanged, ignored +N, binaries +1, reconciliation table).

- [ ] **Step 3: Status log.** Append `## Status: M36-sweepmeasure — nine rows read off kept
  evidence, and a parked gate for every wall`: what it set out to do; the harness change and
  Control 1 (old vs new labels, verbatim); the two runs (tallies, pid ranges, every label that
  differs between O and I); the symbolication (command + output); the table verbatim from
  `sweep-table.md` with the per-row justification; the classes (which evidence test each row met);
  Ruling 1–3; the parked gates and Control 2's output; the M37 routing (the class-B rows in the
  table's order, and the C rows named as parked-not-routed); the gate and reconciliation; what
  stays owed (every M35 item by name, minus what this milestone discharged: the sweep's two E1
  defects, the "no parked gate" debt).

- [ ] **Step 4: Spec §11** — the outcome: the table's classes vs §4's expectation, the symbol,
  every number, the rulings, spec corrections if any.

- [ ] **Step 5: Commit** `M36 t4: README, status-log section, spec outcome`.

- [ ] **Step 6 (controller): the gate.** Full chunked gate; `#[test]` reconciliation against M35's
  575 / 0 / 2 over 125; prediction 575 / 0 / (2 + N) over 126. Then `git merge --no-ff
  m36-sweepmeasure` into local `main`; never push.

---

## Self-Review

**Spec coverage.** §3a → Task 1 Steps 2–5; §3b → Task 2 Steps 2–4; §3c → Task 2 Step 5; §3d →
Task 3; §5b → Task 2 Step 6; §5d → Task 4; §6 Controls 1/2/3 → Task 1 Step 6, Task 3 Step 3, Task
2 (run I's `dddiagnose` line); §9 rulings → carried into Task 2 Step 5 and Task 3's reasons.

**Placeholder scan.** Task 3's `<…>` are explicitly required to be filled from the table and
checked by grep; no TBD/TODO.

**Type consistency.** `records_and_replays_clean(&str)` uses `util::record_dynamic` /
`util::replay` with the signatures in `crates/retrace/tests/util/mod.rs:90` and `:85`; `RunOut`
has `code`, `stdout`, `stderr`. The `ROW` line's nine tab-separated fields are named identically
in Task 1's Interfaces and Task 2's Step 5.

**The one thing the plan cannot know**: whether run I's `dddiagnose` takes the crash or the `brk`
path (M35: 2 of 5 crashed). Both are in the table; the gate's reason names both.
