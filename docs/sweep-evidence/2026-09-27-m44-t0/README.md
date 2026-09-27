# t0 evidence — M44 Task 0, run 2026-09-27

The kept files behind `docs/superpowers/specs/2026-09-27-retrace-m44-owed-measurements.md`. The
directory is named for the day t0 ran, as M37–M39's are. No trace (`.bin`) is committed (Ruling
P1): every recording these files came from was a scratch file under
`.superpowers/sdd/2026-09-27-retrace-m44-owed/` or the session scratchpad.

**M3 (the seven-target sweep) and M5(i) (CPU timings) were controller-run (Ruling P5)** on
scratch `git archive` trees. Their kept files are copied here from the session scratchpad
(`m3a/`, `m3b/`, `m5i/`, `m3-*`, `m5i-controller-note.md`). The traces they cite were read in place
and are not committed.

## Method and binary

Branch `worktree-m44-owed` at `b2ff28e` (t0 Step 1: `[trap]` prints `x0`–`x7`). Binary
`target/aarch64-apple-darwin/debug/retrace` built from it and ad-hoc signed with
`retrace.entitlements`, sha256 `0b6784d3c6f98d8014ecec82c00e573bee7ecaf11d7f1cbd3e6498ded64cc76d`,
run through `tools/codesign-run.sh` or signed by `m5-lldb.sh`. Three sets of files came from other
builds:
- `m1-automationmodetool.err`, from a throwaway build (below);
- the M3 files (`m3a/`, `m3b/`, `m3-*sweep.log`), from the controller's scratch build of `ebd0266`
  plus the throwaway rows in `m3-rows.diff` (no hash was recorded for it);
- the M5(i) files, from `git archive c652cf1` and `git archive ebd0266` trees built by `cpu.sh`.

The two trace-reading tests (`m3-nums-test.rs`, `m3-paths-test.rs`) ran on the worktree at
`87283e4`. Host: macOS 26.5.2 (25F84), `/usr/bin/lldb` `lldb-2100.0.17.203`.

## Files

