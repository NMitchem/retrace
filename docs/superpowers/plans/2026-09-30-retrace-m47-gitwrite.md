# M47-gitwrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `git`'s local workflow (status, diff, log, show, rev-parse, `-C`, add, and default-config commit) record and replay bit-identically under retrace. Close the three mechanisms it needs for every guest: `madvise` modelled by advice and never forwarded; `__mac_syscall` modelled per `(policy, call)`, with AMFI answered by the host into a host-owned slot so `@rpath` guests load; `fork` refused with `EAGAIN`.

**Architecture:**
- **Rows (Task 1).** Five `arg_kinds` rows (`chdir`, `mkdir`, `link`, `utimes`, `__pthread_canceled`), written from their SDK prototypes and forwarded.
- **`madvise` (Task 2).** A pure `madvise_effect` in `retrace-arch` and a `Box_::guest_madvise` that returns the zero-fill writes. The record arm and replay mirror both apply those writes, recomputed on each side and never recorded (R3). A generic-arm assert makes "never forwarded" a checked fact.
- **`__mac_syscall` (Task 3).** Rows 381 and `0x8000_0000` become `NestedDest`. `Box_::guest_mac_syscall` classifies from guest memory, paging in a shared-cache string if it has to. The record arm answers AMFI through the host with host-owned pointers and records the 8-byte answer (R4), and answers Sandbox call 2 with the errno the old forward returned, keyed by operation name (R7).
- **`fork` (Task 4).** `fork_refusal_errno` refuses fork beside M38's exec refusal, with a generic-arm assert. A `machmsg` route answers the prepare handler's `mach_ports_register` (3403) with `KERN_SUCCESS`.
- **No trace-format change.** `TRACE_MAGIC` does not move (R6).

**Tech Stack:** Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, cargo tests, clang for guest fixtures, lldb and Xcode's `git` for t0's native measurements, POSIX `sh`/`zsh` for the sweep and the census.

**Spec:** `docs/superpowers/specs/2026-09-30-retrace-m47-gitwrite-design.md` (committed `39e0d8d`; corrected from this plan, see its §11). Its sections and rulings are cited as `M47 §3c`, `R3` and so on. **§11 items 1 and 2 add a forwarded row (333) and a generic-arm assert (fork) the spec did not list. Both need the operator's approval.**

## Global Constraints

- **Toolchain.** `1.95.0`, target `aarch64-apple-darwin`.
- **The gate.** `cargo test` in chunks, every chunk `--no-fail-fast` and `--test-threads=1`, plus `cargo clippy --workspace --all-targets -- -D warnings`.
- **`clippy -D warnings` rejects dead code and unused imports, in test files too (`--all-targets`).** Each task adds only what its own code uses. A helper or `use` in a test file must be used in that file by the end of the task that adds it.
- **Banned calls.** `clippy.toml` bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`.
- **One VM per process.** Every `cargo test` runs with `--test-threads=1`.
- **The trace format does not move.** `TRACE_MAGIC` stays put, and `crates/retrace-trace` has no diff (R6).
- **Symmetry rule 1.** Every record arm and its replay mirror call the same `Box_` or `retrace-arch` function with the same arguments. Both sit before the generic forward.
- **The thread oracle's count stays at seven.** Every new mirror lives inside the existing `Syscall` landmark chain, after its `verify_thread` call (`crates/retrace-core/src/lib.rs:1897`), beside the M38 exec mirror.
- **Spawn the CLI through `util`.** Every test that spawns the CLI uses `util`'s helpers, which call `util::bin()`, the codesigned copy.
- **Existing assertions stay.** The one named exception: Task 6 rewrites the `#[ignore]` reasons of `apple_walls_e2e`'s `csh`/`tcsh`. It may un-ignore either one, but only on a measurement.
- **Refusal texts.** The tests match on these prefixes:
  - `M47: unmeasured madvise advice <n>.`
  - `M47: madvise range starts at <addr>, not on a 16 KiB page`
  - `M47: madvise range page <page> is neither backed nor reserved`
  - `M47: __mac_syscall policy name: `
  - `M47: unmodelled __mac_syscall policy "<policy>" call <call>`
  - `M47: unmeasured Sandbox operation "<op>"`
  - `[retrace] refusing fork (syscall 2): process creation is unmodelled; returning errno 35 without forwarding`

  A replay-side refusal wraps the same message as `<call> refused on replay, though the recording accepted it — replay diverged before this landmark: <message>`. A replay-side mismatch starts `madvise recorded ` or `__mac_syscall recorded `.
- **Values pending t0.** A value marked `(t0 M…)` is sourced or inferred, not yet measured:
  - the accepted advice set;
  - the alignment and rounding rule;
  - the Sandbox operations;
  - the 3403 layout;
  - git's "cannot fork" line;
  - the in-list git commands.

  Where t0 measures a different value, the task uses t0's and its report says so. A difference the spec's design cannot express is the matching halt.
- **Worktree shell rules:**
  - no `VAR=val cmd` prefix; put `export VAR=val` on its own line first;
  - no `git -C` for the repository itself (the `-C` in git **test** commands is git-under-test's argument, not this rule's subject);
  - put `echo "exit=$?"` in the **same** command as the cargo invocation it checks, **before** any pipe;
  - `--no-fail-fast` goes before `--`;
  - never `git stash` the worktree, which is shared across worktrees.
- **Controls (deliberate breakages).**
  - Run them only on the **committed** tree, and restore with `git checkout -- <file>`.
  - Record each control's actual symptom in the task report.
  - A control that stays green is a finding: report it, never paper over it.
- **Logs.** Each command writes to `$L/t<N>-<what>.log`, where `L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite`. Shell state does not persist between tool calls, so every command that uses `$L` starts with its own `export L=…` line. Scratch traces and binaries go to `/private/tmp/claude-501/m47-*`.
- **Grep gate logs with `grep -a`**, since they carry ANSI and UTF-8. Before any `awk`, sanitize with `LC_ALL=C tr -cd '\11\12\15\40-\176' < log | sed 's/\x1b\[[0-9;]*m//g'`.
- **Evidence commits exclude trace files**, using the directory-anchored pathspec `':(exclude)<dir>/*.bin'` (M44 P1: the bare form stages nothing).
- **Style.**
  - Match the surrounding code's comment density and idiom.
  - Comments cite the spec as `M47 §3x`, rulings as `R<n>`, and t0 as `(t0 M2)`.
  - Test names are sentences.
- **Never push.** The merge goes into local `main` only; the push waits for the operator.
- **Halts.** Halt and ask on the spec's H2, H4 and H6 (§7), and on this plan's two:
  - **H7:** t0 M2(a)'s census finds a `(policy, call)` pair other than `("AMFI", 0x5a)` and `("Sandbox", 2)`.
  - **H8:** t0 M2(a) finds a Sandbox call-2 operation other than `syscall-unix` and `file-write-data`, or a forwarded call-2 result that carried writes or differed between two runs of one guest. That is H2's case, measured.

  **H1 falls back rather than halting:** default-config `commit` becomes a documented limit, and `git_e2e`'s commit test passes `-c maintenance.auto=false` (spec §3e). Report it. **H3 and H5 route:** re-park with the measurement and name the successor.

## Review Focus

These are the five inputs or failure modes the spec implies but no fixture is sure to reach, most likely first. Each is pinned by a test in the task that owns the code.

1. **A `MADV_ZERO` over a range that mixes committed pages and reserved-but-uncommitted ones.** This is libmalloc's nano and xzone regions. Only committed pages are written. An uncommitted page stays uncommitted, because it commits as zero on first touch. Committing it would change `backings` on one side only if a later edit made the model record-only.
   - Pinned in Task 2: `zero_writes_every_backed_page_and_no_reserved_one` (box level).
2. **A `madvise` range that runs past the guest's mappings.** It must be refused whole, naming the first page outside, and never partly applied.
   - Pinned in Task 2: `a_range_that_runs_past_the_guests_mappings_is_refused_naming_the_page`.
3. **An `int` argument whose register carries upper-half garbage.** The kernel reads 32 bits of `madvise`'s `behav` and of `__mac_syscall`'s `call`. A model that compares 64 bits refuses a legal call.
   - Pinned in Task 2: `an_advice_ints_upper_half_is_ignored_as_the_kernel_ignores_it`.
   - Pinned in Task 3: `amfi_is_classified_with_its_flags_and_the_ipa_of_out_flags`, which passes `0x5a | 1 << 32`.
4. **A replayed landmark that disagrees with what replay recomputes.** Examples: a `madvise` event carrying writes, or an AMFI answer recorded at another address. It must be a `Divergence` naming the call, never a silent apply.
   - Pinned in Task 2: `replay_refuses_a_madvise_landmark_that_carries_writes`.
   - Pinned in Task 3: `replay_refuses_an_amfi_answer_recorded_at_another_address`.
5. **A policy or operation name that is unterminated, too long, or unmodelled.** `copyinstr` stops at 32 bytes. An unmodelled name must be refused by name, never forwarded.
   - Pinned in Task 3: `an_unterminated_policy_is_refused_as_copyinstr_would` and `an_unmodelled_policy_or_operation_is_refused_by_name` (box level).

---

## File Structure

| File | Change | Task |
|---|---|---|
| `docs/superpowers/specs/2026-09-30-retrace-m47-gitwrite-measurements.md` | create: t0's M1–M6 | 0 |
| `docs/sweep-evidence/2026-09-30-m47-t0/` | create: t0 evidence (census, lldb logs, native outputs, README) | 0 |
| `crates/retrace-arch/src/lib.rs` | rows 9, 12, 136, 138, 333 (Task 1); the M47 section: `SYS_MADVISE`, `MADV_*`, `MadviseEffect`, `madvise_effect`; row 75's comment (Task 2); rows 381/`0x8000_0000` → `NestedDest`, `NestedDest`'s doc, `SYS_MAC_SYSCALL`, `MAC_MAX_POLICY_NAME`, `AMFI_DYLD_POLICY_SELF`, `SANDBOX_CHECK`, `SANDBOX_OPERATION_MAX`, `MacCall`, `mac_syscall_model`, `sandbox_check_continuity`, the nested-family unit test (Task 3); `SYS_FORK`, row 2, `fork_refusal_errno` (Task 4) | 1–4 |
| `crates/retrace-arch/tests/census.rs` | 9, 12, 136, 138, 333 (Task 1); 2 (Task 4); the M47 doc paragraph | 1, 4 |
| `crates/retrace-arch/tests/legacy_equivalence.rs` | two `EXPECTED_DIFFS` entries | 3 |
| `crates/retrace-arch/tests/gitshapes.rs` | create: 10 tests (2 + 3 + 4 + 1) | 1–4 |
| `crates/retrace-guest/c/fsops_dyn.c`, `madv_dyn.c`, `rpath_dyn.c`, `librpath_dyn.c`, `forkfail_dyn.c` | create: the fixtures | 1–4 |
| `crates/retrace-guest/build.rs`, `src/lib.rs` | build them; `FSOPS_DYN`, `MADV_DYN`, `RPATH_DYN`, `FORKFAIL_DYN`; four parse tests | 1–4 |
| `crates/retrace-box/src/lib.rs` | `guest_madvise` (Task 2); `MacSyscall`, `read_guest_cstr`, `guest_mac_syscall`, `host_amfi_dyld_policy` (Task 3) | 2, 3 |
| `crates/retrace-box/tests/madvise.rs` | create: 6 box-level tests | 2 |
| `crates/retrace-box/tests/macsyscall.rs` | create: 5 box-level tests | 3 |
| `crates/retrace-core/src/lib.rs` | the madvise arm, mirror and assert (Task 2); the `__mac_syscall` arm and mirror (Task 3); the fork arm, mirror and assert, and the 3403 route's arm and mirror (Task 4) | 2–4 |
| `crates/retrace-core/src/machmsg.rs` | `Route::ServicePortsRegister`, `decode_ports_register`, 2 unit tests | 4 |
| `crates/retrace/tests/gitprims_e2e.rs` | create: 8 tests (1 + 4 + 2 + 1) | 1–4 |
| `crates/retrace/tests/git_e2e.rs` | create: 3 + k tests, where k is t0 M4's in-list write commands beyond `add`/`commit` | 5 |
| `crates/retrace/tests/node_e2e.rs` | create: 1 test, `#[ignore]`d | 5, 6 |
| `crates/retrace/tests/apple_walls_e2e.rs` | `csh`/`tcsh` reasons rewritten (or un-ignored on a measurement) | 6 |
| `docs/sweep-evidence/2026-09-30-m47/` | create: the walk and the sweep | 6 |
| `docs/status-log.md`, `docs/current-state.md`, `README.md`, `CLAUDE.md` | docs | 7 |
| `.superpowers/sdd/2026-09-30-retrace-m47-gitwrite/{predict,gate,tally}.sh` | the close | 8 |

Each evidence directory is named for the day it is written. If a task runs on another day, use that day's date and carry the name forward.

**Test-count prediction (made here, reconciled at the close):**

| Task | Tests added |
|---|---|
| 1 | `gitshapes.rs` 2 (**new binary**); `retrace-guest` unit `fsops_guest_parses` 1; `gitprims_e2e.rs` 1 (**new binary**) |
| 2 | `gitshapes.rs` +3; `retrace-box/tests/madvise.rs` 6 (**new binary**); `retrace-guest` unit +1; `gitprims_e2e.rs` +4 |
| 3 | `gitshapes.rs` +4; `retrace-box/tests/macsyscall.rs` 5 (**new binary**); `retrace-guest` unit +1; `gitprims_e2e.rs` +2 |
| 4 | `gitshapes.rs` +1; `retrace-core` `machmsg` unit +2; `retrace-guest` unit +1; `gitprims_e2e.rs` +1 |
| 5 | `git_e2e.rs` 3 + k (**new binary**); `node_e2e.rs` 1, ignored (**new binary**) |
| 6 | if `csh` or `tcsh` un-ignores: one test moves from ignored to passed, each |

The baseline is M46's close, 898 passed / 0 failed / 9 ignored over 152 binaries. That is 905 `#[test]` lines (measured at `39e0d8d`), plus the 2 `census.rs` tests that `legacy_equivalence.rs` compiles a second time. t0 M6 re-derives it.

**Prediction:** +39 + k `#[test]` lines (905 → 944 + k), so passed + ignored = **946 + k over 158**. That is (936 + k) / 0 / 10 if `csh` and `tcsh` stay parked. k is fixed by t0 M4, and Task 8 re-derives the whole figure from source.

---

### Task 0 (t0): Measurements first

**Files:**
- Create: `docs/superpowers/specs/2026-09-30-retrace-m47-gitwrite-measurements.md`
- Create: `docs/sweep-evidence/2026-09-30-m47-t0/` (README plus the kept logs)

**Interfaces:**
- Consumes: the probe evidence `docs/sweep-evidence/2026-09-30-m47-probe/` (its `probe-scratch.patch`, `sandbox-call2.txt`, `git-runs.txt`).
- Produces: the measurements file, which later tasks read by section:
  - **M1:**
    - (a) the `madvise` advice census, with counts and the alignment of every call; this is the accepted set Task 2 codes;
    - (b) how often `g35`'s abort reproduces;
    - (c) whether `madv_dyn reuse` has a RED;
    - (d) native `madvise` alignment and rounding; this is the rule Task 2 codes.
  - **M2:**
    - (a) the `(policy, call)` census, with each call-2 operation and its forwarded result; these are the pairs and operations Task 3 codes;
    - (b) the native answers;
    - (c) whether the policy and operation strings are resident at the trap.
  - **M3:**
    - (a) the 3403 request bytes and the native reply;
    - (b) every trap between the prepare handler and the parent handler's end;
    - (c) git's native stderr and exit status when its maintenance fork fails.
  - **M4:** git's in-list and out-list, and the numbers they dispatch with no row on `main`. These give Task 1's rows and Task 5's k.
  - **M5:** the `chdir` audit.
  - **M6:** the base `#[test]` count.

**Everything experimental in this task is throwaway.** The only committed files are the measurements file and the evidence directory. Restore every source edit with `git checkout -- <file>` before committing, and delete every scratch file you created under `crates/`.

- [ ] **Step 1: Build the base binary and the census binary**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
mkdir -p $L/t0
cargo build -p retrace > $L/t0-base-build.log 2>&1; echo "exit=$?"
cp target/aarch64-apple-darwin/debug/retrace /private/tmp/claude-501/m47-base-retrace
codesign -s - -f --entitlements retrace.entitlements /private/tmp/claude-501/m47-base-retrace; echo "sign=$?"
shasum -a 256 /private/tmp/claude-501/m47-base-retrace
git apply docs/sweep-evidence/2026-09-30-m47-probe/probe-scratch.patch; echo "apply=$?"
```

The base binary is `39e0d8d`'s code, which is `427fa0a`'s: the spec commit changed only docs. Task 6 attributes sweep rows against it.

Now add the census instrumentation to the patched tree. In `crates/retrace-core/src/lib.rs`, find the generic arm's `let (ret, ret1, err, mut writes) = b.forward_and_diff(num, args);` and insert directly after it:

```rust
                // M47 t0 SCRATCH (never committed): the madvise / __mac_syscall census line.
                if num == 75 || num == 381 {
                    let cstr = |va: u64| {
                        let v = b.read_va_prefix(va, 64);
                        let n = v.iter().position(|&c| c == 0).unwrap_or(v.len());
                        String::from_utf8_lossy(&v[..n]).into_owned()
                    };
                    let (policy, op) = if num == 381 {
                        let p = b.read_va_prefix(args[2].wrapping_add(16), 8);
                        (cstr(args[0]), if p.len() == 8 { cstr(u64::from_le_bytes(p.try_into().unwrap())) } else { String::from("<no arg+16>") })
                    } else { (String::new(), String::new()) };
                    eprintln!("[m47] num={num} args=[{:#x},{:#x},{:#x}] policy={policy:?} op={op:?} ret={ret:#x} err={err} writes={}",
                        args[0], args[1], args[2], writes.len());
                }
```

Then, inside the patch's `PROBE SCRATCH` AMFI arm, directly before its `eprintln!("[probe] AMFI dyld policy …")`, insert:

```rust
                eprintln!("[m47] amfi policy_resident={:?}", String::from_utf8_lossy(&b.read_va_prefix(args[0], 5)));
```

An empty `policy` or `policy_resident` means that string's page was not staged at the trap: `read_va_prefix` does not page in. That is M2(c).

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo build -p retrace > $L/t0-census-build.log 2>&1; echo "exit=$?"
cp target/aarch64-apple-darwin/debug/retrace /private/tmp/claude-501/m47-census-retrace
codesign -s - -f --entitlements retrace.entitlements /private/tmp/claude-501/m47-census-retrace; echo "sign=$?"
git diff > $L/t0/census-build.patch
git checkout -- crates/
git status --short
```

Expected: both builds `exit=0`, and `git status --short` shows no `crates/` change. `census-build.patch` records exactly what the census binary is.

- [ ] **Step 2: Write the fixtures into the ledger and run them natively**

Copy these sources **verbatim** into `$L/t0/`; later tasks commit the identical text:
- Task 1 Step 3's `fsops_dyn.c`;
- Task 2 Step 5's `madv_dyn.c`;
- Task 3 Step 5's `rpath_dyn.c` and `librpath_dyn.c`;
- Task 4 Step 5's `forkfail_dyn.c`.