| file | what it is |
|---|---|
| `m1-automationmodetool.err` | stderr of `RETRACE_TRACE=1 … record-dyn /usr/bin/automationmodetool -o <scratch> </dev/null`, exit 101, on a **throwaway** build: `b2ff28e` plus, inside `record_box`'s `if trace_log` block, a `[kevent_qos changelist]` dump of `72 × x2` bytes at `va_to_ipa(x1)` and a `[kevent_qos kq]` line (the fd table's view of `x0`) for syscall 374, and a `[ret]` line after the generic forward. No row for 374 was added. Restored with `git checkout`. |
| `m2-ls.err` | stderr of `RETRACE_TRACE=1 … record-dyn /bin/ls -o <scratch> </dev/null` from the worktree root, exit 101; its `[trap] num=461` line carries `x3 = 0x8000` |
| `m3-controller-note.md` | the controller's M3 record: method, both rounds' tables, the native outcomes, Ruling T0-a |
| `m3-rows.diff` | the round-b scratch tree's `crates/retrace-arch/src/lib.rs` against `ebd0266`: the throwaway rows 464, 345, 461 (round a) plus 10 and 128 (round b), never committed |
| `m3-list.txt`, `m3b-list.txt` | `RETRACE_SWEEP_LIST` for round a (7 targets) and round b (4) |
| `m3-sweep.log`, `m3b-sweep.log` | `tools/apple-sweep.sh`'s output per round (`PASS`/`FAIL`/`ROW`/`TALLY`) |
| `m3a/`, `m3b/` | `RETRACE_SWEEP_KEEP_ALL=1`'s kept `<bin>.rec.err` / `.rp.err` / `.rp.out` per target and round (the `.bin` traces are not committed) |
| `m3-native-{ed,ls,desdp}.{out,err}` | each target's native run, stdin `/dev/null` (`ls` from `crates/retrace`); the rc values are in `m3-controller-note.md` |
| `m3-traces.txt` | the 11 kept M3 traces (scratchpad paths) that `m3-nums-test.rs` read |
| `m3-nums-test.rs` | the throwaway test (run as `crates/retrace-trace/tests/t0_m3_nums.rs`, then deleted) that counted syscall numbers in each trace's `Event::Syscall` landmarks |
| `m3-nums.log` | its output: per trace, each number of interest's count and first landmark — the halt rows and M4's corpus reach |
| `m3-paths-test.rs` | the throwaway test (run as `crates/retrace/tests/t0_m3_paths.rs`, then deleted) that seeked a trace to a landmark's trap and read the path strings its registers point at |
| `m3-paths.txt`, `m3-paths.log` | its input (trace, landmark, addresses) and output: the xcrun `/var/tmp/xcrun_db-*` temp paths and `ed`'s `/tmp/ed.*` |
| `m5i-controller-note.md` | the controller's M5(i) record: the CPU table, the gap, B1's pass bar |
| `m5i/cpu.sh` | the timing script (build untimed, then three `/usr/bin/time -p cargo test …` runs) |
| `m5i/cpu-{m42,m44}-{oracle,cpy}.txt` (+ `.build`, `.run1`–`.run3`) | its raw output per tree (`m42` = `c652cf1`, `m44` = `ebd0266`) and target (`oracle` = `hitorder_e2e oracle_threadrust_breakpoints_at_both_switches`, `cpy` = `cpython_crash_e2e`) |
| `m4-nocancel.txt` | the SDK's 32 `SYS_*_nocancel` names and numbers (`grep` of `$(xcrun --show-sdk-path)/usr/include/sys/syscall.h`) |
| `m4-pairs.sh` | pairs each with its plain twin by name, from the same header |
| `m4-pairs.txt` | its output: `<nocancel> <num> <plain> <num>` |
| `m4-pairs-test.rs` | the throwaway test that compared `arg_kinds` on both sides of each pair (run from `crates/retrace-arch/tests/`, then deleted) |
| `m4-pairs.log` | that test's output (`PAIR … | SAME / ONE-SIDED / BOTH-NONE`, with the rows). The first `PAIR` line (`pselect`) shares a line with libtest's `test t0_m4_pairs ...` prefix. |
| `m5-params-test.rs` | the throwaway test that computed the lldb sessions' parameters over one CLI `threadrust` recording, the way `lldb_e2e` does (run from `crates/retrace/tests/`, then deleted) |
| `m5-params.log` | its output: n = 261, svc `0x1804afaf4`, m = 0, `b` `0x1804ecc14`; the child's last landmark 267 = `bsdthread_terminate` (361), svc `0x1804b0b78`, m = 0 |
| `m5-lldb.sh` | the harness: signs the binary, starts `retrace gdbserver <trace> --port 0`, writes the command file, runs `lldb -x -b -s <cmds> </dev/null` bounded at 120 s by `perl -e 'alarm …'`, kills the server, prints the `vCont;s` count |
| `m5-A.{cmds,out,packets}` | shape A (blocked step past another thread's breakpoint): the command file, lldb's stdout, lldb's `gdb-remote packets` log |
| `m5-B.{cmds,out,packets}` | shape B (step a thread that is not running) |
| `m5-C.{cmds,out,packets}` | shape C (step across the child's `bsdthread_terminate`) |
| `m5-next.{cmds,out,packets}` | M5(iv): `next` at `crashy`'s first `bl` (`0x100000530`), on a recording with a relative argv0 (lldb loads no module) |
| `m5-next-abs.{cmds,out,packets}` | the same on a recording with an absolute argv0 (lldb loads `crashy`) |

lldb's stderr was empty in all five sessions and is not kept. Each command file names the
session's scratch paths and port; they are records of what ran, not reusable as they stand.