Then:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cd $L/t0
for f in fsops_dyn madv_dyn forkfail_dyn; do clang -arch arm64 -o $f $f.c; echo "$f build=$?"; done
clang -arch arm64 -dynamiclib -install_name @rpath/librpath_dyn.dylib -o librpath_dyn.dylib librpath_dyn.c; echo "lib build=$?"
clang -arch arm64 -o rpath_dyn rpath_dyn.c librpath_dyn.dylib -Wl,-rpath,@executable_path; echo "rpath build=$?"
rm -rf fsops-native && mkdir fsops-native && ./fsops_dyn $PWD/fsops-native > native-fsops.out 2>&1; echo "fsops rc=$?"; cat native-fsops.out
stat -f 'h mtime=%m nlink=%l' fsops-native/d/h
for m in zero reuse bad; do ./madv_dyn $m > native-madv-$m.out 2>&1; echo "madv $m rc=$?"; cat native-madv-$m.out; done
./rpath_dyn > native-rpath.out 2>&1; echo "rpath rc=$?"; cat native-rpath.out
./forkfail_dyn > native-forkfail.out 2>&1; echo "forkfail rc=$?"; cat native-forkfail.out
cd -
```

Expected: every build `=0`, and:

| run | rc | output |
|---|---|---|
| `fsops` | 0 | `mkdir chdir link rename utimes ok`, `h mtime=1234567890 nlink=2`; `stat` agrees |
| `madv zero` | 0 | `zero low=zeros high=kept` |
| `madv reuse` | 0 | `reuse kept`, `reuse ok` |
| `madv bad` | 0 | `bad rc=<n> errno=<e>`: record the values |
| `rpath` | 0 | `rpath marker=47` |
| `forkfail` | 0 | `fork failed errno=35` |

A different native output is a fixture defect, not a halt. Fix the text in the ledger copy **and** in the task that commits it, and say so in the report.

- [ ] **Step 3: M1(a), M2(a), M2(c): the census**

Write `$L/t0/census.sh`:

```sh
#!/bin/sh
# M47 t0 M1(a)/M2(a)/M2(c): every madvise (75) and __mac_syscall (381) dispatch across the corpus,
# from RETRACE_TRACE's [trap] lines and the census build's [m47]/[probe] lines. The shape of
# tools/destgaps-census.sh (bounded run, line-capped stderr, watchdog by command pattern).
# Usage: census.sh <signed-census-retrace> <guest-out-dir> <ledger-t0-dir> <repo-root> <out.tsv>
set -u
BIN=$1; GUESTS=$2; FIX=$3; ROOT=$4; OUT=$5
TMP=$(mktemp -d -t m47-census); trap 'rm -rf "$TMP"' EXIT INT TERM
: > "$OUT"
TIMEOUT_SECS=${RETRACE_SWEEP_TIMEOUT:-60}
LINE_CAP=400000
one() {
    label=$1; mode=$2; path=$3; shift 3
    rm -f "$TMP/t.bin" "$TMP/trace"
    ( exec env RETRACE_TRACE=1 "$BIN" "$mode" "$path" -o "$TMP/t.bin" "$@" 2>&1 >/dev/null </dev/null ) \
        | head -n "$LINE_CAP" > "$TMP/trace" &
    ppid=$!
    ( sleep "$TIMEOUT_SECS"; pkill -9 -f "^$BIN $mode $path" 2>/dev/null ) &
    wpid=$!
    wait "$ppid" 2>/dev/null; st=$?
    kill "$wpid" 2>/dev/null; wait "$wpid" 2>/dev/null
    total=$(grep -ac '^\[trap\]' "$TMP/trace")
    grep -aE '^\[trap\] num=(75|381) |^\[m47\] |^\[probe\] AMFI' "$TMP/trace" | sed "s|^|$label	|" >> "$OUT"
    last=$(grep -a -m1 -E 'panicked at|RECORD ERROR|M33:' "$TMP/trace" | cut -c1-160)
    echo "$label pipeline_exit=$st traps=$total last=$last"
}
for g in "$GUESTS"/*; do
    case "$g" in *.bin|*.s|*.txt|*.dylib) continue ;; esac
    [ -f "$g" ] && [ -x "$g" ] || continue
    file "$g" | grep -q 'Mach-O' || continue
    if otool -l "$g" 2>/dev/null | grep -q LC_LOAD_DYLINKER; then one "guest:$(basename "$g")" record-dyn "$g"
    else one "guest:$(basename "$g")" record "$g"; fi
done
rm -rf "$TMP/fsops" && mkdir "$TMP/fsops" && one "fix:fsops" record-dyn "$FIX/fsops_dyn" -- "$TMP/fsops"
for m in zero reuse bad; do one "fix:madv-$m" record-dyn "$FIX/madv_dyn" -- "$m"; done
one "fix:rpath" record-dyn "$FIX/rpath_dyn"
one "fix:forkfail" record-dyn "$FIX/forkfail_dyn"
JQ=/opt/homebrew/bin/jq
[ -x "$JQ" ] && one "jq:--version" record-dyn "$JQ" -- --version || echo "SKIP jq"
[ -x "$JQ" ] && one "jq:file" record-dyn "$JQ" -- .name "$ROOT/crates/retrace/tests/fixtures/rung3.json"
PY=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
[ -x "$PY" ] && one "cpython:print" record-dyn "$PY" -- -c 'print(1)' || echo "SKIP cpython"
[ -x "$PY" ] && one "cpython:crash" record-dyn "$PY" -- "$ROOT/crates/retrace-guest/py/crash.py"
NODE=/opt/homebrew/bin/node
if [ -x "$NODE" ]; then NR=$(cd "$(dirname "$NODE")" && cd "$(dirname "$(readlink "$NODE")")" && pwd)/$(basename "$(readlink "$NODE")")
    one "node:-e" record-dyn "$NR" -- -e 'console.log(1)'; else echo "SKIP node"; fi
while IFS= read -r g <&3; do
    case "$g" in ''|\#*) continue ;; esac
    [ -x "$g" ] || { echo "SKIP $g"; continue; }
    one "apple:$g" record-dyn "$g"
done 3< "$ROOT/tools/apple-sweep-binaries.txt"
echo "DONE matched_total=$(wc -l < "$OUT" | tr -d ' ') -> $OUT"
```

If `readlink` of the node symlink returns an absolute path, use `NR=$(readlink "$NODE")` directly. The probe used `/opt/homebrew/Cellar/node/25.6.1/bin/node`.

Run it **in the background** with nothing else building, and wait for `DONE`:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
export GOUT=$(ls -td target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)
sh $L/t0/census.sh /private/tmp/claude-501/m47-census-retrace $GOUT $L/t0 $PWD $L/t0/census.tsv > $L/t0/census.progress 2>&1; echo "exit=$?"
tail -3 $L/t0/census.progress
```

Write `$L/t0/summ.py`, which summarizes the census and, later, M4's logs:

```python
# M47 t0: summarize madvise (75) and __mac_syscall (381) lines from census.tsv and M4's *.rec.err.
import collections, re, sys
adv, align, mac = collections.Counter(), collections.Counter(), collections.Counter()
who = collections.defaultdict(set)
T75 = re.compile(r'\[trap\] num=75 .*?args=\[(0x[0-9a-f]+),(0x[0-9a-f]+),(0x[0-9a-f]+)')
M381 = re.compile(r'\[m47\] num=381 args=\[(0x[0-9a-f]+),(0x[0-9a-f]+),(0x[0-9a-f]+)\] policy="([^"]*)" op="([^"]*)" ret=(0x[0-9a-f]+) err=(\w+) writes=(\d+)')
for path in sys.argv[1:]:
    for line in open(path, errors='replace'):
        label, rest = line.rstrip('\n').split('\t', 1) if '\t' in line else (path, line.rstrip('\n'))
        m = T75.search(rest)
        if m:
            a, l, v = (int(x, 16) for x in m.groups())
            adv[v & 0xffffffff] += 1; who[('madvise', v & 0xffffffff)].add(label)
            align[('addr16k' if a % 0x4000 == 0 else 'addr-UNALIGNED', 'len16k' if l % 0x4000 == 0 else 'len-UNALIGNED', 'len0' if l == 0 else 'len>0')] += 1
        m = M381.search(rest)
        if m:
            key = (m.group(4), int(m.group(2), 16) & 0xffffffff, m.group(5), m.group(6), m.group(7), m.group(8))
            mac[key] += 1; who[('mac',) + key].add(label)
        if rest.startswith('[probe] AMFI'):
            mac[('AMFI', 0x5a, '', rest, '', '')] += 1; who[('amfi',)].add(label)
        if rest.startswith('[m47] amfi policy_resident='):
            mac[('AMFI-resident', 0, rest.split('=', 1)[1], '', '', '')] += 1
print('madvise advice census:'); [print(f'  advice {k}: {n} calls, guests {sorted(who[("madvise", k)])[:8]}') for k, n in sorted(adv.items())]
print('madvise alignment:'); [print(f'  {k}: {n}') for k, n in sorted(align.items())]
print('__mac_syscall census (policy, call, op, ret, err, writes):'); [print(f'  {k}: {n}') for k, n in sorted(mac.items(), key=str)]
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
python3 $L/t0/summ.py $L/t0/census.tsv > $L/t0/census-summary.txt; echo "exit=$?"; cat $L/t0/census-summary.txt
```

Record in the measurements file:
- **M1(a):** every advice value with its count and a guest that issues it, and the alignment table.
  - Expected: 7, 8 and 11 (the probe).
  - Any other value is classified by Task 2 Step 3's table. A value the table calls a query is **H3**.
  - Any `addr-UNALIGNED` or `len-UNALIGNED` row changes Task 2's refusal rule; carry it to M1(d).
- **M2(a):** every `(policy, call, op, ret, err, writes)` tuple with its count and guests.
  - Expected: AMFI `0x5a` once per dyld guest (the `[probe]` lines);
  - Sandbox `2` with `op` `syscall-unix` → `ret=0xe err=true writes=0`;
  - Sandbox `2` with `op` `file-write-data` → `ret=0x16 err=true writes=0`.
  - Any other pair is **H7**. Any other operation, any `writes` above 0, or one guest's two runs disagreeing is **H8**. To check the last, run one guest twice with `one`.
- **M2(c):** whether the `policy` and `op` strings (and the AMFI `policy_resident`) were readable at the trap. Empty means Task 3's `read_guest_cstr` pages them in, and `rpath_dyn` exercises that path.

- [ ] **Step 4: M1(b), how often the forwarded abort reproduces**

The probe's patch as-is is the `g35` configuration: the rows, `MADV_ZERO` recorded as zeros, and advice 7 and 8 forwarded unless `PROBE_NOREUSABLE` is set. The census binary carries the patch plus print-only lines. Write `$L/t0/m1b.sh`:

```zsh
#!/bin/zsh
# M47 t0 M1(b): git commit with MADV_FREE_REUSABLE forwarded (A) and no-op'd (B), N fresh repos each.
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=/private/tmp/claude-501/m47-census-retrace
O=$1; N=${2:-5}
ID=(-c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false)
mkdir -p $O
for mode in A B; do
  for i in $(seq 1 $N); do
    d=$O/repo-$mode-$i; rm -rf $d; mkdir -p $d
    env -i $G -C $d init -q -b main; print one > $d/a.txt; env -i $G -C $d add a.txt
    if [ $mode = B ]; then export PROBE_NOREUSABLE=1; else unset PROBE_NOREUSABLE; fi
    perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o /private/tmp/claude-501/m47-m1b.bin -- -C $d $ID commit -q -m first > $O/$mode-$i.out 2> $O/$mode-$i.err; rc=$?
    abort=$(grep -a -c 'pointer being freed was not allocated' $O/$mode-$i.err)
    head=$(env -i $G -C $d rev-parse -q --verify HEAD >/dev/null && echo committed || echo none)
    echo "M1b mode=$mode run=$i rc=$rc abort_lines=$abort head=$head"
  done
done
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
zsh $L/t0/m1b.sh $L/t0/m1b 5 > $L/t0/m1b.txt 2>&1; echo "exit=$?"; cat $L/t0/m1b.txt
```

Record the abort rate for A (forwarded) and B (no-op'd). The probe saw A abort 1 of 1 times and B abort 0 of 1. The rate is the strength of `git_e2e`'s commit test as a guard for this class (spec §4's named weakness). It is not a halt either way.

- [ ] **Step 5: M1(c), a repo-owned trigger**

On the **base** binary, where advice 7 and 8 are forwarded, record `madv_dyn reuse` five times:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
for i in 1 2 3 4 5; do /private/tmp/claude-501/m47-base-retrace record-dyn $L/t0/madv_dyn -o /private/tmp/claude-501/m47-m1c.bin -- reuse > $L/t0/m1c-$i.out 2> $L/t0/m1c-$i.err; echo "run $i rc=$?"; cat $L/t0/m1c-$i.out; done
```

Expected: every run prints `reuse kept` and `reuse ok`. If any run prints `dropped` or `bad`, then forwarding can corrupt a repo-owned guest. `madv reuse` is then a RED at base, and Task 2's test says so. Otherwise the measurements file says no trigger was found, and spec §4's named weakness stands.

- [ ] **Step 6: M1(d), native `madvise` edges**

Write `$L/t0/madvnative.c`:

```c
// M47 t0 M1(d): native madvise alignment, rounding and zero length, per advice.
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(void) {
    int adv[] = { MADV_FREE_REUSABLE, MADV_FREE_REUSE, MADV_ZERO };
    for (unsigned i = 0; i < sizeof adv / sizeof *adv; i++) {
        unsigned char *p = mmap(NULL, 0x10000, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        memset(p, 0xAB, 0x10000);
        int a = madvise(p, 0x4000, adv[i]);          int ea = a ? errno : 0;
        int b = madvise(p + 0x1000, 0x4000, adv[i]); int eb = b ? errno : 0;
        int c = madvise(p + 1, 0x4000, adv[i]);      int ec = c ? errno : 0;
        int d = madvise(p, 0, adv[i]);               int ed = d ? errno : 0;
        memset(p, 0xAB, 0x10000);
        int e = madvise(p, 0x4001, adv[i]);          int ee = e ? errno : 0;
        printf("advice %d: aligned rc=%d/%d off4k rc=%d/%d off1 rc=%d/%d len0 rc=%d/%d len0x4001 rc=%d/%d byte[0x4000]=%#x byte[0x7fff]=%#x byte[0x8000]=%#x\n",
               adv[i], a, ea, b, eb, c, ec, d, ed, e, ee, p[0x4000], p[0x7fff], p[0x8000]);
    }
    return 0;
}
```

Add any value M1(a)'s census found to `adv[]` before building.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
clang -arch arm64 -o $L/t0/madvnative $L/t0/madvnative.c && $L/t0/madvnative > $L/t0/m1d.txt 2>&1; echo "exit=$?"; cat $L/t0/m1d.txt
```

Record, for each advice:
- **An address off a 16 KiB page (`off4k`, `off1`).** Expected `EINVAL` (22). Task 2 refuses such a range rather than answering it. That stays right whatever native does, provided M1(a) found no such call in the corpus.
- **Zero length.** Expected rc 0. Task 2's loop answers 0 with nothing done. If native says otherwise, report it: the model then refuses `len == 0`.
- **`len 0x4001` under `MADV_ZERO`.**
  - `byte[0x4000]` and `byte[0x7fff]` equal to `0` means xnu rounds `len` up to the page, and Task 2's `a_length_short_of_a_page_zeroes_the_whole_last_page_as_xnu_rounds_it` stands.
  - `0xab` means it does not round. The model and that test then zero only whole pages inside `[addr, addr + len)`, and Task 2's report says so.
  - `byte[0x8000]` must stay `0xab` either way.

- [ ] **Step 7: M2(b), the native answers**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
{ echo "breakpoint set -n __mac_syscall"; echo "run";
  for i in 1 2 3 4 5 6 7 8; do echo "register read x0 x1 x2"; echo "memory read -s8 -fx -c3 \$x2"; echo "x/s *(char **)(\$x2 + 16)"; echo "x/s \$x0"; echo "finish"; echo "register read x0"; echo "continue"; done; } > $L/t0/m2b.lldb
mkdir -p $L/t0/m2b-repo && cd $L/t0/m2b-repo && env -i /Applications/Xcode.app/Contents/Developer/usr/bin/git init -q -b main && cd -
lldb -b -s $L/t0/m2b.lldb -- /Applications/Xcode.app/Contents/Developer/usr/bin/git -C $L/t0/m2b-repo status --porcelain > $L/t0/m2b.log 2>&1; echo "exit=$?"
grep -a -E 'x0 = |x1 = |"' $L/t0/m2b.log | head -80
```

For each stop, record the policy (`x/s $x0`), the call (`x1`) and, for Sandbox, the operation (`x/s` at `+16`) with the value `finish` leaves in `x0`. This is what the calls return **natively** for an unsandboxed process. `x/s $x0` after `register read` reads the policy before the call. Commands that fail after the process exits are harmless.

R7 keeps the old forward's errno for continuity, so these values are not coded. They go into Known limits (Task 7) as the fidelity gap: native answers X, retrace answers the pre-M47 errno. If `finish` is refused inside dyld, report it and record only the entry registers.

- [ ] **Step 8: M3(a) and M3(b), the fork path natively**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cat > $L/t0/m3a.lldb <<'EOF'
breakpoint set -n mach_msg2_internal -c '*(unsigned int *)($x0 + 20) == 3403'
run
memory read -s1 -fx -c64 $x0
register read x0 x1 x2 x3 x4 x5 x6 x7
bt 10
expr unsigned long $m = (unsigned long)$x0
finish
register read x0
memory read -s1 -fx -c48 $m
continue
EOF
lldb -b -s $L/t0/m3a.lldb -- $L/t0/forkfail_dyn > $L/t0/m3a.log 2>&1; echo "exit=$?"
cat > $L/t0/m3b.lldb <<'EOF'
breakpoint set -n libSystem_atfork_prepare
run
breakpoint set -r '.' -s libsystem_kernel.dylib
breakpoint command add -o "frame info" -o "continue" 2
continue
EOF
lldb -b -s $L/t0/m3b.lldb -- $L/t0/forkfail_dyn > $L/t0/m3b.log 2>&1; echo "exit=$?"
grep -a 'frame #0' $L/t0/m3b.log | sed 's/.*`//; s/ .*//' | uniq > $L/t0/m3b-calls.txt; cat $L/t0/m3b-calls.txt
```

`forkfail_dyn` lowers its own `RLIMIT_NPROC` to 1, so natively its `fork` fails with `EAGAIN` under lldb too. The limit binds the fixture, not lldb.

Record:
- **M3(a).** The 64 request bytes.
  - Expected: `msgh_bits` with `MACH_MSGH_BITS_COMPLEX` (`0x80000000`) set;
  - `msgh_size` 64;
  - `msgh_id` 3403 at offset 20;
  - descriptor count 3 at offset 24;
  - three 12-byte port descriptors from offset 28, each with type byte (descriptor offset 11) 0 = `MACH_MSG_PORT_DESCRIPTOR`.

  Also record each descriptor's disposition (offset 10) and name. Record the native return (`x0` after `finish`, expected 0) and the reply's `msgh_id` at offset 20 (expected 3503) and RetCode at offset 32 (expected 0).
  - Any other layout changes Task 4's `decode_ports_register` to match it.
  - A `MOVE_*` disposition is recorded, not halted on: natively the send moves the right, and the model's not moving it leaves one extra user reference in retrace's own IPC space. Task 4's doc comment names it.
- **M3(b).** The ordered list of `libsystem_kernel` functions entered from the prepare handler to exit. Expected: `mach_ports_register` (and its `mach_msg2` path), then `__fork`, then `cerror`/`__error`, then the parent handler's calls, then the `printf` path's `write`.
  - **H1** applies if any `mach_msg` other than 3403 appears before `__fork`, or if the parent handler issues a trap that has no `arg_kinds` row on `main` and is not already serviced. Check candidate numbers with Step 10's rowcheck.
  - `setrlimit` (195) comes before the prepare handler and has a row.

- [ ] **Step 9: M3(c), git's native "cannot fork" text**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
export G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
export D=$L/t0/m3c-repo
rm -rf $D && mkdir -p $D && env -i $G -C $D init -q -b main && echo one > $D/a.txt && env -i $G -C $D add a.txt
env -i $G -C $D -c user.name=retrace -c user.email=retrace@example.invalid -c maintenance.auto=false commit -q -m first; echo "setup=$?"
echo two >> $D/a.txt && env -i $G -C $D add a.txt
sh -c "ulimit -u 1; exec env -i $G -C $D -c user.name=retrace -c user.email=retrace@example.invalid commit -q -m second" > $L/t0/m3c.out 2> $L/t0/m3c.err; echo "rc=$?"
cat $L/t0/m3c.out; cat $L/t0/m3c.err
env -i $G -C $D log -1 --format=%s
```

`exec` replaces the shell without forking, so only git's own maintenance fork meets the limit.

Record git's exit status and every stderr line **verbatim**. Expected: rc 0, the commit written (`second`), and one line of the form `error: cannot fork() for …: Resource temporarily unavailable`. Task 5's `CANNOT_FORK` takes that exact text.
- A non-zero rc changes Task 5's commit test to expect it; report it.
- If `ulimit -u 1` is refused, use the lowest value the shell accepts, and report it.

- [ ] **Step 10: M4, git's command list**

Write `$L/t0/m4.sh`:

```zsh
#!/bin/zsh
# M47 t0 M4: each candidate git command natively and under the census build. Reads run on ONE repo
# (native first, then recorded), so their stdout is comparable; writes run on twin repos and
# compare a state fingerprint of hash-free facts (trees, subjects, ref names, index, status).
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=/private/tmp/claude-501/m47-census-retrace
O=$1; mkdir -p $O; : > $O/nums.txt
ID=(-c user.name=retrace -c user.email=retrace@example.invalid)
mkrepo() {
  d=$1; rm -rf $d; mkdir -p $d
  env -i $G -C $d init -q -b main
  print one > $d/a.txt; env -i $G -C $d add a.txt
  env -i $G -C $d $ID -c maintenance.auto=false commit -q -m first
  if [ "$2" = merge ]; then
    env -i $G -C $d switch -q -c topic; print t > $d/t.txt; env -i $G -C $d add t.txt
    env -i $G -C $d $ID -c maintenance.auto=false commit -q -m topic; env -i $G -C $d switch -q main
  fi
  print 'one\ntwo' > $d/a.txt; print untracked > $d/b.txt; print added > $d/c.txt
}
state() {
  for a in "status --porcelain" "ls-files --stage" "for-each-ref --format=%(refname)" "log --all --format=%T%x20%s" "stash list --format=%s" "symbolic-ref -q HEAD"; do
    env -i $G -C $1 ${=a}; done 2>&1
}
rec() {  # rec <tag> <repo> <git args...>
  tag=$1; d=$2; shift 2
  export RETRACE_TRACE=1
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/$tag.bin -- -C $d "$@" > $O/$tag.rec.out 2> $O/$tag.rec.err; rrc=$?
  unset RETRACE_TRACE
  prc=n/a; same=n/a
  if [ -s $O/$tag.bin ] && ! grep -a -q -E 'panicked at|RECORD ERROR' $O/$tag.rec.err; then
    perl -e 'alarm 300; exec @ARGV' $R replay $O/$tag.bin > $O/$tag.rp.out 2> $O/$tag.rp.err; prc=$?
    cmp -s $O/$tag.rec.out $O/$tag.rp.out && same=yes || same=no
  fi
  grep -a -o '^\[trap\] num=[-0-9]*' $O/$tag.rec.err | sed 's/.*=//' >> $O/nums.txt
  wall=$(grep -a -m1 -E 'panicked at|RECORD ERROR|M33:' $O/$tag.rec.err | cut -c1-200)
}
read_cmd() {  # read_cmd <tag> <git args...>
  tag=$1; shift; d=$O/r-$tag; mkrepo $d
  env -i $G -C $d "$@" > $O/$tag.native.out 2> $O/$tag.native.err; nrc=$?
  rec $tag $d "$@"
  cmp -s $O/$tag.native.out $O/$tag.rec.out && nat=yes || nat=no
  echo "M4 READ $tag native_rc=$nrc rec_rc=$rrc rp_rc=$prc stdout_rec==native:$nat rp==rec:$same wall=$wall"
}
write_cmd() {  # write_cmd <tag> <prep> <git args...>
  tag=$1; prep=$2; shift 2; n=$O/n-$tag; d=$O/w-$tag; mkrepo $n $prep; mkrepo $d $prep
  env -i $G -C $n "$@" > $O/$tag.native.out 2> $O/$tag.native.err; nrc=$?
  rec $tag $d "$@"
  state $n > $O/$tag.native.state; state $d > $O/$tag.rec.state
  cmp -s $O/$tag.native.state $O/$tag.rec.state && st=yes || st=no
  echo "M4 WRITE $tag native_rc=$nrc rec_rc=$rrc rp_rc=$prc state_rec==native:$st rp==rec:$same wall=$wall"
}
read_cmd status-porcelain status --porcelain
read_cmd status status
read_cmd diff diff
read_cmd diff-cached diff --cached
read_cmd log log -1
read_cmd show show --stat HEAD
read_cmd rev-parse rev-parse HEAD
write_cmd add - add c.txt
write_cmd commit - $ID -c maintenance.auto=false commit -q -a -m second
write_cmd branch - branch topic
write_cmd tag - tag v1
write_cmd switch-c - switch -q -c topic
write_cmd mv - mv a.txt moved.txt
write_cmd rm - rm -q --cached a.txt
write_cmd stash - $ID stash -q
write_cmd merge merge $ID -c maintenance.auto=false merge -q --ff-only topic
sort -un $O/nums.txt -o $O/nums.txt
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
zsh $L/t0/m4.sh $L/t0/m4 > $L/t0/m4.txt 2>&1; echo "exit=$?"; cat $L/t0/m4.txt
python3 $L/t0/summ.py $L/t0/m4/*.rec.err > $L/t0/m4-census-summary.txt; cat $L/t0/m4-census-summary.txt
```

Merge `m4-census-summary.txt` into M1(a)/M2(a): git's calls are part of the census.

Now find which dispatched numbers have no row on `main`. Create the throwaway file `crates/retrace-arch/tests/zz_m47_rowcheck.rs`:

```rust
// M47 t0 SCRATCH (never committed): which of $M47_NUMS have no arg_kinds row on this tree.
#[test]
fn m47_rowcheck() {
    let path = std::env::var("M47_NUMS").expect("M47_NUMS");
    for n in std::fs::read_to_string(path).unwrap().split_whitespace() {
        let n: i64 = n.parse().unwrap();
        if retrace_arch::arg_kinds(n as u64).is_none() { println!("NOROW {n}"); }
    }
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
export M47_NUMS=$L/t0/m4/nums.txt
cargo test -p retrace-arch --test zz_m47_rowcheck -- --nocapture > $L/t0/m4-rowcheck.log 2>&1; echo "exit=$?"
grep -a NOROW $L/t0/m4-rowcheck.log
rm crates/retrace-arch/tests/zz_m47_rowcheck.rs; git status --short
```

Expected `NOROW`: 9, 12, 136, and 138 if any command freshens an object. 333 does not appear, because M4 disables maintenance and 333 is issued only around the maintenance fork (the probe's `g36`/`g40`). 2 is not reached either: no command forks here.

Rows from the scratch patch that M4 does **not** reach (137, 15, 124, 95) get no row (M33's rule).

Decide the lists:
- **In:** `rec_rc == native_rc`, `rp == rec`, and for a read `stdout_rec==native: yes`, for a write `state_rec==native: yes`. The only exception is a wall that is one of this milestone's rows, which the probe build already carries.
- **Out:** everything else, each with its wall. **H5**: route it, don't widen.

The in-list's write commands other than `add` and `commit` are Task 5's k tests. Its read commands are Task 5's `READS`.

Every `NOROW` number that an in-list command dispatches, and that is not in {9, 12, 136, 138}, is a row Task 1 adds in the same form. Write it from its SDK prototype and cite this step.

- [ ] **Step 11: M5, the `chdir` audit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
grep -rn -E 'File::(open|create)|OpenOptions|fs::(read|write|read_to_string|metadata|canonicalize|copy|remove_file|create_dir)|Command::new|Writer::create|Reader::open' crates/*/src > $L/t0/m5-grep.txt; wc -l < $L/t0/m5-grep.txt
```

Classify every line in the measurements file as one of:
- **(i)** an absolute path;
- **(ii)** opened before the guest's first instruction on the record path (the CLI's trace `Writer::create`, the guest image read, `/usr/lib/dyld`);
- **(iii)** replay, debug or gdbserver only, which forward nothing and so never `chdir`;
- **(iv)** test-only or diagnostic code, named.

Read `record_box` and the CLI's `record`/`record-dyn` arm to confirm the trace file is opened once, before the guest runs, and never reopened by path.

**H6** applies if any line is a relative path opened on the record path after the guest starts.

- [ ] **Step 12: M6, the base `#[test]` count**

```bash
grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
ls crates/*/tests/*.rs | wc -l
```

Expected: `905` and `137`. M46 closed at 898 + 9 = 907, which is 905 plus the two `census.rs` tests compiled twice, over 152 binaries. A different number is reconciled file by file against M46's `predict.txt` (`.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers/predict.txt` in the main checkout) before Task 1 starts.

- [ ] **Step 13: Write the measurements file and the evidence; commit**

Create `docs/sweep-evidence/2026-09-30-m47-t0/` containing:
- every `$L/t0/*.txt`, `*.log`, `*.out`, `*.err`, `*.tsv`, `*.lldb`, `*.sh`, `*.py` and `*.patch` file;
- the fixture sources and `madvnative.c`;
- `m1b/*.out`, `m1b/*.err`, `m4/*.out`, `m4/*.err` and `m4/*.state`, never a `.bin` and never a repo directory;
- a `README.md` saying, for each file, which command produced it, on which binary (with sha256), and on which date.

Write the measurements file with one section per measurement, `## M1` (with (a)–(d)) through `## M6`. Each section gives:
- the command;
- the result, quoted from the evidence file;
- the decision the spec's or plan's rule makes from it;
- any halt considered.

End with a **Decisions** list. It must give:
- the accepted advice set;
- the alignment and rounding rule;
- the modelled `(policy, call)` pairs and Sandbox operations with their errnos;
- the 3403 layout;
- `CANNOT_FORK`;
- the read and write in-lists;
- Task 1's full row set;
- k.

```bash
git add docs/superpowers/specs/2026-09-30-retrace-m47-gitwrite-measurements.md docs/sweep-evidence/2026-09-30-m47-t0 ':(exclude)docs/sweep-evidence/2026-09-30-m47-t0/*.bin' ':(exclude)docs/sweep-evidence/2026-09-30-m47-t0/**/*.bin'
git status --short
git commit -m "M47 t0: measurements M1-M6 — the madvise and __mac_syscall census, the fork path, git's command list, the chdir audit, the base count"
```

---

### Task 1: The rows (`retrace-arch`) and the `fsops` fixture

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (rows, after the `128 =>` rename row and the `331 =>` row)
- Modify: `crates/retrace-arch/tests/census.rs`
- Create: `crates/retrace-arch/tests/gitshapes.rs`
- Create: `crates/retrace-guest/c/fsops_dyn.c`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Create: `crates/retrace/tests/gitprims_e2e.rs`

**Interfaces:**
- Consumes: t0 M4's row set (the Decisions list).
- Produces:
  - rows `9 [Path, Path]`, `12 [Path]`, `136 [Path, Scalar]`, `138 [Path, Ptr]`, `333 [Scalar]`, all `Ret::Plain`, forwarded;
  - `retrace_guest::FSOPS_DYN: &str`;
  - `gitprims_e2e.rs`'s helpers `scratch_dir(tag: &str) -> PathBuf` and `records_and_replays_twice(guest: &str, args: &[&str]) -> (util::RunOut, PathBuf)`, which Tasks 2–4 reuse.
  - **Fork's row (2) is NOT added here.** It lands in Task 4, with its refusal, so that no commit on the branch forwards a fork.

- [ ] **Step 1: Write the failing shape tests**

Create `crates/retrace-arch/tests/gitshapes.rs`:

```rust
//! M47 (spec §3b–§3e): the pure halves of git's mechanisms — the path rows, `madvise_effect`,
//! `mac_syscall_model` with `sandbox_check_continuity`, and `fork_refusal_errno`. VM-free. Every
//! number is read from the SDK's headers at test time (M44 R5's method), so a value typed from
//! memory cannot satisfy these.
use retrace_arch::{arg_kinds, ArgKind, Ret};

fn sdk_header(rel: &str) -> String {
    let out = std::process::Command::new("xcrun").arg("--show-sdk-path").output().expect("run xcrun");
    assert!(out.status.success(), "xcrun --show-sdk-path failed");
    let path = format!("{}/usr/include/{rel}", String::from_utf8(out.stdout).unwrap().trim());
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The value of `#define <name> <value>` in `text`: parentheses stripped, decimal or `0x` hex, an
/// optional leading minus. `None` if the header does not define `name` numerically.
fn define(text: &str, name: &str) -> Option<i64> {
    text.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        if w.next() != Some("#define") || w.next() != Some(name) { return None; }
        let v = w.next()?.trim_start_matches('(').trim_end_matches(')');
        let (neg, v) = match v.strip_prefix('-') { Some(r) => (true, r), None => (false, v) };
        let n = match v.strip_prefix("0x") { Some(h) => i64::from_str_radix(h, 16).ok()?, None => v.parse().ok()? };
        Some(if neg { -n } else { n })
    })
}

/// M47 §3b: the numbers the new rows are keyed by are the SDK's.
#[test]
fn the_path_rows_are_the_sdks_numbers() {
    let h = sdk_header("sys/syscall.h");
    for (name, n) in [("SYS_link", 9), ("SYS_chdir", 12), ("SYS_mkdir", 136), ("SYS_utimes", 138),
                      ("SYS___pthread_canceled", 333)] {
        assert_eq!(define(&h, name), Some(n), "{name}");
    }
}

/// M47 §3b: each row is its C prototype, argument by argument. All five are forwarded (R1 for
/// chdir; 333 on the 331 precedent), so none is a refusal and each returns a plain value.
#[test]
fn the_path_rows_are_written_from_their_prototypes() {
    use ArgKind::*;
    let want: [(u64, &[ArgKind]); 5] =
        [(12, &[Path]), (136, &[Path, Scalar]), (9, &[Path, Path]), (138, &[Path, Ptr]), (333, &[Scalar])];
    for (n, kinds) in want {
        let s = arg_kinds(n).unwrap_or_else(|| panic!("syscall {n} has no arg_kinds row"));
        assert_eq!((s.args, s.ret), (kinds, Ret::Plain), "syscall {n}");
    }
}
```

If t0 M4 added rows, add each `(name, n)` to the first test and `(n, kinds)` to the second, and widen the array length.

- [ ] **Step 2: Run them to verify they fail**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-arch --test gitshapes -- --test-threads=1 > $L/t1-red.log 2>&1; echo "exit=$?"
grep -a -E '^test |panicked|no arg_kinds row' $L/t1-red.log
```

Expected: `exit=101`. `the_path_rows_are_the_sdks_numbers` passes. `the_path_rows_are_written_from_their_prototypes` FAILS with `syscall 12 has no arg_kinds row`.

- [ ] **Step 3: Write the fixture and its failing e2e test**

Create `crates/retrace-guest/c/fsops_dyn.c`:

```c
// M47 fixture (spec §3f): the path calls git's `add` and `commit` issue — mkdir, chdir, link and
// utimes, each with no arg_kinds row before M47, plus rename, which has had one since M44 — in the
// order git issues them. <dir> must exist and be empty. Prints markers, and the stat'd mtime and
// link count of the renamed file, which only a forwarded utimes and link can produce.
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: fsops_dyn <dir>\n"); return 2; }
    char d[1024];
    snprintf(d, sizeof d, "%s/d", argv[1]);
    if (mkdir(d, 0755) != 0) { perror("mkdir"); return 1; }
    if (chdir(d) != 0) { perror("chdir"); return 1; }
    int fd = open("f", O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) { perror("open"); return 1; }
    if (write(fd, "fsops\n", 6) != 6) { perror("write"); return 1; }
    close(fd);
    if (link("f", "g") != 0) { perror("link"); return 1; }
    if (rename("g", "h") != 0) { perror("rename"); return 1; }
    struct timeval tv[2] = { { 1000000000, 0 }, { 1234567890, 0 } };
    if (utimes("h", tv) != 0) { perror("utimes"); return 1; }
    struct stat st;
    if (stat("h", &st) != 0) { perror("stat"); return 1; }
    printf("mkdir chdir link rename utimes ok\n");
    printf("h mtime=%ld nlink=%d\n", (long)st.st_mtimespec.tv_sec, (int)st.st_nlink);
    return 0;
}
```

In `crates/retrace-guest/build.rs`, directly after the `timer_dyn` block (the one ending `assert!(status.success(), "timer_dyn guest build failed");`), add:

```rust
    // fsops_dyn: the M47 path-row fixture — mkdir, chdir, link, rename and utimes under a
    // directory the test names. Same recipe as hello_dyn.
    let src = format!("{}/c/fsops_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/fsops_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang fsops_dyn");
    assert!(status.success(), "fsops_dyn guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, after `pub const TIMER_DYN: …`, add:

```rust
/// M47: mkdir, chdir, link, rename and utimes under a directory the test names (spec §3f).
pub const FSOPS_DYN: &str = concat!(env!("OUT_DIR"), "/fsops_dyn");
```

After the `timer_guest_parses` test, add:

```rust
    #[test]
    fn fsops_guest_parses() {
        // M47: proves the build.rs wiring and the path constant; behaviour is gitprims_e2e's.
        let l = parse_macho(&std::fs::read(FSOPS_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

Create `crates/retrace/tests/gitprims_e2e.rs`:

```rust
//! M47 gate (spec §3f): the repo-owned fixtures for git's mechanisms, one test per fixture mode.
//! Each test records, replays twice byte-identically, and asserts the difference M47 makes — never
//! an exit code a weaker failure would also produce. Every fixture is C, built by
//! `crates/retrace-guest/build.rs`, and was run natively by t0 for its reference output
//! (`docs/sweep-evidence/2026-09-30-m47-t0/native-*.out`).
mod util;
use std::path::PathBuf;

/// A fresh, empty directory for a fixture that writes to disk.
fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("retrace-m47-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Record `guest` with `args`, replay it twice, and hold both replays to the recording.
fn records_and_replays_twice(guest: &str, args: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(guest, args);
    assert_eq!(rec.code, 0, "record {guest} {args:?}: {}", rec.stderr);
    for i in 0..2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i} of {guest} {args:?}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} of {guest} {args:?}: stdout differs from the recording");
    }
    (rec, trace)
}

/// RED at `427fa0a`: the recorder stops at the M33 panic for mkdir (136), the first of the new rows
/// the fixture reaches. The on-disk assertions are the difference: only a FORWARDED link and
/// utimes can leave a second link and that mtime, and replay forwards nothing.
#[test]
fn fsops_mkdir_chdir_link_and_utimes_are_forwarded_and_land_on_disk() {
    use std::os::unix::fs::MetadataExt;
    let dir = scratch_dir("fsops");
    let (rec, _) = records_and_replays_twice(retrace_guest::FSOPS_DYN, &[dir.to_str().unwrap()]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "mkdir chdir link rename utimes ok\nh mtime=1234567890 nlink=2\n");
    let st = std::fs::metadata(dir.join("d/h")).unwrap();
    assert_eq!((st.mtime(), st.nlink()), (1_234_567_890, 2), "utimes and link must have reached the host");
    assert_eq!(std::fs::read(dir.join("d/f")).unwrap(), b"fsops\n", "the relative open after chdir must land in <dir>/d");
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace --test gitprims_e2e -- --test-threads=1 > $L/t1-e2e-red.log 2>&1; echo "exit=$?"
grep -a -E '^test |M33: syscall|panicked' $L/t1-e2e-red.log | head -5
```

Expected: `exit=101`, the test FAILS on `record … M33: syscall 136 (136) has no arg_kinds row`.

- [ ] **Step 4: Add the rows and the census entries**

In `crates/retrace-arch/src/lib.rs`, directly after the `128 => row!(P, [Path, Path]),` line, add:

```rust
        // chdir(const char *path): M47 (the 2026-09-30 probe and t0 M4: git calls it for `-C`, and
        // at startup even without `-C`). FORWARDED, and like fchdir (13) below it moves RETRACE's
        // own working directory. On record that is the point: the guest's later relative paths
        // are forwarded too and must resolve where the guest put them. Inert on replay, which
        // forwards nothing (R1). t0 M5 found no relative path retrace opens after the guest starts.
        12 => row!(P, [Path]),
        // mkdir(const char *path, mode_t mode): M47 (git `add`, creating `.git/objects/xx`).
        136 => row!(P, [Path, Scalar]),
        // link(const char *path1, const char *path2): M47 (git `add`, moving a finished object from
        // its temporary name into place).
        9 => row!(P, [Path, Path]),
        // utimes(const char *path, const struct timeval times[2]): M47 (git's object freshen; the
        // probe reached it only after a crashed run left objects behind, and `fsops_dyn` reaches it
        // deterministically). `times` is read for exactly two timevals, 2 × 16 = 32 bytes
        // (bsd/vfs/vfs_syscalls.c `getutimes`, one `copyin` of `sizeof(tv)`), the cited bound —
        // Ptr. NULL means "now".
        138 => row!(P, [Path, Ptr]),
```

Directly after the `331 => row!(P, [Scalar]),` line, add:

```rust
        // __pthread_canceled(int action): xnu-private, one scalar (SDK `SYS___pthread_canceled
        // 333`). M47: git's run-command issues it through `pthread_setcancelstate` around its
        // maintenance fork (the probe's g36 stopped at this row's absence; g40 passed it). Forwarded
        // like 331 above: the kernel applies it to RETRACE's thread, and retrace never cancels a
        // thread, so the flag it sets is inert. Noted, not modelled.
        333 => row!(P, [Scalar]),
```

Add any t0 M4 row in the same form, from its SDK prototype, citing `t0 M4`.

In `crates/retrace-arch/tests/census.rs`, append to the module doc, after the M45 paragraph:

```rust
//!
//! M47 adds 9 `link`, 12 `chdir`, 136 `mkdir`, 138 `utimes` and 333 `__pthread_canceled`, measured
//! from the same `[trap] num=` lines in the 2026-09-30 probe's `git` runs
//! (`docs/sweep-evidence/2026-09-30-m47-probe/git-runs.txt`) and re-measured by M47 t0 M4; 138 is
//! also reached by the repo-owned `fsops_dyn`. All five are forwarded.
```

Replace the `CENSUS` array with (adding 9, 12, 136, 138 and 333, and any t0 M4 row, in order):

```rust
pub const CENSUS: &[i64] = &[
    -89, -70, -50, -47, -36, -33, -29, -28, -27, -26, -24, -19, -18, -15, -14, -12, -10, 1, 3, 4,
    5, 6, 9, 10, 12, 13, 20, 24, 25, 33, 36, 37, 38, 39, 41, 42, 43, 46, 47, 48, 49, 52, 53, 54,
    58, 59, 60, 73, 74, 75, 81, 90, 92, 97, 98, 116, 117, 128, 133, 136, 138, 153, 169, 170, 184,
    189, 191, 194, 195, 197, 199, 202, 220, 228, 244, 266, 286, 294, 327, 328, 329, 331, 333, 336,
    338, 339, 340, 344, 345, 346, 347, 360, 361, 362, 366, 367, 368, 372, 374, 381, 396, 397, 398,
    399, 406, 412, 427, 461, 463, 464, 470, 478, 483, 500, 515, 516, 539, 550, 2147483648,
];
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t1-arch.log 2>&1; echo "exit=$?"
grep -a -E '^test result|FAILED|panicked' $L/t1-arch.log
cargo test -p retrace-guest --lib -- --test-threads=1 fsops > $L/t1-guest.log 2>&1; echo "exit=$?"
cargo test -p retrace --test gitprims_e2e -- --test-threads=1 > $L/t1-e2e.log 2>&1; echo "exit=$?"
grep -a -E '^test |test result' $L/t1-e2e.log
```

Expected: every `exit=0`. `retrace-arch` must include `gitshapes` (2 passed), `census` and `legacy_equivalence` green. `legacy_equivalence` stays green because none of the five rows has an `Fd`, `Dest`, nested or reader kind, so no view differs. `gitprims_e2e`: 1 passed.

- [ ] **Step 6: Clippy, then commit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo clippy --workspace --all-targets -- -D warnings > $L/t1-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace-arch crates/retrace-guest crates/retrace/tests/gitprims_e2e.rs
git commit -m "M47 t1: chdir, mkdir, link, utimes and __pthread_canceled rows, forwarded; the fsops fixture"
```

---

### Task 2: The `madvise` model, test-first

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (the new M47 section before `// ---- M12-signal-delivery`; row 75's comment)
- Modify: `crates/retrace-arch/tests/gitshapes.rs`
- Modify: `crates/retrace-box/src/lib.rs` (`guest_madvise`, beside `guest_kevent_qos`)
- Create: `crates/retrace-box/tests/madvise.rs`
- Modify: `crates/retrace-core/src/lib.rs` (record arm, replay mirror, generic-arm assert)
- Create: `crates/retrace-guest/c/madv_dyn.c`; modify `build.rs`, `src/lib.rs`
- Modify: `crates/retrace/tests/gitprims_e2e.rs`

**Interfaces:**
- Consumes: Task 1's `records_and_replays_twice`; t0 M1(a)'s advice set and M1(d)'s rule.
- Produces:
  - `retrace_arch::{SYS_MADVISE: u64 = 75, MADV_FREE_REUSABLE: u32 = 7, MADV_FREE_REUSE: u32 = 8, MADV_ZERO: u32 = 11, MadviseEffect { NoOp, Zero }, madvise_effect(advice: u32) -> Result<MadviseEffect, String>}`;
  - `Box_::guest_madvise(&self, args: [u64; 8]) -> Result<Vec<Region>, String>`, where the `Vec` is the zero-fill writes the caller applies, and every accepted call returns 0;
  - `retrace_guest::MADV_DYN`;
  - `gitprims_e2e.rs`'s `tamper(trace: &Path, tag: &str, edit: impl FnMut(&mut Event) -> bool) -> PathBuf`, which Task 3 reuses.

- [ ] **Step 1: Write the failing validator tests**

Append to `crates/retrace-arch/tests/gitshapes.rs`, and add `MadviseEffect`, `madvise_effect`, `MADV_FREE_REUSABLE`, `MADV_FREE_REUSE`, `MADV_ZERO` and `SYS_MADVISE` to its `use retrace_arch::{…}` line:

```rust
/// M47 §3c: the advice values are the SDK's (`sys/mman.h`), and so is `madvise`'s number.
#[test]
fn the_madvise_values_are_the_sdks() {
    assert_eq!(define(&sdk_header("sys/syscall.h"), "SYS_madvise"), Some(SYS_MADVISE as i64));
    let m = sdk_header("sys/mman.h");
    for (name, v) in [("MADV_FREE_REUSABLE", MADV_FREE_REUSABLE), ("MADV_FREE_REUSE", MADV_FREE_REUSE),
                      ("MADV_ZERO", MADV_ZERO)] {
        assert_eq!(define(&m, name), Some(i64::from(v)), "{name}");
    }
}

/// M47 §3c: t0 M1(a)'s census, and only it, is modelled.
#[test]
fn the_measured_advice_values_are_modelled() {
    assert_eq!(madvise_effect(MADV_FREE_REUSABLE), Ok(MadviseEffect::NoOp));
    assert_eq!(madvise_effect(MADV_FREE_REUSE), Ok(MadviseEffect::NoOp));
    assert_eq!(madvise_effect(MADV_ZERO), Ok(MadviseEffect::Zero));
}

/// M47 §3c: every other value is refused by value, naming it — the kernel's whole `int` range is
/// swept at its edges and densely where `sys/mman.h` defines values.
#[test]
fn every_other_advice_value_is_refused_naming_it() {
    let accepted = [MADV_FREE_REUSABLE, MADV_FREE_REUSE, MADV_ZERO];
    for v in (0..=64u32).chain([0x7fff_ffff, 0x8000_0000, 0xffff_fff9, u32::MAX]) {
        if accepted.contains(&v) { continue; }
        let e = madvise_effect(v).unwrap_err();
        assert!(e.starts_with(&format!("M47: unmeasured madvise advice {v}.")), "{v}: {e}");
    }
}
```

If t0 M1(a) accepted more values, add each to `accepted`, to the SDK check, and to the modelled test.

- [ ] **Step 2: Run them to verify they fail**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-arch --test gitshapes -- --test-threads=1 > $L/t2-red-arch.log 2>&1; echo "exit=$?"
grep -a -E 'error\[E' $L/t2-red-arch.log | head -5
```

Expected: `exit=101`, a compile error `unresolved import`/`cannot find` for `madvise_effect`.

- [ ] **Step 3: The validator**

In `crates/retrace-arch/src/lib.rs`, directly before the line `// ---- M12-signal-delivery ---…`, add:

```rust
// ---- M47-gitwrite: madvise, __mac_syscall and fork, modelled ---------------------------------------
// Values from the macOS 26 SDK's `sys/mman.h`, `sys/syscall.h` and `sys/errno.h`, which
// tests/gitshapes.rs re-reads at test time.
/// `madvise` (SDK `SYS_madvise 75`). Modelled by advice since M47, never forwarded.
pub const SYS_MADVISE: u64 = 75;
/// `MADV_FREE_REUSABLE` (`sys/mman.h:217`): libmalloc marks a freed span reusable.
pub const MADV_FREE_REUSABLE: u32 = 7;
/// `MADV_FREE_REUSE` (`sys/mman.h:218`): libmalloc takes a reusable span back.
pub const MADV_FREE_REUSE: u32 = 8;
/// `MADV_ZERO` (`sys/mman.h:221`): "zero pages without faulting in additional pages" — libmalloc
/// zeroing a span in place (git `commit`, the probe's g33).
pub const MADV_ZERO: u32 = 11;

/// What a modelled `madvise` advice does to guest memory (M47 §3c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MadviseEffect {
    /// Nothing. Kernel-faithful for these values: a kernel may keep a reusable or freed page's
    /// contents indefinitely, and native reclamation depends on memory pressure, so no guest can
    /// depend on it. A deterministic "never reclaims" is one legal kernel.
    NoOp,
    /// Every resident page of the range reads as zero afterwards. A page never touched already
    /// reads as zero, which is why `MADV_ZERO` need not fault one in.
    Zero,
}

/// The effect of `madvise` advice `advice`, the `int behav` the kernel reads (the caller passes the
/// register's low 32 bits). The accepted set is EXACTLY t0 M1(a)'s census (M47 §3c); every other
/// value is refused by value, naming it, because a guessed effect is either a lie about memory or a
/// forward onto retrace's own backing.
pub fn madvise_effect(advice: u32) -> Result<MadviseEffect, String> {
    match advice {
        MADV_FREE_REUSABLE | MADV_FREE_REUSE => Ok(MadviseEffect::NoOp),
        MADV_ZERO => Ok(MadviseEffect::Zero),
        _ => Err(format!(
            "M47: unmeasured madvise advice {advice}. Modelled, from t0 M1(a)'s census: \
             FREE_REUSABLE (7) and FREE_REUSE (8) as no-ops and ZERO (11) as a zero-fill (M47 §3c). \
             Measure its native effect before modelling it; forwarded, an advice acts on retrace's \
             own backing of the guest range")),
    }
}
```

**If t0 M1(a)'s census found more values,** extend the match by this table and name each value in the error text. The table follows `sys/mman.h` and xnu `bsd/kern/kern_mman.c`:

| value | name | model |
|---|---|---|
| 0–3 | NORMAL, RANDOM, SEQUENTIAL, WILLNEED | `NoOp` (access-pattern hints) |
| 4 | DONTNEED | `NoOp` (xnu deactivates; contents kept) |
| 5 | FREE | `NoOp` (contents undefined afterwards; kept is legal) |
| 6 | ZERO_WIRED_PAGES | `NoOp` (acts at unwire; the guest wires nothing) |
| 9 | CAN_REUSE | **H3**: it reports on the range, which is a query. Keep it refused and route it. |
| 10 | PAGEOUT | `NoOp` (paging is invisible to the guest) |

Give each added value a `pub const` with its `sys/mman.h` line.

Rewrite row 75's comment in place (keep the row itself):

```rust
        // madvise(void *addr, size_t len, int behav): addr is a VM range the kernel neither reads
        // nor writes as data (bsd/kern/kern_mman.c `madvise`). NEVER FORWARDED since M47: the
        // record arm ahead of the generic forward models it by advice (`madvise_effect`,
        // `Box_::guest_madvise`), and the generic arm asserts it never arrives. Forwarded (M2–M46),
        // the host applied `behav` to RETRACE's backing of the guest range. A MADV_FREE_REUSABLE let
        // the host reclaim pages the guest then wrote through stage 2 — a heap corruption record
        // and replay both reproduced (the M47 probe's g35; the /bin/ps hazard M37 named) — and a
        // MADV_ZERO wrote 512 KiB past the diff window (g33). The row stays for the census and the
        // views, and `Ptr` still says what the argument is: a range, rebased when it was forwarded.
        75 => row!(P, [Ptr, Scalar, Scalar]),
```

Run Step 2's command again. Expected: `exit=0`, `gitshapes` 5 passed.

- [ ] **Step 4: Write the failing box tests**

Create `crates/retrace-box/tests/madvise.rs`:

```rust
//! M47 §3c, box level: `Box_::guest_madvise` on a static box, with its mappings made by hand. These
//! pin the paths the fixtures may not reach (Review Focus 1–3): a zero-fill over committed and
//! reserved pages together, a range past the guest's mappings, an unaligned address, an `int`'s
//! upper half, and xnu's rounding of `len`. `gitprims_e2e`'s `madv_dyn` covers the arms end to end.
use retrace_arch::{MADV_FREE_REUSABLE, MADV_ZERO};
use retrace_box::Box_;

const G: u64 = 0x4000;

fn tb() -> Box_ {
    Box_::load(&retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap()))
}

fn args(addr: u64, len: u64, advice: u64) -> [u64; 8] { [addr, len, advice, 0, 0, 0, 0, 0] }

/// Review Focus 1. A reserved page the guest never touched is not committed by the model: it
/// commits as zero on its first touch, on both sides, so writing it would only cost memory.
#[test]
fn zero_writes_every_backed_page_and_no_reserved_one() {
    let mut b = tb();
    let base = b.guest_vm_reserve(0, 4 * G, true);
    assert!(b.commit_reserved_page(base) && b.commit_reserved_page(base + 2 * G));
    b.poke_guest(base, &[0xAB; 16]);
    let w = b.guest_madvise(args(base, 4 * G, MADV_ZERO.into())).unwrap();
    let got: Vec<(u64, usize, bool)> = w.iter().map(|r| (r.ipa, r.bytes.len(), r.bytes.iter().all(|&x| x == 0))).collect();
    assert_eq!(got, vec![(base, G as usize, true), (base + 2 * G, G as usize, true)]);
    assert!(!b.is_mapped(base + G) && !b.is_mapped(base + 3 * G), "the model commits nothing");
    assert_eq!(b.read_guest(base, 16), vec![0xAB; 16], "guest_madvise computes the writes; the caller applies them");
}

#[test]
fn a_noop_advice_writes_nothing_over_a_backed_range() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    assert_eq!(b.guest_madvise(args(base, 2 * G, MADV_FREE_REUSABLE.into())), Ok(vec![]));
}

/// Review Focus 2. Refused whole: the error comes before any write is computed, and names the
/// first page outside.
#[test]
fn a_range_that_runs_past_the_guests_mappings_is_refused_naming_the_page() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    let e = b.guest_madvise(args(base, 3 * G, MADV_ZERO.into())).unwrap_err();
    assert!(e.starts_with(&format!("M47: madvise range page {:#x} is neither backed nor reserved", base + 2 * G)), "{e}");
}

/// t0 M1(d): native madvise answers EINVAL off a page; the model refuses rather than guessing.
#[test]
fn an_address_off_a_16k_page_is_refused() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    let e = b.guest_madvise(args(base + 0x1000, G, MADV_FREE_REUSABLE.into())).unwrap_err();
    assert!(e.starts_with(&format!("M47: madvise range starts at {:#x}, not on a 16 KiB page", base + 0x1000)), "{e}");
}

/// Review Focus 3. `behav` is a C `int`; the kernel reads 32 bits of the register.
#[test]
fn an_advice_ints_upper_half_is_ignored_as_the_kernel_ignores_it() {
    let mut b = tb();
    let base = b.guest_vm_map(0, G, true, false);
    for bit in 32..64 {
        assert_eq!(b.guest_madvise(args(base, G, u64::from(MADV_FREE_REUSABLE) | 1 << bit)), Ok(vec![]), "bit {bit}");
    }
}

/// t0 M1(d): xnu rounds `len` up to the page, so a zero-fill one byte into a page zeroes all of it.
#[test]
fn a_length_short_of_a_page_zeroes_the_whole_last_page_as_xnu_rounds_it() {
    let mut b = tb();
    let base = b.guest_vm_map(0, 2 * G, true, false);
    let w = b.guest_madvise(args(base, G + 1, MADV_ZERO.into())).unwrap();
    assert_eq!(w.iter().map(|r| r.ipa).collect::<Vec<_>>(), vec![base, base + G]);
}
```

If t0 M1(d) measured that xnu does **not** round `len`, change the last test to expect `vec![base]` and rename it `a_length_short_of_a_page_zeroes_only_whole_pages`. Step 6's loop then uses `addr + len` truncated to the page instead of rounded up.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-box --test madvise -- --test-threads=1 > $L/t2-red-box.log 2>&1; echo "exit=$?"
grep -a -E 'error\[E' $L/t2-red-box.log | head -3
```

Expected: `exit=101`, `no method named guest_madvise`.

- [ ] **Step 5: The box method**

In `crates/retrace-box/src/lib.rs`, directly before `/// \`args\` as \`0x…,0x…\` on one line` (the `fmt_args` doc), add:

```rust
    /// M47 §3c: `madvise(addr, len, behav)` (75), **modelled, never forwarded**. Forwarded, the
    /// host applied the advice to RETRACE's backing of the guest range: a `MADV_FREE_REUSABLE` let
    /// the host reclaim pages the guest later wrote through stage 2, a heap corruption record and
    /// replay both reproduced (the 2026-09-30 probe's g35), and a `MADV_ZERO` wrote 512 KiB past the
    /// diff window (g33).
    ///
    /// The advice comes from `retrace_arch::madvise_effect`, read as the `int` the kernel reads. The
    /// range must start on a 16 KiB page, and every page of it, `len` rounded up as xnu rounds it,
    /// must be the guest's: backed, or inside a reservation (`commit_reserved_page`'s bookkeeping;
    /// t0 M1(d)). Anything else is refused by value rather than answered with a guessed errno, and
    /// the refusal comes before any write is computed.
    ///
    /// Returns the call's writes for the caller to apply with `apply_and_return`, so a watched range
    /// sees a zero-fill as a write. `Zero` gives one zeroed page per BACKED page of the range; a
    /// reserved page commits as zero, so it needs none. `NoOp` gives none. The writes are recomputed
    /// identically on both sides and never recorded (R3). Every accepted call returns 0.
    pub fn guest_madvise(&self, args: [u64; 8]) -> Result<Vec<Region>, String> {
        let (addr, len) = (args[0], args[1]);
        let effect = retrace_arch::madvise_effect(args[2] as u32)
            .map_err(|why| format!("{why}. args=[{}]", Self::fmt_args(args)))?;
        let g = GRANULE as u64;
        if addr % g != 0 {
            return Err(format!("M47: madvise range starts at {addr:#x}, not on a 16 KiB page (t0 M1(d)). \
                args=[{}]", Self::fmt_args(args)));
        }
        let end = addr.checked_add(len).and_then(|e| e.checked_add(g - 1)).map(|e| e & !(g - 1))
            .ok_or_else(|| format!("M47: madvise range {addr:#x}+{len:#x} overflows. args=[{}]", Self::fmt_args(args)))?;
        let mut writes = Vec::new();
        let mut page = addr;
        while page < end {
            let backed = self.host_span(page).is_some();
            if !backed && !self.reservations.iter().any(|&(s, l)| (s..s + l).contains(&page)) {
                return Err(format!("M47: madvise range page {page:#x} is neither backed nor reserved — the \
                    range runs outside the guest's mappings. args=[{}]", Self::fmt_args(args)));
            }
            if backed && effect == retrace_arch::MadviseEffect::Zero {
                writes.push(Region { ipa: page, bytes: vec![0u8; GRANULE] });
            }
            page += g;
        }
        Ok(writes)
    }
```

Run Step 4's command again. Expected: `exit=0`, 6 passed.

- [ ] **Step 6: The fixture and the failing e2e tests**

Create `crates/retrace-guest/c/madv_dyn.c`:

```c
// M47 fixture (spec §3f): madvise, by mode.
//   zero  — map 1 MiB, fill it with 0xAB, MADV_ZERO the first 512 KiB, report both halves.
//   reuse — fill, MADV_FREE_REUSABLE, MADV_FREE_REUSE, report whether the bytes were kept, then
//           write a pattern and read it back.
//   bad   — an advice outside the measured set (MADV_CAN_REUSE, 9); prints what it returned.
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

#define MIB (1u << 20)

static int all(const unsigned char *p, size_t n, unsigned char v) {
    for (size_t i = 0; i < n; i++) if (p[i] != v) return 0;
    return 1;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "zero";
    unsigned char *p = mmap(NULL, MIB, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (p == MAP_FAILED) { perror("mmap"); return 1; }
    memset(p, 0xAB, MIB);
    if (strcmp(mode, "zero") == 0) {
        if (madvise(p, MIB / 2, MADV_ZERO) != 0) { printf("zero errno=%d\n", errno); return 1; }
        printf("zero low=%s high=%s\n", all(p, MIB / 2, 0) ? "zeros" : "dirty",
               all(p + MIB / 2, MIB / 2, 0xAB) ? "kept" : "changed");
        return 0;
    }
    if (strcmp(mode, "reuse") == 0) {
        if (madvise(p, MIB, MADV_FREE_REUSABLE) != 0) { printf("reusable errno=%d\n", errno); return 1; }
        if (madvise(p, MIB, MADV_FREE_REUSE) != 0) { printf("reuse errno=%d\n", errno); return 1; }
        printf("reuse %s\n", all(p, MIB, 0xAB) ? "kept" : "dropped");
        memset(p, 0xCD, MIB);
        printf("reuse %s\n", all(p, MIB, 0xCD) ? "ok" : "bad");
        return 0;
    }
    if (strcmp(mode, "bad") == 0) {
        int rc = madvise(p, MIB, MADV_CAN_REUSE);
        printf("bad rc=%d errno=%d\n", rc, rc ? errno : 0);
        return 0;
    }
    fprintf(stderr, "unknown mode %s\n", mode);
    return 2;
}
```

If t0 M1(a)'s census contains 9, use the lowest value in 0–11 the census does not contain, in the fixture, its comment and the `bad` test.

In `build.rs`, after the `fsops_dyn` block:

```rust
    // madv_dyn: the M47 madvise fixture — modes zero, reuse and bad. Same recipe as hello_dyn.
    let src = format!("{}/c/madv_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/madv_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang madv_dyn");
    assert!(status.success(), "madv_dyn guest build failed");
```

In `src/lib.rs`, after `FSOPS_DYN`:

```rust
/// M47: `madvise` by mode — `zero`, `reuse`, `bad` (spec §3f).
pub const MADV_DYN: &str = concat!(env!("OUT_DIR"), "/madv_dyn");
```

and after `fsops_guest_parses`:

```rust
    #[test]
    fn madv_guest_parses() {
        // M47: proves the build.rs wiring and the path constant; behaviour is gitprims_e2e's.
        let l = parse_macho(&std::fs::read(MADV_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

In `crates/retrace/tests/gitprims_e2e.rs`, change `use std::path::PathBuf;` to:

```rust
use retrace_trace::{Event, Region};
use std::path::{Path, PathBuf};
```

and append:

```rust
/// One madvise landmark, as the tests read it.
#[derive(Debug)]
struct Madv { len: u64, advice: u32, ret: u64, ret1: u64, err: bool, writes: usize }

/// Every madvise landmark in `trace`.
fn madvise_events(trace: &Path) -> Vec<Madv> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().filter_map(|e| match e {
        Event::Syscall { num, args, ret, ret1, err, writes, .. } if num == retrace_arch::SYS_MADVISE =>
            Some(Madv { len: args[1], advice: args[2] as u32, ret, ret1, err, writes: writes.len() }),
        _ => None,
    }).collect()
}

/// A copy of `trace` with the FIRST event `edit` accepts rewritten in place (`edit` returns true
/// when it edited one). `Writer` re-frames every record with a fresh CRC, so only the content lies —
/// `util::tamper_last_write`'s method.
fn tamper(trace: &Path, tag: &str, mut edit: impl FnMut(&mut Event) -> bool) -> PathBuf {
    let mut ev = retrace_trace::Reader::open(trace).unwrap();
    assert!(ev.iter_mut().any(&mut edit), "no event in {} to tamper", trace.display());
    let out = trace.with_extension(format!("{tag}.tampered.bin"));
    let mut w = retrace_trace::Writer::create(&out).unwrap();
    for e in &ev { w.append(e).unwrap(); }
    out
}

/// RED at `427fa0a`: the forwarded MADV_ZERO writes 512 KiB past the diff window and the guard
/// band stops the recorder. The difference M47 makes is the zeros landing AND the landmark carrying
/// none of them (R3: recomputed on both sides, never recorded).
#[test]
fn madv_zero_zeroes_the_range_by_recompute_and_records_no_bytes() {
    let (rec, trace) = records_and_replays_twice(retrace_guest::MADV_DYN, &["zero"]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "zero low=zeros high=kept\n");
    let zeros: Vec<_> = madvise_events(&trace).into_iter().filter(|e| e.advice == retrace_arch::MADV_ZERO && e.len == 0x8_0000).collect();
    assert_eq!(zeros.len(), 1, "the fixture's one 512 KiB MADV_ZERO must be a landmark");
    let z = &zeros[0];
    assert_eq!((z.ret, z.ret1, z.err, z.writes), (0, 0, false, 0),
        "ret 0, no ret1, no error, and NO writes: the zeros are recomputed, not recorded (R3)");
}

/// No RED at `427fa0a` unless t0 M1(c) found one: it guards the no-op semantics. Every madvise in
/// the run — the fixture's two and libmalloc's own — is a landmark with no writes.
#[test]
fn madv_free_reusable_and_reuse_are_no_ops_that_keep_the_bytes() {
    let (rec, trace) = records_and_replays_twice(retrace_guest::MADV_DYN, &["reuse"]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "reuse kept\nreuse ok\n");
    let ev = madvise_events(&trace);
    for adv in [retrace_arch::MADV_FREE_REUSABLE, retrace_arch::MADV_FREE_REUSE] {
        assert!(ev.iter().any(|e| e.advice == adv && e.len == 1 << 20), "the fixture's advice {adv} over 1 MiB must be a landmark");
    }
    assert!(ev.iter().all(|e| (e.ret, e.ret1, e.err, e.writes) == (0, 0, false, 0)), "{ev:x?}");
}

/// No RED at `427fa0a`: the value was forwarded. After M47 it stops the recorder, by value.
#[test]
fn an_unmeasured_advice_stops_the_recorder_naming_the_value() {
    let (rec, _) = util::record_dynamic_args(retrace_guest::MADV_DYN, &["bad"]);
    assert_ne!(rec.code, 0, "an unmeasured advice must not record: stdout {:?}", String::from_utf8_lossy(&rec.stdout));
    assert!(rec.stderr.contains("M47: unmeasured madvise advice 9."), "{}", rec.stderr);
}

/// Review Focus 4. A recording whose madvise landmark carries writes is not one this build wrote;
/// replay refuses it by name rather than applying bytes the model never made.
#[test]
fn replay_refuses_a_madvise_landmark_that_carries_writes() {
    let (_, trace) = records_and_replays_twice(retrace_guest::MADV_DYN, &["zero"]);
    let t = tamper(&trace, "madv", |e| match e {
        Event::Syscall { num, args, writes, .. } if *num == retrace_arch::SYS_MADVISE && args[2] as u32 == retrace_arch::MADV_ZERO => {
            writes.push(Region { ipa: args[0], bytes: vec![0; 16] });
            true
        }
        _ => false,
    });
    let rp = util::replay(&t);
    assert_eq!(rp.code, 3, "a tampered madvise landmark must be a divergence: {}", rp.stderr);
    assert!(rp.stderr.contains("madvise recorded ret=0x0 ret1=0x0 err=false with 1 write(s)"), "{}", rp.stderr);
}
```

Check that `crates/retrace/Cargo.toml` has `retrace-trace` and `retrace-arch` as dev-dependencies (`grep -n 'retrace-trace\|retrace-arch' crates/retrace/Cargo.toml`). `exec_e2e.rs` already uses both, so they are.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace --test gitprims_e2e --no-fail-fast -- --test-threads=1 > $L/t2-red-e2e.log 2>&1; echo "exit=$?"
grep -a -E '^test |guard band|canary|M47:|panicked at' $L/t2-red-e2e.log | head -20
```

Expected: `exit=101`. The two tests the model makes pass are RED:
- `madv_zero…` fails on `record … ` with the M30 guard-band panic (the base's forwarded MADV_ZERO);
- `an_unmeasured_advice…` fails because `rec.code` is 0 with no `M47:` line;
- `replay_refuses…` fails at its `records_and_replays_twice`, on the same guard band.

`madv_free_reusable…` may pass: that is t0 M1(c)'s finding. Record each symptom.

- [ ] **Step 7: The arms and the assert**

In `crates/retrace-core/src/lib.rs`, directly after the M38 exec-refusal record arm (the arm ending `b.apply_and_return(e, true, &[]);` before `// Every other syscall goes through the general memory-diff engine`), add:

```rust
            // M47 §3c: madvise is MODELLED, never forwarded (see Box_::guest_madvise). The zeros a
            // MADV_ZERO makes are applied here and recomputed by the mirror, never recorded (R3), so
            // the event carries no writes. A refusal panics before anything is appended (R5).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MADVISE => {
                let zeros = b.guest_madvise(args).unwrap_or_else(|m| panic!("{m}"));
                w.append(&Event::Syscall { num, args, ret: 0, ret1: 0, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append madvise: {e}"))?; count += 1;
                b.apply_and_return(0, false, &zeros);
            }
```

In the generic arm, directly after the `kevent_qos` assert, add:

```rust
                // M47 §3c: madvise joins them. Forwarded, the advice acts on RETRACE's backing of
                // the guest range, and a MADV_FREE_REUSABLE there is a heap corruption that record
                // and replay both reproduce. The arm above models it; this assert makes "never
                // forwarded" a checked fact rather than an arm-ordering accident.
                assert!(num != retrace_arch::SYS_MADVISE,
                    "madvise (75) reached the generic forward arm — it must be modelled above (M47). \
                     Forwarded, its advice acts on retrace's own backing of the guest range.");
```

In `ReplaySession::advance`, directly after the M38 exec mirror (the `if let Some(e) = retrace_arch::exec_refusal_errno(num) { … return self.finish_event(); }` block), add:

```rust
                            // M47 §3c: the madvise arm's mirror (symmetry rule 1). The zeros are
                            // recomputed from the same box state and applied; the recording carries
                            // none (R3), so a recorded write, return or error is a trace this build
                            // did not write.
                            if num == retrace_arch::SYS_MADVISE {
                                let zeros = match self.b.guest_madvise(args) {
                                    Ok(z) => z,
                                    Err(m) => return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "madvise refused on replay, though the recording accepted it \
                                         — replay diverged before this landmark: {m}") }),
                                };
                                if *ret != 0 || *ret1 != 0 || *err || !writes.is_empty() {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "madvise recorded ret={ret:#x} ret1={ret1:#x} err={err} with {} write(s); \
                                         the model records 0, 0, false and none (R3)", writes.len()) });
                                }
                                self.b.apply_and_return(0, false, &zeros);
                                return self.finish_event();
                            }
```

- [ ] **Step 8: Run the tests to verify they pass**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace --test gitprims_e2e --no-fail-fast -- --test-threads=1 > $L/t2-e2e.log 2>&1; echo "exit=$?"
grep -a -E '^test |test result' $L/t2-e2e.log
cargo test -p retrace-guest --lib -- --test-threads=1 madv > $L/t2-guest.log 2>&1; echo "exit=$?"
for t in hello_dyn_e2e cpython_e2e sysbin_e2e jq_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t2-$t.log 2>&1; echo "$t exit=$?"; done
grep -a -h -E 'test result|SKIPPED' $L/t2-hello_dyn_e2e.log $L/t2-cpython_e2e.log $L/t2-sysbin_e2e.log $L/t2-jq_e2e.log
```

Expected: every `exit=0`; `gitprims_e2e` 5 passed. These are the gates nearest the change (spec §3f): CPython issues about 44 `madvise(…, 7)` per run, and `/bin/ps` was the named hazard. A `SKIPPED` line is acceptable only where the tool is absent.

- [ ] **Step 9: Controls, on the committed tree**

Commit first (Step 10), then run each control and restore with `git checkout -- crates/`:

1. **R3's assert.** In the record arm, change `writes: vec![]` to `writes: zeros.clone()`. Run `madv_zero_zeroes_the_range_by_recompute_and_records_no_bytes`. Expected: RED, either at replay (the mirror refuses the recorded writes, exit 3) or at the empty-writes assert. Record which.
2. **The refusal.** In `madvise_effect`, add `9 => Ok(MadviseEffect::NoOp),`. Run `an_unmeasured_advice_stops_the_recorder_naming_the_value`. Expected: RED, `rec.code` 0.
3. **The generic-arm assert.** Delete the madvise record arm. Run `madv_free_reusable_and_reuse_are_no_ops_that_keep_the_bytes`. Expected: RED, `madvise (75) reached the generic forward arm`.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace --test gitprims_e2e -- --test-threads=1 <test name> > $L/t2-control-<n>.log 2>&1; echo "exit=$?"
grep -a -E '^test |panicked at|DIVERGENCE|assert' $L/t2-control-<n>.log | head -5
git checkout -- crates/
```

- [ ] **Step 10: Clippy, then commit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo clippy --workspace --all-targets -- -D warnings > $L/t2-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace-arch crates/retrace-box crates/retrace-core crates/retrace-guest crates/retrace/tests/gitprims_e2e.rs
git commit -m "M47 t2: madvise modelled by advice, never forwarded — zeros recomputed on both sides (R3), a generic-arm assert"
```

---

### Task 3: The `__mac_syscall` model, test-first

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (rows 381 and `0x8000_0000`; `NestedDest`'s doc; the M47 section; `the_nested_pointer_family_is_pinned_by_number`)
- Modify: `crates/retrace-arch/tests/legacy_equivalence.rs`, `tests/gitshapes.rs`
- Modify: `crates/retrace-box/src/lib.rs` (`MacSyscall`, `read_guest_cstr`, `guest_mac_syscall`, `host_amfi_dyld_policy`)
- Create: `crates/retrace-box/tests/macsyscall.rs`
- Modify: `crates/retrace-core/src/lib.rs` (record arm, replay mirror)
- Create: `crates/retrace-guest/c/rpath_dyn.c`, `c/librpath_dyn.c`; modify `build.rs`, `src/lib.rs`
- Modify: `crates/retrace/tests/gitprims_e2e.rs`

**Interfaces:**
- Consumes: Task 2's `tamper`; t0 M2's pairs and operations (Decisions).
- Produces:
  - `retrace_arch::{SYS_MAC_SYSCALL: u64 = 381, MAC_MAX_POLICY_NAME: usize = 32, AMFI_DYLD_POLICY_SELF: u32 = 0x5a, SANDBOX_CHECK: u32 = 2, SANDBOX_OPERATION_MAX: usize = 64, MacCall { AmfiDyldPolicy, SandboxCheck }, mac_syscall_model(policy: &[u8], call: u32) -> Result<MacCall, String>, sandbox_check_continuity(operation: &[u8]) -> Result<u64, String>}`;
  - `retrace_box::MacSyscall { AmfiDyldPolicy { in_flags: u64, out_ipa: u64 }, SandboxCheck { errno: u64 } }`;
  - `Box_::read_guest_cstr(&mut self, va: u64, cap: usize) -> Result<Vec<u8>, String>`;
  - `Box_::guest_mac_syscall(&mut self, args: [u64; 8]) -> Result<MacSyscall, String>`;
  - `retrace_box::host_amfi_dyld_policy(in_flags: u64) -> Result<u64, u64>` (record-only);
  - `retrace_guest::RPATH_DYN`.

- [ ] **Step 1: Write the failing validator tests**

Append to `gitshapes.rs`, adding `MacCall`, `mac_syscall_model`, `sandbox_check_continuity`, `AMFI_DYLD_POLICY_SELF`, `SANDBOX_CHECK` and `SYS_MAC_SYSCALL` to the `use` line:

```rust
/// M47 §3d: the two modelled pairs, and `__mac_syscall`'s number from the SDK.
#[test]
fn the_two_modelled_mac_syscalls_are_classified() {
    assert_eq!(define(&sdk_header("sys/syscall.h"), "SYS___mac_syscall"), Some(SYS_MAC_SYSCALL as i64));
    assert_eq!(mac_syscall_model(b"AMFI", AMFI_DYLD_POLICY_SELF), Ok(MacCall::AmfiDyldPolicy));
    assert_eq!(mac_syscall_model(b"Sandbox", SANDBOX_CHECK), Ok(MacCall::SandboxCheck));
}

/// M47 §3d: a policy with any bit flipped or any byte missing, or a call with any bit flipped, is a
/// different call, and is refused naming what it got.
#[test]
fn every_flipped_or_truncated_policy_or_call_is_refused() {
    for (policy, call) in [(&b"AMFI"[..], AMFI_DYLD_POLICY_SELF), (&b"Sandbox"[..], SANDBOX_CHECK)] {
        for i in 0..policy.len() {
            for bit in 0..8 {
                let mut p = policy.to_vec();
                p[i] ^= 1 << bit;
                let e = mac_syscall_model(&p, call).unwrap_err();
                assert!(e.starts_with(&format!("M47: unmodelled __mac_syscall policy {:?} call {call:#x}", String::from_utf8_lossy(&p))), "{e}");
            }
        }
        for n in 0..policy.len() { assert!(mac_syscall_model(&policy[..n], call).is_err(), "{policy:?} cut to {n}"); }
        for bit in 0..32 { assert!(mac_syscall_model(policy, call ^ (1 << bit)).is_err(), "{policy:?} call bit {bit}"); }
    }
    assert!(mac_syscall_model(b"AMFI", SANDBOX_CHECK).is_err() && mac_syscall_model(b"Sandbox", AMFI_DYLD_POLICY_SELF).is_err());
}

/// R7 (t0 M2(b)): Sandbox call 2 keeps the errno the pre-M47 forward returned, by operation name.
#[test]
fn sandbox_continuity_is_by_operation_and_refuses_the_rest() {
    assert_eq!(sandbox_check_continuity(b"syscall-unix"), Ok(14));
    assert_eq!(sandbox_check_continuity(b"file-write-data"), Ok(22));
    for op in [&b"syscall-uni"[..], b"syscall-unix2", b"file-read-data", b"", b"SYSCALL-UNIX"] {
        let e = sandbox_check_continuity(op).unwrap_err();
        assert!(e.starts_with(&format!("M47: unmeasured Sandbox operation {:?}", String::from_utf8_lossy(op))), "{e}");
    }
}

/// M47 §3d: both spellings of `__mac_syscall` are nested destinations, so the generic arm's
/// `writes_via_nested_pointer` assert refuses an unmodelled pair.
#[test]
fn the_mac_syscall_rows_are_nested_destinations() {
    use ArgKind::*;
    for n in [SYS_MAC_SYSCALL, 0x8000_0000] {
        assert_eq!(arg_kinds(n).unwrap().args, &[Path, Scalar, NestedDest], "{n:#x}");
        assert!(retrace_arch::writes_via_nested_pointer(n), "{n:#x}");
    }
}
```

If t0 M2 measured other Sandbox operations (after H8 was cleared with the operator), add each to the accepted assertions.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-arch --test gitshapes -- --test-threads=1 > $L/t3-red-arch.log 2>&1; echo "exit=$?"
```

Expected: `exit=101`, compile errors for the missing names.

- [ ] **Step 2: The validator and the kind change**

Append to the M47 section in `crates/retrace-arch/src/lib.rs`:

```rust
/// `__mac_syscall` (SDK `SYS___mac_syscall 381`). Modelled per `(policy, call)` since M47, never
/// forwarded; `MAC_SYSCALL_MAGIC` (0x8000_0000) is dyld's inline spelling, serviced separately.
pub const SYS_MAC_SYSCALL: u64 = 381;
/// `MAC_MAX_POLICY_NAME` (xnu `security/mac.h`): the buffer `__mac_syscall` `copyinstr`s the policy
/// name into, NUL included (security/mac_base.c `__mac_syscall`).
pub const MAC_MAX_POLICY_NAME: usize = 32;
/// dyld's `amfi_check_dyld_policy_self` call: `arg` is `{u64 inFlags; u64 *outFlags}`, and AMFI
/// writes `*outFlags` (the M47 probe's amfi-disasm.txt).
pub const AMFI_DYLD_POLICY_SELF: u32 = 0x5a;
/// Sandbox's call 2, a sandbox check, issued by dyld's `sandbox_check_common` and libsystem_sandbox's
/// `rootless_check_trusted_internal`. Its struct holds guest pointers at +0 and +16; the operation
/// name is at `*(arg + 16)` (the M47 probe's sandbox-call2.txt).
pub const SANDBOX_CHECK: u32 = 2;
/// M47's own cap on a Sandbox operation name, NUL included: four times the longest measured one
/// (`file-write-data`, 15 bytes). A longer name is refused, not truncated.
pub const SANDBOX_OPERATION_MAX: usize = 64;

/// A modelled `__mac_syscall` (M47 §3d).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacCall {
    /// `("AMFI", 0x5a)`: answered by the host, about retrace's own process (R4).
    AmfiDyldPolicy,
    /// `("Sandbox", 2)`: answered with the errno the pre-M47 forward returned (R7).
    SandboxCheck,
}

/// Classify `__mac_syscall(policy, call, …)`. `call` is the `int` the kernel reads (the caller
/// passes the register's low 32 bits). Exactly the two measured pairs are modelled; every other is
/// refused, naming both, because its struct may carry a pointer the policy writes through.
pub fn mac_syscall_model(policy: &[u8], call: u32) -> Result<MacCall, String> {
    match (policy, call) {
        (b"AMFI", AMFI_DYLD_POLICY_SELF) => Ok(MacCall::AmfiDyldPolicy),
        (b"Sandbox", SANDBOX_CHECK) => Ok(MacCall::SandboxCheck),
        _ => Err(format!(
            "M47: unmodelled __mac_syscall policy {:?} call {call:#x}. Modelled: (\"AMFI\", 0x5a) and \
             (\"Sandbox\", 0x2) (M47 §3d). A policy's argument struct may carry pointers the policy \
             writes through, so it is never forwarded (ArgKind::NestedDest); measure this one's \
             struct and its native answer before modelling it", String::from_utf8_lossy(policy))),
    }
}

/// R7: Sandbox call 2's answer is the errno the pre-M47 forward returned, which the corpus passes
/// with. It is keyed by the operation name at `*(arg + 16)`, the one field that tells the two
/// measured callers apart (the M47 probe's sandbox-call2.txt; t0 M2(a) re-measured it across the
/// corpus). The forward's errno was the host kernel's verdict on a struct whose nested guest
/// pointers it could not follow — continuity, not fidelity; t0 M2(b) has the native answers.
pub fn sandbox_check_continuity(operation: &[u8]) -> Result<u64, String> {
    match operation {
        b"syscall-unix" => Ok(14),
        b"file-write-data" => Ok(22),
        _ => Err(format!(
            "M47: unmeasured Sandbox operation {:?}. Measured: \"syscall-unix\" (dyld's \
             sandbox_check_common, EFAULT 14) and \"file-write-data\" (libsystem_sandbox's \
             rootless_check_trusted_internal, EINVAL 22) (t0 M2, R7). Measure what the pre-M47 \
             forward returned for this one before adding it", String::from_utf8_lossy(operation))),
    }
}
```

Replace rows 381 and `0x8000_0000` and their comments with:

```rust
        // __mac_syscall(char *policy, int call, void *arg): policy is `copyinstr`'d into a
        // MAC_MAX_POLICY_NAME (32) buffer (security/mac_base.c `__mac_syscall`) — Path. arg: xnu
        // hands the raw pointer to the policy's `mpo_policy_syscall(p, call, arg)`, and the policy
        // reads a per-call struct of ITS choosing, which may carry pointers the policy WRITES
        // through. AMFI's dyld-policy call (0x5a) takes `{u64 inFlags; u64 *outFlags}` and writes
        // `*outFlags` (the M47 probe's amfi-disasm.txt); Sandbox's call 2 struct holds guest stack
        // pointers at +0 and +16 (sandbox-call2.txt). So NestedDest since M47. Forwarded (M2–M46), a
        // guest address reached the host as a host address; the EFAULT every dyld guest got was
        // luck, because the stack VA fell in retrace's own __PAGEZERO. MODELLED per (policy, call)
        // above the generic forward (`Box_::guest_mac_syscall`), and the generic arm's
        // `writes_via_nested_pointer` assert refuses any other pair.
        381 => row!(P, [Path, Scalar, NestedDest]),
        // MAC_SYSCALL_MAGIC (0x8000_0000): not a syscall number. dyld's inline
        // `__mac_syscall("Sandbox", …)` loads this magic into x16 (`movz x16, #0x8000, lsl #16`);
        // only a platform binary may issue it, so retrace-core synthesizes the reply and never
        // forwards it (its `MAC_SYSCALL_MAGIC` arm). The argument shape is __mac_syscall's,
        // NestedDest included since M47.
        0x8000_0000 => row!(P, [Path, Scalar, NestedDest]),
```

In `ArgKind::NestedDest`'s doc, replace the sentence beginning `/// The six rows are the MEASURED family, not a proof of exhaustiveness` with `/// The iovec and msghdr rows are the MEASURED family, not a proof of exhaustiveness`. Leave the rest of that sentence. Then append this paragraph at the end of the doc, directly before `NestedDest,`:

```rust
    ///
    /// M47 added `__mac_syscall` (381) and its `MAC_SYSCALL_MAGIC` band: a policy may write through
    /// a pointer inside `arg` (AMFI's `outFlags`). Those two are modelled above the generic arm per
    /// `(policy, call)`, so the generic arm's assert refuses only an unmodelled pair.
```

In the in-crate test `the_nested_pointer_family_is_pinned_by_number`, change `for num in [120u64, 411, 27, 401, 540, 480]` to `for num in [120u64, 411, 27, 401, 540, 480, 381, 0x8000_0000]`.

In `crates/retrace-arch/tests/legacy_equivalence.rs`, append to `EXPECTED_DIFFS`, before its closing `];`:

```rust
    // M47: __mac_syscall's policy writes through `outFlags` inside `arg` (AMFI 0x5a), so its row and
    // the MAC_SYSCALL_MAGIC band's are NestedDest; the legacy nested table predates both.
    (381, View::NestedPointer, "__mac_syscall(policy, call, arg): a policy writes through a pointer inside arg — M47; exercised (every dynamic guest)"),
    (0x8000_0000, View::NestedPointer, "MAC_SYSCALL_MAGIC: __mac_syscall's shape, NestedDest since M47 — exercised (every dynamic guest, dyld's inline Sandbox check)"),
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t3-arch.log 2>&1; echo "exit=$?"
grep -a -E '^test result|FAILED|panicked' $L/t3-arch.log
```

Expected: `exit=0`, `gitshapes` 9 passed, `legacy_equivalence` and the lib's unit tests green.

- [ ] **Step 3: Write the failing box tests**

Create `crates/retrace-box/tests/macsyscall.rs`:

```rust
//! M47 §3d, box level: `Box_::guest_mac_syscall` classifies from guest memory on a static box (MMU
//! off, so VA == IPA), and `host_amfi_dyld_policy` asks the host. These pin what the fixtures may
//! not reach (Review Focus 3 and 5): an `int` call's upper half, an unterminated policy, and
//! unmodelled names. `gitprims_e2e`'s `rpath_dyn` covers the arms end to end, including the
//! shared-cache page-in `read_guest_cstr` does for a real dyld.
use retrace_box::{Box_, MacSyscall};

fn tb() -> Box_ {
    Box_::load(&retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap()))
}

fn args(policy: u64, call: u64, arg: u64) -> [u64; 8] { [policy, call, arg, 0, 0, 0, 0, 0] }

/// Lay out a policy name at `t - 0x400`, a Sandbox struct at `t - 0x300` whose +16 points at an
/// operation name at `t - 0x100`, and return `t`.
fn sandbox(b: &mut Box_, op: &[u8]) -> u64 {
    let t = b.stack_top();
    b.poke_guest(t - 0x400, b"Sandbox\0");
    b.poke_guest(t - 0x300 + 16, &(t - 0x100).to_le_bytes());
    b.poke_guest(t - 0x100, &[op, b"\0"].concat());
    t
}

/// Review Focus 3: `call` is a C `int`; bit 32 is not the kernel's.
#[test]
fn amfi_is_classified_with_its_flags_and_the_ipa_of_out_flags() {
    let mut b = tb();
    let t = b.stack_top();
    b.poke_guest(t - 0x400, b"AMFI\0");
    b.poke_guest(t - 0x300, &[2u64.to_le_bytes(), (t - 0x200).to_le_bytes()].concat());
    for call in [0x5a, 0x5a | 1 << 32] {
        assert_eq!(b.guest_mac_syscall(args(t - 0x400, call, t - 0x300)),
                   Ok(MacSyscall::AmfiDyldPolicy { in_flags: 2, out_ipa: t - 0x200 }), "call {call:#x}");
    }
}

#[test]
fn sandbox_is_answered_by_its_operation() {
    let mut b = tb();
    let t = sandbox(&mut b, b"syscall-unix");
    assert_eq!(b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)), Ok(MacSyscall::SandboxCheck { errno: 14 }));
    let t = sandbox(&mut b, b"file-write-data");
    assert_eq!(b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)), Ok(MacSyscall::SandboxCheck { errno: 22 }));
}

/// Review Focus 5: `copyinstr` into a 32-byte buffer finds no NUL and fails; so does the model.
#[test]
fn an_unterminated_policy_is_refused_as_copyinstr_would() {
    let mut b = tb();
    let t = b.stack_top();
    b.poke_guest(t - 0x400, &[b'A'; 40]);
    let e = b.guest_mac_syscall(args(t - 0x400, 0x5a, t - 0x300)).unwrap_err();
    assert!(e.starts_with("M47: __mac_syscall policy name: no NUL within 32 bytes"), "{e}");
}

/// Review Focus 5: an unmodelled policy, and an unmeasured Sandbox operation, are refused by name.
#[test]
fn an_unmodelled_policy_or_operation_is_refused_by_name() {
    let mut b = tb();
    let t = b.stack_top();
    b.poke_guest(t - 0x400, b"Quarantine\0");
    let e = b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)).unwrap_err();
    assert!(e.starts_with("M47: unmodelled __mac_syscall policy \"Quarantine\" call 0x2"), "{e}");
    let t = sandbox(&mut b, b"file-read-data");
    let e = b.guest_mac_syscall(args(t - 0x400, 2, t - 0x300)).unwrap_err();
    assert!(e.starts_with("M47: unmeasured Sandbox operation \"file-read-data\""), "{e}");
}

/// R4: the host's answer about THIS process (the test binary, ad-hoc signed by the cargo runner, as
/// the recorder is). Bit 0 is `AMFI_DYLD_OUTPUT_ALLOW_AT_PATH`, the bit an `@rpath` load needs. The
/// whole word is host policy and is not asserted; the probe host answered 0x1df (amfi.out).
#[test]
fn the_host_answers_amfi_for_this_process_with_at_path_allowed() {
    let flags = retrace_box::host_amfi_dyld_policy(0).expect("AMFI's dyld policy for an ad-hoc process");
    assert_eq!(flags & 1, 1, "ALLOW_AT_PATH: {flags:#x}");
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-box --test macsyscall -- --test-threads=1 > $L/t3-red-box.log 2>&1; echo "exit=$?"
```

Expected: `exit=101`, `unresolved import retrace_box::MacSyscall`.

- [ ] **Step 4: The box methods**

In `crates/retrace-box/src/lib.rs`, directly after `host_svc` (the function ending `(ret, ret1, carry != 0)` and its closing `}`), add:

```rust
/// M47 §3d: AMFI's dyld policy for RETRACE's own process — `__mac_syscall("AMFI", 0x5a, {inFlags,
/// &out})` with every pointer host-owned, so the host kernel never sees a guest address. The record
/// arm writes the answer to the guest's `outFlags` and records it: the answer is host data, as
/// `task_info`'s audit token is (R4). `Err` is the host's errno. Record-only; replay applies the
/// recorded answer.
pub fn host_amfi_dyld_policy(in_flags: u64) -> Result<u64, u64> {
    #[repr(C)]
    struct AmfiArgs { in_flags: u64, out: *mut u64 }
    unsafe extern "C" {
        fn __mac_syscall(policy: *const std::ffi::c_char, call: i32, arg: *mut std::ffi::c_void) -> i32;
    }
    let mut out: u64 = 0;
    let mut a = AmfiArgs { in_flags, out: &mut out };
    // SAFETY: `a` and `out` outlive the call, and the policy name is a NUL-terminated literal.
    let r = unsafe {
        __mac_syscall(c"AMFI".as_ptr(), retrace_arch::AMFI_DYLD_POLICY_SELF as i32, (&mut a as *mut AmfiArgs).cast())
    };
    if r == 0 { Ok(out) } else { Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u64) }
}

/// What `Box_::guest_mac_syscall` found (M47 §3d).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacSyscall {
    /// `("AMFI", 0x5a)`: `arg` is `{u64 inFlags; u64 *outFlags}`; the answer lands at `out_ipa`.
    AmfiDyldPolicy { in_flags: u64, out_ipa: u64 },
    /// `("Sandbox", 2)`: the errno the pre-M47 forward returned for this operation (R7).
    SandboxCheck { errno: u64 },
}
```

Directly after `guest_madvise` (Task 2), add:

```rust
    /// M47: the NUL-terminated guest string at `va`, read the way the kernel's `copyinstr` reads it
    /// into a `cap`-byte buffer: at most `cap` bytes, NUL included, and an error if none of them is a
    /// NUL. Returned without the NUL.
    ///
    /// A shared-cache page the guest has not touched yet is paged in first (`page_in_cache`). A
    /// policy or operation name is a `__cstring` the code computed an address for without loading
    /// from it, so its page may be unstaged at the `svc` (t0 M2(c)). Paging it in is the same
    /// deterministic operation a guest load would have triggered, and both sides do it at the same
    /// landmark (symmetry rule 1: the record arm and the mirror call `guest_mac_syscall` alike).
    pub fn read_guest_cstr(&mut self, va: u64, cap: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut a = va;
        while out.len() < cap {
            let ipa = self.va_to_ipa(a).ok_or_else(|| format!("{a:#x} has no stage-1 translation"))?;
            let n = (((a | (GRANULE as u64 - 1)) + 1 - a) as usize).min(cap - out.len());
            let bytes = match self.read_guest_checked(ipa, n) {
                Some(b) => b,
                None if self.page_in_cache(ipa) => self.read_guest_checked(ipa, n)
                    .ok_or_else(|| format!("{a:#x} is unmapped after paging it in"))?,
                None => return Err(format!("{a:#x} (ipa {ipa:#x}) is not mapped")),
            };
            if let Some(z) = bytes.iter().position(|&c| c == 0) {
                out.extend_from_slice(&bytes[..z]);
                return Ok(out);
            }
            out.extend_from_slice(&bytes);
            a += n as u64;
        }
        Err(format!("no NUL within {cap} bytes of {va:#x}"))
    }

    /// M47 §3d: classify a `__mac_syscall` (381) from guest memory — `(policy, call)` through
    /// `retrace_arch::mac_syscall_model`, then the struct the pair's policy reads. Shared by the
    /// record arm and the replay mirror, which is what makes the classification symmetric.
    ///
    /// **Modelled, never forwarded.** A policy may write through a pointer inside `arg` (AMFI's
    /// `outFlags`), which a forward hands the host as a host address. AMFI's `outFlags` must be a
    /// mapped guest address for 8 bytes; natively an unmapped one is `EFAULT`, which the model
    /// refuses rather than guessing. Sandbox's operation name is at `*(arg + 16)`. Every other pair
    /// or operation is refused by value, naming it.
    pub fn guest_mac_syscall(&mut self, args: [u64; 8]) -> Result<MacSyscall, String> {
        let fail = |why: String| format!("{why}. args=[{}]", Self::fmt_args(args));
        let policy = self.read_guest_cstr(args[0], retrace_arch::MAC_MAX_POLICY_NAME)
            .map_err(|w| fail(format!("M47: __mac_syscall policy name: {w}")))?;
        match retrace_arch::mac_syscall_model(&policy, args[1] as u32).map_err(fail)? {
            retrace_arch::MacCall::AmfiDyldPolicy => {
                let a = self.read_va_prefix(args[2], 16);
                if a.len() != 16 {
                    return Err(fail(format!("M47: AMFI argument struct at {:#x} is not mapped for 16 bytes", args[2])));
                }
                let in_flags = u64::from_le_bytes(a[0..8].try_into().unwrap());
                let out = u64::from_le_bytes(a[8..16].try_into().unwrap());
                let out_ipa = self.va_to_ipa(out).filter(|&i| self.read_guest_checked(i, 8).is_some())
                    .ok_or_else(|| fail(format!("M47: AMFI outFlags {out:#x} is not mapped for 8 bytes")))?;
                Ok(MacSyscall::AmfiDyldPolicy { in_flags, out_ipa })
            }
            retrace_arch::MacCall::SandboxCheck => {
                let p = self.read_va_prefix(args[2].wrapping_add(16), 8);
                if p.len() != 8 {
                    return Err(fail(format!("M47: Sandbox argument struct at {:#x} is not mapped through +24", args[2])));
                }
                let op = self.read_guest_cstr(u64::from_le_bytes(p.try_into().unwrap()), retrace_arch::SANDBOX_OPERATION_MAX)
                    .map_err(|w| fail(format!("M47: Sandbox operation name: {w}")))?;
                let errno = retrace_arch::sandbox_check_continuity(&op).map_err(fail)?;
                Ok(MacSyscall::SandboxCheck { errno })
            }
        }
    }
```

The refusal strings the box tests match start with the `M47:` text, so `fail` appends `. args=[…]` after it and never prefixes anything.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-box --test macsyscall -- --test-threads=1 > $L/t3-box.log 2>&1; echo "exit=$?"
grep -a -E '^test |test result' $L/t3-box.log
```

Expected: `exit=0`, 5 passed. If `the_host_answers_amfi…` fails with an `Err`, report the errno and stop. The AMFI half of the design rests on that call (R4), and `amfi.out` measured it succeeding.

- [ ] **Step 5: The fixture and the failing e2e tests**

Create `crates/retrace-guest/c/librpath_dyn.c`:

```c
// M47 fixture (spec §3f): the dylib rpath_dyn loads through @rpath. Its install name is
// @rpath/librpath_dyn.dylib, so dyld expands @rpath to find it, which AMFI's dyld policy must allow.
int rpath_marker(void) { return 47; }
```

Create `crates/retrace-guest/c/rpath_dyn.c`:

```c
// M47 fixture (spec §3f): a guest that links a dylib by @rpath (LC_RPATH @executable_path). dyld
// refuses the load unless AMFI's dyld policy allows @-path expansion, so reaching main at all is the
// AMFI answer arriving; the marker proves the dylib's code ran.
#include <stdio.h>

int rpath_marker(void);

int main(void) {
    printf("rpath marker=%d\n", rpath_marker());
    return 0;
}
```

In `build.rs`, after the `madv_dyn` block:

```rust
    // rpath_dyn + librpath_dyn.dylib: the M47 AMFI fixture. The dylib's install name is @rpath/…
    // and the exe's LC_RPATH is @executable_path, so dyld expands @rpath only if AMFI's dyld policy
    // allows it. Both land in OUT_DIR side by side.
    let lib_src = format!("{}/c/librpath_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let lib = format!("{out}/librpath_dyn.dylib");
    println!("cargo:rerun-if-changed={lib_src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-dynamiclib","-install_name","@rpath/librpath_dyn.dylib","-o",&lib,&lib_src])
        .status().expect("clang librpath_dyn");
    assert!(status.success(), "librpath_dyn build failed");
    let src = format!("{}/c/rpath_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/rpath_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src,&lib,"-Wl,-rpath,@executable_path"])
        .status().expect("clang rpath_dyn");
    assert!(status.success(), "rpath_dyn guest build failed");
```

In `src/lib.rs`, after `MADV_DYN`:

```rust
/// M47: links `librpath_dyn.dylib` by `@rpath`, so it loads only if AMFI allows `@`-path expansion.
pub const RPATH_DYN: &str = concat!(env!("OUT_DIR"), "/rpath_dyn");
```

and after `madv_guest_parses`:

```rust
    #[test]
    fn rpath_guest_parses() {
        // M47: proves the build.rs wiring and the path constant; behaviour is gitprims_e2e's.
        let l = parse_macho(&std::fs::read(RPATH_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

Append to `gitprims_e2e.rs`:

```rust
/// Bit 0 of AMFI's dyld-policy answer, `AMFI_DYLD_OUTPUT_ALLOW_AT_PATH` (dyld's
/// `amfi_check_dyld_policy_self` output flags): the bit an `@rpath` load needs.
const ALLOW_AT_PATH: u64 = 1;

/// RED at `427fa0a`: dyld's `@`-path refusal, then `abort_with_payload` (521), which the recorder
/// asserts on. The difference: the dylib's marker, and the AMFI landmark's one recorded 8-byte
/// write with ALLOW_AT_PATH set. Every other `__mac_syscall` landmark must be a Sandbox check
/// answered by continuity (R7): an error with no writes.
#[test]
fn an_rpath_guest_loads_because_amfi_is_answered_by_the_host() {
    let (rec, trace) = records_and_replays_twice(retrace_guest::RPATH_DYN, &[]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "rpath marker=47\n");
    let mut amfi = 0;
    for e in retrace_trace::Reader::open(&trace).unwrap() {
        let Event::Syscall { num, args, ret, err, writes, .. } = e else { continue };
        if num != retrace_arch::SYS_MAC_SYSCALL { continue; }
        if args[1] as u32 == retrace_arch::AMFI_DYLD_POLICY_SELF {
            assert!(!err && ret == 0 && writes.len() == 1 && writes[0].bytes.len() == 8, "AMFI landmark: ret {ret} err {err} {writes:x?}");
            let flags = u64::from_le_bytes(writes[0].bytes[..].try_into().unwrap());
            assert_eq!(flags & ALLOW_AT_PATH, ALLOW_AT_PATH, "the recorded answer {flags:#x} must allow @-path expansion");
            amfi += 1;
        } else {
            assert_eq!(args[1] as u32, retrace_arch::SANDBOX_CHECK, "a __mac_syscall that is neither: {args:x?}");
            assert!(err && writes.is_empty() && (ret == 14 || ret == 22), "Sandbox landmark: ret {ret} err {err} {} writes", writes.len());
        }
    }
    assert_eq!(amfi, 1, "dyld asks AMFI once (t0 M2(a))");
}

/// Review Focus 4. AMFI's answer is applied, not recomputed (it is host data), so its ADDRESS is the
/// one thing replay can check: an answer recorded anywhere but the outFlags replay reads diverges.
#[test]
fn replay_refuses_an_amfi_answer_recorded_at_another_address() {
    let (_, trace) = records_and_replays_twice(retrace_guest::RPATH_DYN, &[]);
    let t = tamper(&trace, "amfi", |e| match e {
        Event::Syscall { num, args, writes, .. }
            if *num == retrace_arch::SYS_MAC_SYSCALL && args[1] as u32 == retrace_arch::AMFI_DYLD_POLICY_SELF => {
            writes[0].ipa += 8;
            true
        }
        _ => false,
    });
    let rp = util::replay(&t);
    assert_eq!(rp.code, 3, "a moved AMFI answer must be a divergence: {}", rp.stderr);
    assert!(rp.stderr.contains("__mac_syscall recorded ret=0x0 ret1=0x0 err=false with 1 write(s)"), "{}", rp.stderr);
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace --test gitprims_e2e --no-fail-fast -- --test-threads=1 > $L/t3-red-e2e.log 2>&1; echo "exit=$?"
grep -a -E '^test |@ path|521|writes through a nested|panicked at' $L/t3-red-e2e.log | head -12
```

Expected: `exit=101`. Both new tests FAIL at record.
- Neither `__mac_syscall` arm nor its mirror exists yet, so the new `NestedDest` kind sends 381 to the generic arm's `writes_via_nested_pointer` assert.
- That is a different RED from the base's 521, and a stronger one: it is this commit's structural guard firing.

Record the symptom. Then, for the base's RED, run `/private/tmp/claude-501/m47-base-retrace record-dyn <OUT_DIR>/rpath_dyn -o /private/tmp/claude-501/m47-rpath-base.bin` and quote dyld's `@ path` line and the 521 panic.

- [ ] **Step 6: The arm and the mirror**

In `crates/retrace-core/src/lib.rs`, directly after Task 2's madvise record arm, add:

```rust
            // M47 §3d: __mac_syscall is MODELLED per (policy, call), never forwarded: a policy may
            // write through a pointer INSIDE `arg` (AMFI's outFlags), which a forward hands the host
            // as a host address (NestedDest; the generic arm's assert refuses any other pair).
            // AMFI's answer is the host's, about retrace's own process (R4), asked with host-owned
            // pointers and recorded as the one 8-byte write at outFlags — the task_info posture. A
            // host error is recorded as that error with no write, which is what a native failure
            // looks like to dyld. Sandbox's call 2 keeps the errno the pre-M47 forward returned (R7).
            Stop::Syscall { num, args } if num == retrace_arch::SYS_MAC_SYSCALL => {
                let call = b.guest_mac_syscall(args).unwrap_or_else(|m| panic!("{m}"));
                let (ret, err, writes) = match call {
                    retrace_box::MacSyscall::AmfiDyldPolicy { in_flags, out_ipa } =>
                        match retrace_box::host_amfi_dyld_policy(in_flags) {
                            Ok(flags) => (0, false, vec![Region { ipa: out_ipa, bytes: flags.to_le_bytes().to_vec() }]),
                            Err(errno) => (errno, true, vec![]),
                        },
                    retrace_box::MacSyscall::SandboxCheck { errno } => (errno, true, vec![]),
                };
                w.append(&Event::Syscall { num, args, ret, ret1: 0, err, writes: writes.clone(), thread })
                    .map_err(|e| format!("append __mac_syscall: {e}"))?; count += 1;
                b.apply_and_return(ret, err, &writes);
            }
```

In `ReplaySession::advance`, directly after Task 2's madvise mirror, add:

```rust
                            // M47 §3d: the __mac_syscall arm's mirror (symmetry rule 1). The
                            // classification is recomputed from guest memory. AMFI's answer is host
                            // data, so the recorded write is applied rather than recomputed, once its
                            // address is checked against the outFlags replay reads; Sandbox's errno
                            // is recomputed and compared.
                            if num == retrace_arch::SYS_MAC_SYSCALL {
                                let call = match self.b.guest_mac_syscall(args) {
                                    Ok(c) => c,
                                    Err(m) => return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "__mac_syscall refused on replay, though the recording accepted it \
                                         — replay diverged before this landmark: {m}") }),
                                };
                                let ok = *ret1 == 0 && match call {
                                    retrace_box::MacSyscall::AmfiDyldPolicy { out_ipa, .. } =>
                                        (!*err && *ret == 0 && writes.len() == 1 && writes[0].ipa == out_ipa
                                            && writes[0].bytes.len() == 8)
                                        || (*err && writes.is_empty()),
                                    retrace_box::MacSyscall::SandboxCheck { errno } => *err && *ret == errno && writes.is_empty(),
                                };
                                if !ok {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "__mac_syscall recorded ret={ret:#x} ret1={ret1:#x} err={err} with {} write(s) {:x?}; \
                                         replay classifies it as {call:?}",
                                        writes.len(), writes.iter().map(|r| (r.ipa, r.bytes.len())).collect::<Vec<_>>()) });
                                }
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace --test gitprims_e2e --no-fail-fast -- --test-threads=1 > $L/t3-e2e.log 2>&1; echo "exit=$?"
grep -a -E '^test |test result' $L/t3-e2e.log
cargo test -p retrace-guest --lib -- --test-threads=1 rpath > $L/t3-guest.log 2>&1; echo "exit=$?"
for t in hello_dyn_e2e hello_rust_e2e jq_e2e cpython_e2e cpython_crash_e2e thread_oracle checkpoint_seek dispatch_e2e sysbin_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t3-$t.log 2>&1; echo "$t exit=$?"; done
grep -a -h -E 'test result|SKIPPED' $L/t3-*_e2e.log $L/t3-thread_oracle.log $L/t3-checkpoint_seek.log
```

Expected: every `exit=0`; `gitprims_e2e` 7 passed. Every dyld guest now runs with AMFI's real answer, not 0 (spec §3d's named consequence). A red here is that consequence moving a guest. Attribute it before changing anything, and report it.

- [ ] **Step 8: Controls, on the committed tree**

Commit first (Step 9), then:

1. **The structural guard.** Delete the `__mac_syscall` record arm. Run `an_rpath_guest_loads_because_amfi_is_answered_by_the_host`. Expected: RED at `syscall 381 writes through a nested guest pointer`.
2. **The address check.** In the mirror, delete `&& writes[0].ipa == out_ipa`. Run `replay_refuses_an_amfi_answer_recorded_at_another_address`. Expected: RED (replay exits 0, or fails elsewhere).

Restore with `git checkout -- crates/` after each, and record the symptoms.

- [ ] **Step 9: Clippy, then commit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo clippy --workspace --all-targets -- -D warnings > $L/t3-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace-arch crates/retrace-box crates/retrace-core crates/retrace-guest crates/retrace/tests/gitprims_e2e.rs
git commit -m "M47 t3: __mac_syscall modelled per (policy, call) — AMFI answered by the host into a host-owned slot (R4), Sandbox by continuity (R7), rows NestedDest"
```

---

### Task 4: The `fork` refusal and the prepare handler's message, test-first

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (`SYS_FORK` in the M47 section; row 2 beside row 59; `fork_refusal_errno` after `exec_refusal_errno`)
- Modify: `crates/retrace-arch/tests/census.rs`, `tests/gitshapes.rs`
- Modify: `crates/retrace-core/src/machmsg.rs` (route, decoder, 2 unit tests)
- Modify: `crates/retrace-core/src/lib.rs` (fork arm, mirror and assert; 3403 arm and mirror)
- Create: `crates/retrace-guest/c/forkfail_dyn.c`; modify `build.rs`, `src/lib.rs`
- Modify: `crates/retrace/tests/gitprims_e2e.rs`

**Interfaces:**
- Consumes: t0 M3(a)'s layout, M3(b)'s trap list (H1 cleared).
- Produces:
  - `retrace_arch::{SYS_FORK: u64 = 2, fork_refusal_errno(num: u64) -> Option<u64>}`;
  - `machmsg::{Route::ServicePortsRegister, decode_ports_register(buf: &[u8]) -> Result<(), String>}`;
  - `retrace_guest::FORKFAIL_DYN`.

- [ ] **Step 1: Write the failing validator and route tests**

Append to `gitshapes.rs`, adding `fork_refusal_errno` and `SYS_FORK` to the `use` line:

```rust
/// M47 §3e, R2: fork, and only fork, is refused, with the errno `fork(2)` documents for a process
/// limit reached. Its row exists for the census and says it takes no arguments.
#[test]
fn fork_alone_is_refused_with_eagain() {
    assert_eq!(define(&sdk_header("sys/syscall.h"), "SYS_fork"), Some(SYS_FORK as i64));
    assert_eq!(define(&sdk_header("sys/errno.h"), "EAGAIN"), Some(35));
    assert_eq!(fork_refusal_errno(SYS_FORK), Some(35));
    for n in (0..=1023u64).filter(|&n| n != SYS_FORK) { assert_eq!(fork_refusal_errno(n), None, "{n}"); }
    assert_eq!(arg_kinds(SYS_FORK).map(|s| (s.args.len(), s.ret)), Some((0, Ret::Plain)));
}
```

In `crates/retrace-core/src/machmsg.rs`'s test module, after `decode_set_special_port_rejects_malformed`, add:

```rust
    // --- mach_ports_register (3403) — M47 ---

    /// 3403 to the guest task port is libxpc's `xpc_atfork_prepare`, in fork's prepare handlers.
    #[test]
    fn routes_mach_ports_register_to_the_task_port_only() {
        assert!(matches!(route(&msg(3403, 0x203, KOBJ), Some(0x203)), Route::ServicePortsRegister));
        assert!(matches!(route(&msg(3403, 0x999, KOBJ), Some(0x203)), Route::Unsupported(_)));
    }

    /// Hand-built from t0 M3(a)'s 64 bytes: header(24, COMPLEX) + descriptor count(4) = 3 + three
    /// port descriptors(12 each), type byte at descriptor offset 11 = 0 (MACH_MSG_PORT_DESCRIPTOR).
    fn ports_register_req() -> Vec<u8> {
        let mut b = vec![0u8; 64];
        b[0..4].copy_from_slice(&MACH_MSGH_BITS_COMPLEX.to_le_bytes());
        b[4..8].copy_from_slice(&64u32.to_le_bytes());
        b[20..24].copy_from_slice(&3403u32.to_le_bytes());
        b[24..28].copy_from_slice(&3u32.to_le_bytes());
        for i in 0..3 { b[28 + 12 * i + 10] = 19; } // disposition COPY_SEND; type 0
        b
    }
    #[test]
    fn decodes_ports_register_and_rejects_malformed() {
        assert_eq!(decode_ports_register(&ports_register_req()), Ok(()));
        assert!(decode_ports_register(&ports_register_req()[..63]).is_err(), "short");
        let mut b = ports_register_req(); b.push(0);
        assert!(decode_ports_register(&b).is_err(), "long");
        let mut b = ports_register_req(); b[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(decode_ports_register(&b).is_err(), "not COMPLEX");
        let mut b = ports_register_req(); b[20..24].copy_from_slice(&3404u32.to_le_bytes());
        assert!(decode_ports_register(&b).is_err(), "wrong id");
        let mut b = ports_register_req(); b[24..28].copy_from_slice(&2u32.to_le_bytes());
        assert!(decode_ports_register(&b).is_err(), "two descriptors");
        let mut b = ports_register_req(); b[28 + 12 + 11] = 1;
        assert!(decode_ports_register(&b).is_err(), "an OOL descriptor");
    }
```

If t0 M3(a) measured a different disposition, write that value in the test's comment and loop. If it measured a different layout, change the helper and the decoder to match it.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-arch --test gitshapes -- --test-threads=1 > $L/t4-red-arch.log 2>&1; echo "exit=$?"
cargo test -p retrace-core --lib -- --test-threads=1 ports_register > $L/t4-red-core.log 2>&1; echo "exit=$?"
```

Expected: both `exit=101`, compile errors for `fork_refusal_errno`/`SYS_FORK` and for `ServicePortsRegister`/`decode_ports_register`.

- [ ] **Step 2: The validator, the row and the census**

In `crates/retrace-arch/src/lib.rs`'s M47 section, after `SYS_MADVISE`, add:

```rust
/// `fork` (SDK `SYS_fork 2`). Refused since M47, never forwarded (`fork_refusal_errno`).
pub const SYS_FORK: u64 = 2;
```

Directly after `exec_refusal_errno`'s closing `}`, add:

```rust
/// M47 §3e: `fork` (2) is REFUSED, never forwarded, with `EAGAIN` (35), the errno `fork(2)`
/// documents for a process limit reached, so the guest takes a failure path libc and its callers
/// already have (R2): libc's `fork` calls `cerror`, then its parent handlers, and returns −1
/// (the M47 probe's fork-disasm.txt). Fidelity, not continuity: before M47 a fork never returned,
/// because its prepare handler's `mach_ports_register` (3403) stopped the recorder first. `vfork`
/// (66) and the other process-creation calls have no row. `Some` doubles as the predicate the record
/// arm, the replay mirror and the generic arm's assert share, as `exec_refusal_errno`'s does.
pub fn fork_refusal_errno(num: u64) -> Option<u64> {
    match num { SYS_FORK => Some(35), _ => None }
}
```

Directly before the `59 => row!(P, [Path, NestedSource, NestedSource]),` row's comment block (`// execve(char *fname, …`), add:

```rust
        // fork(void): REFUSED since M47, never forwarded. The record arm ahead of the generic
        // forward answers `fork_refusal_errno` (EAGAIN) and writes nothing, on both sides, and the
        // generic arm asserts it never arrives: this row makes `forwarded_shape` accept 2, so the
        // assert is what stands between a missing arm and a real child of retrace. Documentation of
        // the prototype only, as 59's and 244's are.
        2 => row!(P, []),
```

In `census.rs`, extend the M47 paragraph with:

```rust
//! M47 also adds 2 `fork`, reached by `forkfail_dyn`, by default-config `git commit` and by
//! `/bin/csh`/`/bin/tcsh` once the prepare handler's `mach_ports_register` (3403) is answered
//! (M47 t0 M3); it is refused, never forwarded.
```

In `CENSUS`, change `-10, 1, 3, 4,` to `-10, 1, 2, 3, 4,` and re-wrap the lines if they pass 100 columns.

- [ ] **Step 3: The route and the decoder**

In `machmsg.rs`, add `ServicePortsRegister` to `Route` (after `ServiceSetSpecialPort`):

```rust
pub enum Route { ServiceVmMap, ServiceVmRemap, ServiceGetSpecialPort, ServiceSetSpecialPort, ServicePortsRegister,
                 StubMigReply(i32), RefuseMqSend, RefuseMqRecv, Forward(&'static str), Unsupported(String) }
```

In `route()`, after the `3410 => return Route::ServiceSetSpecialPort,` arm, add:

```rust
            // mach_ports_register (task subsystem base 3400, slot 3): libxpc's xpc_atfork_prepare,
            // in fork's prepare handlers (M47 t0 M3(a); backtrace in apple_walls_e2e's csh/tcsh
            // reasons). It sets the port array a CHILD would inherit; M47 refuses the fork, so no
            // child exists and the parent observes only the reply. Answered with a mig_reply_error
            // KERN_SUCCESS — never forwarded (that would register retrace's own). Decoded and
            // checked by value in dispatch.
            3403 => return Route::ServicePortsRegister,
```

After `decode_set_special_port`, add:

```rust
/// mach_ports_register (3403) request, as libxpc's `xpc_atfork_prepare` sends it before `fork`
/// (M47 t0 M3(a)): a COMPLEX message of header(24) + descriptor count(4) + three
/// mach_msg_port_descriptor_t (12 each, type byte at offset 11) = 64 bytes, with no NDR and no inline
/// data. Checked by value: the length, the COMPLEX bit, the id, the count and each descriptor's type.
/// The names and dispositions are the guest's and are not modelled: the registration sets what a
/// child would inherit, and M47 never creates one (§3e).
pub fn decode_ports_register(buf: &[u8]) -> Result<(), String> {
    if buf.len() != 64 { return Err(format!("mach_ports_register request is {} bytes, measured 64", buf.len())); }
    if u32_at(buf, 0) & MACH_MSGH_BITS_COMPLEX == 0 { return Err("mach_ports_register request is not COMPLEX".into()); }
    let id = u32_at(buf, 20);
    if id != 3403 { return Err(format!("msgh_id {id} != 3403")); }
    let n = u32_at(buf, 24);
    if n != 3 { return Err(format!("descriptor count {n}, measured 3")); }
    for i in 0..3 {
        let ty = buf[28 + 12 * i + 11];
        if ty != 0 { return Err(format!("descriptor {i} has type {ty}, measured MACH_MSG_PORT_DESCRIPTOR (0)")); }
    }
    Ok(())
}
```

If t0 M3(a) found a `MOVE_*` disposition, append this sentence to that doc: `The descriptors carry MOVE_SEND (t0 M3(a)); natively the send moves the right, so the model leaves one extra user reference per name in retrace's own IPC space.`

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-arch --test gitshapes -- --test-threads=1 > $L/t4-arch.log 2>&1; echo "exit=$?"
cargo build -p retrace-core > $L/t4-core-build.log 2>&1; echo "exit=$?"; grep -a -E 'error\[E0004\]' -A3 $L/t4-core-build.log | head -12
```

Expected: `gitshapes` `exit=0` (10 passed). `retrace-core` fails to build with `E0004` non-exhaustive patterns: `Route::ServicePortsRegister` not covered, in `record_box` and in `ReplaySession::advance`. Step 5 fills both.

- [ ] **Step 4: The fixture and its failing e2e test**

Create `crates/retrace-guest/c/forkfail_dyn.c`:

```c
// M47 fixture (spec §3f): fork() must fail with EAGAIN. Natively the fixture first lowers its own
// RLIMIT_NPROC to 1, so the kernel refuses the fork as it refuses one past the process limit (t0
// measured this under lldb); under retrace the fork never reaches the kernel (M47 §3e). A child, if
// one is ever created, exits at once without printing.
#include <errno.h>
#include <stdio.h>
#include <sys/resource.h>
#include <unistd.h>

int main(void) {
    struct rlimit rl = { 1, 1 };
    if (setrlimit(RLIMIT_NPROC, &rl) != 0) { perror("setrlimit"); return 1; }
    pid_t p = fork();
    if (p == 0) _exit(0);
    if (p < 0) { printf("fork failed errno=%d\n", errno); return 0; }
    printf("fork succeeded\n");
    return 1;
}
```

In `build.rs`, after the `rpath_dyn` block:

```rust
    // forkfail_dyn: the M47 fork fixture — fork() with RLIMIT_NPROC lowered to 1; prints the errno.
    // Same recipe as hello_dyn.
    let src = format!("{}/c/forkfail_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/forkfail_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang forkfail_dyn");
    assert!(status.success(), "forkfail_dyn guest build failed");
```

In `src/lib.rs`, after `RPATH_DYN`:

```rust
/// M47: `fork()` with `RLIMIT_NPROC` lowered to 1; prints the errno the fork failed with.
pub const FORKFAIL_DYN: &str = concat!(env!("OUT_DIR"), "/forkfail_dyn");
```

and after `rpath_guest_parses`:

```rust
    #[test]
    fn forkfail_guest_parses() {
        // M47: proves the build.rs wiring and the path constant; behaviour is gitprims_e2e's.
        let l = parse_macho(&std::fs::read(FORKFAIL_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

Append to `gitprims_e2e.rs`:

```rust
/// RED at `427fa0a`: `RECORD ERROR: unsupported mach_msg2 … msgh_id 3403`, the prepare handler's
/// message. The difference: the guest's own errno line, which only a fork that RETURNED can print;
/// the recorder's refusal line, which a forwarded fork cannot fake; and in the trace, one refused
/// fork and one answered 3403 (its 3503 reply).
#[test]
fn fork_is_refused_with_eagain_after_the_prepare_handlers_message_is_answered() {
    let (rec, trace) = records_and_replays_twice(retrace_guest::FORKFAIL_DYN, &[]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), "fork failed errno=35\n");
    assert!(rec.stderr.contains("[retrace] refusing fork (syscall 2): process creation is unmodelled; returning errno 35 without forwarding"),
        "the recorder's refusal line: {}", rec.stderr);
    let ev = retrace_trace::Reader::open(&trace).unwrap();
    let forks: Vec<_> = ev.iter().filter_map(|e| match e {
        Event::Syscall { num, ret, err, writes, .. } if *num == retrace_arch::SYS_FORK => Some((*ret, *err, writes.len())),
        _ => None,
    }).collect();
    assert_eq!(forks, vec![(35, true, 0)], "one refused fork, no writes");
    const MACH_MSG2: u64 = -47i64 as u64;
    let replies = ev.iter().filter(|e| matches!(e, Event::Syscall { num, writes, .. } if *num == MACH_MSG2
        && writes.iter().any(|r| r.bytes.len() >= 24 && u32::from_le_bytes(r.bytes[20..24].try_into().unwrap()) == 3503))).count();
    assert_eq!(replies, 1, "the prepare handler's mach_ports_register (3403) answered once, with its 3503 reply");
}
```

- [ ] **Step 5: The arms, the mirrors and the assert**

In `crates/retrace-core/src/lib.rs`'s record mach_msg2 match, directly after the `machmsg::Route::ServiceSetSpecialPort => { … }` arm, add:

```rust
                    machmsg::Route::ServicePortsRegister => {
                        // M47 §3e: mach_ports_register (3403), libxpc's xpc_atfork_prepare in fork's
                        // prepare handlers. It sets the port array a CHILD would inherit; the fork is
                        // refused, so no child exists and the parent observes only the reply. A
                        // mig_reply_error KERN_SUCCESS, never forwarded (it would register retrace's
                        // own). Deterministic → the standard symmetric posture: replay recomputes and
                        // byte-compares (the 3410 shape).
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        machmsg::decode_ports_register(&buf)
                            .unwrap_or_else(|e| panic!("mach_ports_register (3403) decode: {e}"));
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_mig_error(m.msgh_id, m.reply_port, machmsg::KERN_SUCCESS) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS, ret1: 0,
                            err: false, writes: writes.clone(), thread })
                            .map_err(|e| format!("append mach_msg2 ports_register: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
```

In the replay mach_msg2 match, directly after the `machmsg::Route::ServiceSetSpecialPort => { … }` mirror, add:

```rust
                                    machmsg::Route::ServicePortsRegister => {
                                        // M47 §3e: deterministic mig_reply_error → the standard
                                        // symmetric posture, as ServiceSetSpecialPort above.
                                        let buf = self.b.read_guest(m.data, m.send_size as usize);
                                        machmsg::decode_ports_register(&buf).map_err(|e| Divergence {
                                            landmark: self.idx, pc, detail: format!("replay mach_ports_register decode: {e}") })?;
                                        let reply = machmsg::encode_mig_error(m.msgh_id, m.reply_port, machmsg::KERN_SUCCESS);
                                        if writes.len() != 1 || writes[0].bytes != reply {
                                            return Err(Divergence { landmark: self.idx, pc,
                                                detail: "mach_ports_register reply mismatch".into() });
                                        }
                                        self.b.apply_and_return(*ret, *err, writes);
                                    }
```

Directly after the M38 exec-refusal **record** arm, and before Task 2's madvise arm, add:

```rust
            // M47 §3e: fork is refused, never forwarded — a forwarded fork would start a real child
            // of retrace. The exec arm's posture (M38): constant return, no writes; replay
            // recomputes and compares. EAGAIN is fork's documented process-limit failure (R2), and
            // libc's fork then runs its parent handlers and returns -1, a path it already has.
            Stop::Syscall { num, args } if retrace_arch::fork_refusal_errno(num).is_some() => {
                let e = retrace_arch::fork_refusal_errno(num).unwrap();
                eprintln!("[retrace] refusing fork (syscall {num}): process creation is unmodelled; returning errno {e} without forwarding");
                w.append(&Event::Syscall { num, args, ret: e, ret1: 0, err: true, writes: vec![], thread })
                    .map_err(|e| format!("append fork refusal: {e}"))?; count += 1;
                b.apply_and_return(e, true, &[]);
            }
```

In the generic arm, directly after Task 2's madvise assert:

```rust
                // M47 §3e: fork's row exists for the census and the views, which makes
                // `forwarded_shape` accept it; this assert is what keeps a missing refusal arm from
                // forwarding it and starting a real child of the recorder.
                assert!(retrace_arch::fork_refusal_errno(num).is_none(),
                    "fork ({num}) reached the generic forward arm — it must be refused above (M47). \
                     Forwarded, it starts a real child of the recorder.");
```

In `ReplaySession::advance`, directly after the M38 exec mirror and before Task 2's madvise mirror:

```rust
                            // M47 mirror of record's fork refusal: recompute the constant, compare.
                            if let Some(e) = retrace_arch::fork_refusal_errno(num) {
                                if *ret != e || !*err || *ret1 != 0 || !writes.is_empty() {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "fork refusal mismatch: recorded ret {ret} ret1 {ret1} err {err} with {} write(s), \
                                         expected errno {e}, err, no writes", writes.len()) });
                                }
                                self.b.apply_and_return(*ret, *err, writes);
                                return self.finish_event();
                            }
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace-core --lib -- --test-threads=1 > $L/t4-core.log 2>&1; echo "exit=$?"
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t4-arch-all.log 2>&1; echo "exit=$?"
cargo test -p retrace-guest --lib -- --test-threads=1 forkfail > $L/t4-guest.log 2>&1; echo "exit=$?"
cargo test -p retrace --test gitprims_e2e --no-fail-fast -- --test-threads=1 > $L/t4-e2e.log 2>&1; echo "exit=$?"
grep -a -E '^test |test result' $L/t4-e2e.log
cargo test -p retrace --test exec_e2e -- --test-threads=1 > $L/t4-exec.log 2>&1; echo "exit=$?"
```

Expected: every `exit=0`; `gitprims_e2e` 8 passed; `exec_e2e` green beside the new arm. If `forkfail` fails past the refusal, record the first unmodelled trap. If that trap is not one M3(b) listed, it is **H1**.

- [ ] **Step 7: Controls, on the committed tree**

Commit first (Step 8), then:

1. **RED at base.** Run `/private/tmp/claude-501/m47-base-retrace record-dyn <OUT_DIR>/forkfail_dyn -o /private/tmp/claude-501/m47-ff-base.bin` and quote the `RECORD ERROR … msgh_id 3403` line.
2. **The generic-arm assert.** Delete the fork record arm. Run the forkfail test. Expected: RED at `fork (2) reached the generic forward arm`. **This control must be run exactly as written:** the assert is what prevents a real fork. If the assert were missing, this control would fork the recorder.

Restore with `git checkout -- crates/`, and record the symptoms.

- [ ] **Step 8: Clippy, then commit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo clippy --workspace --all-targets -- -D warnings > $L/t4-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace-arch crates/retrace-core crates/retrace-guest crates/retrace/tests/gitprims_e2e.rs
git commit -m "M47 t4: fork refused with EAGAIN (R2) behind a generic-arm assert; mach_ports_register (3403) answered KERN_SUCCESS"
```

---

### Task 5: `git_e2e` and `node_e2e`

**Files:**
- Create: `crates/retrace/tests/git_e2e.rs`, `crates/retrace/tests/node_e2e.rs`

**Interfaces:**
- Consumes: Tasks 1–4 landed; t0 M3(c)'s `CANNOT_FORK`; t0 M4's read list and write in-list; t0 M1(b)'s rate.
- Produces: the gate list Task 7 writes into CLAUDE.md.

- [ ] **Step 1: RED at base, measured**

The base binary panics at `chdir` for every `-C` run, so every write command is RED:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
export G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
export D=/private/tmp/claude-501/m47-t5-red
rm -rf $D && mkdir -p $D && env -i $G -C $D init -q -b main && echo x > $D/c.txt
/private/tmp/claude-501/m47-base-retrace record-dyn $G -o /private/tmp/claude-501/m47-t5-red.bin -- -C $D add c.txt > $L/t5-red.out 2> $L/t5-red.err; echo "rc=$?"
grep -a -m1 -E 'M33:|panicked' $L/t5-red.err
```

Expected: rc 101, `M33: syscall 12 (12) has no arg_kinds row`.

- [ ] **Step 2: Write `git_e2e.rs`**

```rust
//! M47 gate (spec §1 part 1, §3f): git's local workflow, recorded and replayed. Xcode's git, because
//! `/usr/bin/git` is an xcrun shim that reaches M38's posix_spawn refusal. Every test builds a fresh
//! repository NATIVELY first, with the same empty environment the guest runs under (`load_dynamic`
//! pushes an empty envp), so native git and recorded git read the same configuration: none. Every
//! git invocation names its repository with `-C`, which is the chdir (12) row at work (R1).
//!
//! What each asserts is the difference M47 makes, never an exit code alone:
//! - reads: stdout byte-equal to native, and two replays byte-equal to the recording;
//! - writes: the repository state native git reads back afterwards;
//! - commit under default config: the refused maintenance fork (the recorder's line and git's own
//!   error line) and the tree a native twin commit writes. The commit ID differs from the twin's by
//!   design: the guest's timestamps are the host's clock, recorded (spec §2a).
//!
//! The corruption M47 closes (a forwarded MADV_FREE_REUSABLE, spec §2c) aborted a commit in t0
//! M1(b)'s forwarded runs at the rate the measurements file gives; the commit test is that class's
//! guard, as strong as that rate.
//!
//! NOT a repo artifact: without Xcode's git each test announces a skip and passes, which is not
//! evidence of anything.
mod util;
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT: &str = "/Applications/Xcode.app/Contents/Developer/usr/bin/git";
const IDENT: [&str; 4] = ["-c", "user.name=retrace", "-c", "user.email=retrace@example.invalid"];
/// t0 M3(c): git's own stderr line when its auto-maintenance fork fails with EAGAIN, natively.
const CANNOT_FORK: &str = "error: cannot fork() for maintenance: Resource temporarily unavailable";

fn have_git(test: &str) -> bool {
    if Path::new(GIT).exists() { return true; }
    util::announce(&format!("SKIPPED {test}: {GIT} not installed (Xcode). \
        This gate did NOT run — it is not evidence of anything."));
    false
}

/// Native git in `repo`, with the guest's empty environment. Asserts success.
fn native(repo: &Path, args: &[&str]) -> Vec<u8> {
    let o = Command::new(GIT).env_clear().arg("-C").arg(repo).args(args).output().unwrap();
    assert!(o.status.success(), "native git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

/// A fresh repository on `main` with one commit (`a.txt`), then `a.txt` modified and `b.txt` and
/// `c.txt` untracked, so status, diff and add have something to act on. `topic` adds a branch whose
/// one commit (`t.txt`) is ahead of `main`, for a fast-forward merge.
fn repo(tag: &str, topic: bool) -> PathBuf {
    let d = std::env::temp_dir().join(format!("retrace-m47-git-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let commit = |d: &Path, msg: &str| {
        let mut a = IDENT.to_vec();
        a.extend(["-c", "maintenance.auto=false", "commit", "-q", "-m", msg]);
        native(d, &a);
    };
    native(&d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\n").unwrap();
    native(&d, &["add", "a.txt"]);
    commit(&d, "first");
    if topic {
        native(&d, &["switch", "-q", "-c", "topic"]);
        std::fs::write(d.join("t.txt"), "t\n").unwrap();
        native(&d, &["add", "t.txt"]);
        commit(&d, "topic");
        native(&d, &["switch", "-q", "main"]);
    }
    std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(d.join("b.txt"), "untracked\n").unwrap();
    std::fs::write(d.join("c.txt"), "added\n").unwrap();
    d
}

/// Record git in `repo` with `args`, then replay twice; both replays must match the recording.
fn recorded(repo: &Path, args: &[&str]) -> util::RunOut {
    let mut argv = vec!["-C", repo.to_str().unwrap()];
    argv.extend_from_slice(args);
    let (rec, trace) = util::record_dynamic_args(GIT, &argv);
    assert_eq!(rec.code, 0, "record git {args:?}: {}", rec.stderr);
    for i in 0..2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "replay {i} of git {args:?}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} of git {args:?}: stdout differs from the recording");
    }
    let _ = std::fs::remove_file(&trace);
    rec
}

/// t0 M4's in-list read commands (the measurements file, Decisions).
const READS: &[&[&str]] = &[
    &["status", "--porcelain"],
    &["status"],
    &["diff"],
    &["diff", "--cached"],
    &["log", "-1"],
    &["show", "--stat", "HEAD"],
    &["rev-parse", "HEAD"],
];

/// RED at `427fa0a`: the M33 panic at chdir (12). Native runs first on the same repository, so both
/// read the same objects and the same index.
#[test]
fn every_read_command_matches_native_and_replays_identically() {
    if !have_git("every_read_command_matches_native_and_replays_identically") { return; }
    let d = repo("reads", false);
    for args in READS {
        let want = native(&d, args);
        let rec = recorded(&d, args);
        assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&want),
            "git {args:?}: the recorded stdout differs from native");
    }
}

/// RED at `427fa0a`: the M33 panic at chdir (12), then mkdir (136) and link (9). `hash-object`
/// without `-w` names the blob without writing it, so the object existing afterwards is the guest's
/// write.
#[test]
fn add_writes_the_blob_native_git_reads_back() {
    if !have_git("add_writes_the_blob_native_git_reads_back") { return; }
    let d = repo("add", false);
    let blob = String::from_utf8(native(&d, &["hash-object", "c.txt"])).unwrap().trim().to_string();
    recorded(&d, &["add", "c.txt"]);
    assert_eq!(String::from_utf8(native(&d, &["ls-files", "--stage", "c.txt"])).unwrap(), format!("100644 {blob} 0\tc.txt\n"));
    assert_eq!(native(&d, &["cat-file", "-t", &blob]), b"blob\n");
}

/// RED at `427fa0a`: the M33 panic at chdir (12). Past the rows, the forwarded MADV_FREE_REUSABLE
/// corrupted the heap (t0 M1(b)), and default config reached the 3403 record error. The difference:
/// the refused fork (the recorder's line and git's own line), a clean record, and the tree a native
/// commit of the same index writes.
#[test]
fn commit_under_default_config_refuses_the_maintenance_fork_and_writes_the_native_tree() {
    if !have_git("commit_under_default_config_refuses_the_maintenance_fork_and_writes_the_native_tree") { return; }
    let (d, twin) = (repo("commit", false), repo("commit-twin", false));
    for r in [&d, &twin] { native(r, &["add", "-A"]); }
    let mut args = IDENT.to_vec();
    args.extend(["commit", "-q", "-m", "second"]);
    let rec = recorded(&d, &args);
    assert!(rec.stderr.contains("[retrace] refusing fork (syscall 2)"), "the recorder's refusal line: {}", rec.stderr);
    assert!(rec.stderr.contains(CANNOT_FORK), "git's own line when its fork fails (t0 M3(c)): {}", rec.stderr);
    let mut twin_args = vec!["-c", "maintenance.auto=false"];
    twin_args.extend(&args);
    native(&twin, &twin_args);
    assert_eq!(native(&d, &["log", "-1", "--format=%T %s"]), native(&twin, &["log", "-1", "--format=%T %s"]),
        "the recorded commit must carry the tree and subject a native commit of the same index writes");
}
```

If t0 M3(c)'s rc was non-zero, `recorded`'s `rec.code == 0` assertion is wrong for commit. Give `commit…` its own record call that asserts the measured rc instead, and say so. If H1 fell back, the commit test passes `-c maintenance.auto=false`, drops both stderr assertions, and its doc says default-config commit is a documented limit.

Then add one test per t0 M4 in-list write command beyond `add` and `commit`. Delete any command below that t0 put on the out-list; its wall is in the measurements file. Each test starts with its own `have_git` guard:

```rust
#[test]
fn branch_and_tag_point_at_head() {
    if !have_git("branch_and_tag_point_at_head") { return; }
    let d = repo("branch", false);
    recorded(&d, &["branch", "topic"]);
    recorded(&d, &["tag", "v1"]);
    let head = native(&d, &["rev-parse", "HEAD"]);
    assert_eq!(native(&d, &["rev-parse", "topic"]), head);
    assert_eq!(native(&d, &["rev-parse", "v1^{commit}"]), head);
}

#[test]
fn switch_c_moves_head_to_a_new_branch() {
    if !have_git("switch_c_moves_head_to_a_new_branch") { return; }
    let d = repo("switch", false);
    recorded(&d, &["switch", "-q", "-c", "topic"]);
    assert_eq!(native(&d, &["symbolic-ref", "HEAD"]), b"refs/heads/topic\n");
}

#[test]
fn mv_renames_the_index_entry_and_the_file() {
    if !have_git("mv_renames_the_index_entry_and_the_file") { return; }
    let d = repo("mv", false);
    recorded(&d, &["mv", "a.txt", "moved.txt"]);
    assert_eq!(native(&d, &["ls-files", "a.txt", "moved.txt"]), b"moved.txt\n");
    assert!(d.join("moved.txt").exists() && !d.join("a.txt").exists());
}

#[test]
fn rm_cached_drops_the_index_entry_and_keeps_the_file() {
    if !have_git("rm_cached_drops_the_index_entry_and_keeps_the_file") { return; }
    let d = repo("rm", false);
    recorded(&d, &["rm", "-q", "--cached", "a.txt"]);
    assert_eq!(native(&d, &["ls-files", "a.txt"]), b"");
    assert!(d.join("a.txt").exists());
}

#[test]
fn stash_saves_the_change_and_cleans_the_tracked_file() {
    if !have_git("stash_saves_the_change_and_cleans_the_tracked_file") { return; }
    let d = repo("stash", false);
    let mut args = IDENT.to_vec();
    args.extend(["stash", "-q"]);
    recorded(&d, &args);
    // The subject carries an abbreviated hash, which differs per repository: assert around it.
    let list = String::from_utf8(native(&d, &["stash", "list", "--format=%s"])).unwrap();
    assert!(list.starts_with("WIP on main: ") && list.ends_with(" first\n") && list.lines().count() == 1, "{list:?}");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "one\n", "the tracked change is stashed away");
}

#[test]
fn merge_ff_only_moves_head_to_topic() {
    if !have_git("merge_ff_only_moves_head_to_topic") { return; }
    let d = repo("merge", true);
    let mut args = IDENT.to_vec();
    args.extend(["merge", "-q", "--ff-only", "topic"]);
    recorded(&d, &args);
    assert_eq!(native(&d, &["rev-parse", "HEAD"]), native(&d, &["rev-parse", "topic"]));
}
```

Check the stash test's expected prefix and suffix against t0 M4's `docs/sweep-evidence/2026-09-30-m47-t0/m4/stash.native.state`. If native git words the subject differently, use native's words.

`merge` auto-maintains, so under default config it meets the refused fork like `commit` does. Its stderr then carries `CANNOT_FORK` too, if t0 M4 ran it under default config. The test asserts only the ref, which is the merge's difference.

- [ ] **Step 3: Write `node_e2e.rs`**

```rust
//! M47 (spec §1 part 5, §3g): node, parked at its measured wall. Before M47 dyld refused node's
//! `@rpath/libnode.*.dylib` because AMFI's dyld policy read as 0 under retrace (the forwarded
//! `__mac_syscall` EFAULTed); M47 answers it from the host (R4), and node now loads every dylib and
//! runs to the wall below. The Cellar path, not the `/opt/homebrew/bin` symlink, is what the probe
//! and the walk measured.
//!
//! NOT a repo artifact: without Homebrew's node the test announces a skip, even when run with
//! `--ignored`.
mod util;

const NODE: &str = "/opt/homebrew/bin/node";

#[test]
#[ignore = "M47 wall, class C (new subsystem: kevent on a guest kqueue), parked, not routed. /opt/homebrew/bin/node -e 'console.log(1)': its @rpath dylibs load since M47 answers AMFI's dyld policy from the host; the run then stops at kevent (363), which has no arg_kinds row, on the descriptor kqueue (362) returned — libuv's loop — after 1,101 traps, with no thread created and no MAP_JIT mapping (the 2026-09-30 probe, docs/sweep-evidence/2026-09-30-m47-probe/node-kevent.txt; re-measured by M47 Task 6). UN-IGNORE when kevent on a guest kqueue is modelled."]
fn node_prints_one_and_replays() {
    if !std::path::Path::new(NODE).exists() {
        util::announce(&format!("SKIPPED node_prints_one_and_replays: {NODE} not installed (`brew install node`). \
            This gate did NOT run — it is not evidence of anything."));
        return;
    }
    let exe = std::fs::canonicalize(NODE).unwrap();
    let out = util::assert_rung_records_and_replays(exe.to_str().unwrap(), &["-e", "console.log(1)"], b"1\n");
    assert_eq!(out.stdout, b"1\n");
}
```

- [ ] **Step 4: Run them**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo test -p retrace --test git_e2e --no-fail-fast -- --test-threads=1 > $L/t5-git.log 2>&1; echo "exit=$?"
grep -a -E '^test |test result|SKIPPED' $L/t5-git.log
cargo test -p retrace --test node_e2e --no-fail-fast -- --test-threads=1 > $L/t5-node.log 2>&1; echo "exit=$?"
grep -a -E 'test result' $L/t5-node.log
cargo test -p retrace --test node_e2e -- --ignored --test-threads=1 > $L/t5-node-ignored.log 2>&1; echo "exit=$?"
grep -a -E 'panicked|M33:|RECORD ERROR|SKIPPED' $L/t5-node-ignored.log | head -5
```

Expected:
- `git_e2e`: `exit=0`, 3 + k passed, no `SKIPPED` line (Xcode is present).
- `node_e2e`: `exit=0`, 0 passed / 1 ignored.
- `--ignored`: `exit=101`, failing at the wall. Quote its line; Task 6 measures it properly.

- [ ] **Step 5: Clippy, then commit**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cargo clippy --workspace --all-targets -- -D warnings > $L/t5-clippy.log 2>&1; echo "exit=$?"
git add crates/retrace/tests/git_e2e.rs crates/retrace/tests/node_e2e.rs
git commit -m "M47 t5: git_e2e — reads against native, add, default-config commit through the refused fork; node_e2e parked at kevent"
```

---

### Task 6: The walk (controller-run for the sweep)

**Files:**
- Modify: `crates/retrace/tests/node_e2e.rs` (the reason, from the measurement)
- Modify: `crates/retrace/tests/apple_walls_e2e.rs` (`csh`, `tcsh`)
- Create: `docs/sweep-evidence/2026-09-30-m47/` (`README.md`, the walk's stderr, the sweep log, `rowdiff.txt`)

**Interfaces:**
- Consumes: Tasks 1–5 landed; the base binary from t0.
- Produces: node's measured wall; `csh`/`tcsh`'s new state; the sweep tally and row diff Task 7 writes.

- [ ] **Step 1: node to its wall**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/docs/sweep-evidence/2026-09-30-m47
mkdir -p $E
cargo build -p retrace > $L/t6-build.log 2>&1; echo "exit=$?"
export RETRACE_TRACE=1
export NODE=$(python3 -c 'import os; print(os.path.realpath("/opt/homebrew/bin/node"))')
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $NODE -o /private/tmp/claude-501/m47-node.bin -- -e 'console.log(1)' > $E/node.rec.out 2> $E/node.rec.err; echo "record exit=$?"
unset RETRACE_TRACE
grep -a -c '^\[trap\] ' $E/node.rec.err
grep -a -E 'panicked at|RECORD ERROR|M33:|M47:' $E/node.rec.err | head -3
grep -a '^\[trap\] ' $E/node.rec.err | tail -3
```

Record the first stop line, its trap number and pc, and the trap count (the landmark). Also record `node --version`.
- **Wall is `kevent` (363):** rewrite `node_e2e`'s reason with the measured count and evidence path `docs/sweep-evidence/2026-09-30-m47/node.rec.err`. Keep the house form: class, parked, subsystem, the recorder's own words, evidence, UN-IGNORE.
- **Any other wall:** that is the reason, in the same form (H5: route, don't model).

Truncate `node.rec.err` to its last 400 lines before committing (`tail -400`). It carries a `[trap]` line per trap.

- [ ] **Step 2: `csh` and `tcsh` past the refused fork**

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/docs/sweep-evidence/2026-09-30-m47
export RETRACE_TRACE=1
for s in csh tcsh; do tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /bin/$s -o /private/tmp/claude-501/m47-$s.bin < /dev/null > $E/$s.rec.out 2> $E/$s.rec.err; echo "$s record exit=$?"; grep -a -c '^\[trap\] ' $E/$s.rec.err; grep -a -E 'refusing fork|panicked at|RECORD ERROR|M33:|M47:' $E/$s.rec.err | head -4; done
unset RETRACE_TRACE
```

For each shell:
- **It exits cleanly.** Replay it and compare stdout. On `cmp` 0, it passes **only by refusal**: its fork failed and it carried on. Un-ignore it only if its body's assertions (`rc == 0`, replay equal) hold. Add a doc line in the `launchctl` style saying it passes by refusal, and that the refused fork is the program's outcome, not the native one (as `docs/current-state.md` marks the xcrun trio).
- **It stops.** The first stop line is its new wall. Rewrite its `#[ignore]` reason in the file's house form with every field from this evidence. Keep the M37 history sentence short, and name the evidence `docs/sweep-evidence/2026-09-30-m47/<s>.rec.err`.

Append one sentence to the file's header comment: M47 refused `fork` and answered `mach_ports_register`, which moved `csh`/`tcsh` to the state above.

- [ ] **Step 3: The sweep (controller-run)**

Copy `target/aarch64-apple-darwin/debug/retrace` into the session scratchpad and sign it there with `retrace.entitlements`, so a concurrent build cannot swap it. Then run the sweep **detached, with no concurrent `cargo`** (M45 T3-a):

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/docs/sweep-evidence/2026-09-30-m47
export RETRACE_SWEEP_KEEP=$E/sweep
tools/apple-sweep.sh <signed copy> > $E/sweep.log 2>&1; echo "exit=$?"
grep -a '^TALLY' $E/sweep.log
grep -a '^ROW' docs/sweep-evidence/2026-09-30-m47-probe/sweep-427fa0a.log | cut -f2,3 > $E/rows-base.txt
grep -a '^ROW' $E/sweep.log | cut -f2,3 > $E/rows.txt
diff $E/rows-base.txt $E/rows.txt > $E/rowdiff.txt; echo "diff=$?"; cat $E/rowdiff.txt
```

Record the load average at start and end (`sysctl -n vm.loadavg`). The baseline ran at 1.9.

**For every moved row**, measure it against the base binary `/private/tmp/claude-501/m47-base-retrace` before calling it M47's. That is t0's copy of `427fa0a`'s code; if `/private/tmp` was cleared, rebuild it with `git archive 39e0d8d | tar -x -C /private/tmp/claude-501/m47-base` and build it there with `--target-dir /private/tmp/claude-501/m47-base-target`. Record the row twice on each binary, alternating base and swept.
- A move the base binary shows too is **host state**, not M47.
- A move only the swept binary shows is M47's.

Every dyld guest's AMFI answer changed (spec §3d's named consequence), so a row whose landmark count moved is expected. A row whose **outcome** moved must be explained by name. An unexplained moved outcome is **H5**.

Write the evidence `README.md` in M46's shape: method, binary hash and commit, load, the tally, and every moved row with its reason.

- [ ] **Step 4: Commit**

```bash
git add crates/retrace/tests/node_e2e.rs crates/retrace/tests/apple_walls_e2e.rs docs/sweep-evidence/2026-09-30-m47 ':(exclude)docs/sweep-evidence/2026-09-30-m47/*.bin' ':(exclude)docs/sweep-evidence/2026-09-30-m47/sweep/*.bin'
git commit -m "M47 t6: the walk — node at <wall>, csh/tcsh <state>, the sweep <tally>"
```

---

### Task 7: The docs

**Files:**
- Modify: `docs/status-log.md` (append), `docs/current-state.md` (edit in place), `CLAUDE.md`, and `README.md` only where it states something that changed

**Interfaces:** Consumes every task's report, the t0 measurements file and Task 6's evidence. Produces the docs Task 8's reviewer reads.

- [ ] **Step 1: `docs/current-state.md`, edited in place**

Find each passage with `grep -n` and edit it to describe the new reality.

**"What works today."** Add `git` beside the other real programs (`grep -n 'CPython\|jq' docs/current-state.md | head`):
- Xcode's `git` records and replays its local workflow (the in-list, by name);
- `git_e2e` and `gitprims_e2e` are the gates;
- `@rpath` guests load (`rpath_dyn`), because AMFI's dyld policy is answered.

**Known limits.**
- **Retire** the `madvise` hazard (`grep -n 'FREE_REUSABLE\|class-E\|class E\|reusable' docs/current-state.md`). It is not "reduced": the forward that caused it no longer exists. Replace it with one sentence: `madvise` is modelled by advice since M47; only the measured values are accepted, and any other stops the recorder by value.
- **Add:**
  - `fork` is refused with `EAGAIN`, so a program that must fork cannot (git's auto-maintenance fails as it does at a process limit);
  - `vfork` and exec-in-place stay unmodelled;
  - AMFI's dyld policy is the recorder's own, answered by the host (R4), so a guest whose native answer differs (a platform binary, a hardened runtime) sees retrace's;
  - Sandbox call 2 answers the pre-M47 errno, not the native answer, which t0 M2(b) measured (R7);
  - node's wall (Task 6);
  - git's out-list, each with its wall (t0 M4).
- **The ignored-gates paragraph:** add `node_e2e`, and `csh`/`tcsh`'s new state.
- **The Apple-sweep figure and table:** the new tally, measured date, commit and load, every moved row, and the idle-host confirmation of the 49/54 baseline.
- **The gate line:** not touched here. Task 8 writes the measured counts.

- [ ] **Step 2: `CLAUDE.md`**

In "Commands", the e2e list: replace

```markdown
return replay must name, and seeks across it). Run one with
```

with (keep the file's line breaks):

```markdown
return replay must name, and seeks across it), `gitprims_e2e` (M47: the repo-owned fixtures for
  git's mechanisms — `fsops_dyn`'s forwarded path rows landing on disk, `madv_dyn`'s `MADV_ZERO`
  recomputed and never recorded, the no-op reuse pair and the refused advice, `rpath_dyn` loading
  through `@rpath` because AMFI is answered by the host, `forkfail_dyn`'s fork refused with
  `EAGAIN`, and two tampered-trace divergences), `git_e2e` (M47: Xcode's `git` — read commands
  against native, `add`, and default-config `commit` through the refused maintenance fork; skips
  loudly without Xcode), `node_e2e` (M47: node parked at its measured wall; skips loudly without
  Homebrew node). Run one with
```

In "Guest threads", replace:

```markdown
The generic arm's asserts
are `is_signal_syscall`, the workq pair, `kevent_qos` (M45) and `writes_via_nested_pointer` only;
```

with:

```markdown
The generic arm's asserts
are `is_signal_syscall`, the workq pair, `kevent_qos` (M45), `writes_via_nested_pointer`, and since
M47 `madvise` and `fork` only;
```

Match the file's own line breaks exactly, and check each replaced text with `grep -n` first. This edit corrects spec §3h, which said the paragraph does not change (spec §11 item 7).

- [ ] **Step 3: README**

Check it against Tasks 1–6 (`grep -n 'git\|node\|python3\|Limits\|Apple\|madvise' README.md`). Edit only what it states that changed:
- a limit in its Limits list;
- the headline, if git belongs there: the vision's three are python3, node and git, and M47 makes git the second;
- the Apple-sweep figure.

Leave the gate line for Task 8. If nothing it states changed, leave it alone and say so in the report.

- [ ] **Step 4: `docs/status-log.md`, appended**

Append `## M47-gitwrite: git's local workflow, with madvise, __mac_syscall and fork modelled` at the end, never editing an earlier section. Mirror M46's subsections:
- what t0 measured, M1–M6, with the halts considered;
- one subsection per task with its commit hashes, and every control with its symptom;
- the walk: node, `csh`/`tcsh`, the sweep tally, and the moved rows;
- the gate (from Task 8);
- what measurement changed: the spec's §11 corrections, and t0's;
- rulings: R1–R7 and every ruling made during execution;
- **Named weakness** (spec §4): unless t0 M1(c) found a repo-owned trigger, the reclaim corruption is guarded by construction (the assert) and by `git_e2e` alone, at t0 M1(b)'s rate, and `git_e2e` skips without Xcode;
- **What stays owed:**
  - timed waits (M46's, routed out by the operator);
  - real process creation (`fork` beyond refusal, `vfork`, exec-in-place, `posix_spawn`);
  - `kevent` on a guest `kqueue()` (node's next milestone);
  - the V8 JIT;
  - git's out-list, each with its wall;
  - jq's 300k abort (not this class);
  - the Sandbox policy beyond continuity;
  - AMFI's answer for a platform-binary guest;
  - M46's untouched owed items, by reference.

- [ ] **Step 5: Commit**

```bash
git add docs/current-state.md CLAUDE.md README.md docs/status-log.md
git commit -m "M47 docs: current-state in place, status log appended, CLAUDE.md's gate list and generic-arm asserts"
```

---

### Task 8: The gate, the reconciliation, the merge (controller-run)

**Files:**
- Create: `.superpowers/sdd/2026-09-30-retrace-m47-gitwrite/{predict.sh,gate.sh,tally.sh,gate-summary.txt}`

- [ ] **Step 1: Predict the count from source, before the gate**

Create `$L/predict.sh` from M46's (`.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers/predict.sh` in the main checkout), changing only:
- the `cd` line to `/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite`;
- `BASE=39e0d8d`;
- the temp file to `/private/tmp/claude-501/m47-predict-files.txt`;
- the header comment's milestone name.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
bash $L/predict.sh > $L/predict.txt 2>&1; echo "exit=$?"
cat $L/predict.txt
```

Expected:
- `TOTAL #[test]: 905 -> 944+k` (+39 + k), made up of:
  - `gitshapes.rs` +10;
  - `retrace-box/tests/madvise.rs` +6;
  - `retrace-box/tests/macsyscall.rs` +5;
  - `retrace-core/src/machmsg.rs` +2;
  - `retrace-guest/src/lib.rs` +4;
  - `gitprims_e2e.rs` +8;
  - `git_e2e.rs` +3 + k;
  - `node_e2e.rs` +1.
- `TARGETS` lines: `retrace-arch` one file up, `retrace-box` two up, `retrace` three up.

The prediction is then **passed + ignored = 946 + k**, over **158** binaries. Ignored is 10, less one for each of `csh`/`tcsh` that Task 6 un-ignored. Explain every difference by task before running the gate.

- [ ] **Step 2: The chunked gate**

Create `$L/gate.sh` from M46's `gate.sh`, changing only the `cd` line, `D=.superpowers/sdd/2026-09-30-retrace-m47-gitwrite` and the header comment. It keeps:
- the `ws`, `box` and `bins` chunks;
- one chunk per `crates/retrace/tests/*.rs` target;
- clippy;
- `DONE`.

Run it in the background (`bash $L/gate.sh`), and wait for `DONE` in `gate-summary.txt`. **Read the logs, not the script's exit status.**

- [ ] **Step 3: Tally and reconcile**

Create `$L/tally.sh` from M46's `tally.sh`, changing only `D`.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
cat $L/gate-summary.txt
bash $L/tally.sh
grep -a -h 'SKIPPED' $L/gate-*.log | sort | uniq -c
```

The pass bar:
- `failed` is 0, and every line in `gate-summary.txt` reads `exit=0`;
- `passed + ignored` equals Step 1's prediction;
- the number of `test result:` lines equals 158, or Step 1's binary count;
- no `SKIPPED` line appears for a tool this machine has (Xcode's git, jq, Homebrew Python, node, lldb).

Any disagreement is reconciled file by file before anything is merged.

- [ ] **Step 4: Fill the gate line, then the final review**

Replace the numbers on the `**Gate:**` lines of `docs/current-state.md` and `README.md` (`grep -n '^\*\*Gate:\*\*'`) with the tally's. Also replace the sentence after each that names the count of `crates/retrace/tests/` files. Append the gate to the status-log M47 section, then commit:

```bash
git add README.md docs/current-state.md docs/status-log.md
git commit -m "M47 close: the gate — <passed> passed / 0 failed / <ignored> ignored over <binaries>"
```

Dispatch the whole-branch reviewer (the SDD skill's final review) over `39e0d8d..HEAD`. Apply its fix wave, one commit per item. Re-run only the chunks a fix touched, and re-tally.

- [ ] **Step 5: The merge waits for the operator**

Report the branch head, the tally and the reviewer's verdict, and ask the operator to merge from the main checkout:

```bash
git merge --no-ff worktree-m47-gitwrite -m "Merge M47-gitwrite: git's local workflow, with madvise, __mac_syscall and fork modelled"
git rev-parse 'main^{tree}' 'worktree-m47-gitwrite^{tree}'
```

The two tree hashes must be equal: that is the proof that `main` holds exactly the gated tree.

**Do not push.** Before any `git worktree remove`, copy the ledger `.superpowers/sdd/2026-09-30-retrace-m47-gitwrite/` into the main checkout's `.superpowers/sdd/`. `git worktree remove` deletes the ignored ledger silently (the M45 lesson).
