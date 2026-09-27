# t0 evidence — M44 Task 0, run 2026-09-27

The kept files behind `docs/superpowers/specs/2026-09-27-retrace-m44-owed-measurements.md`. The
directory is named for the day t0 ran, as M37–M39's are. No trace (`.bin`) is committed (Ruling
P1): every recording these files came from was a scratch file under
`.superpowers/sdd/2026-09-27-retrace-m44-owed/`.

**M3 (the seven-target sweep) and M5(i) (CPU timings) are controller-run (Ruling P5)**; their kept
files are added here when those sections are filled.

## Method and binary

Branch `worktree-m44-owed` at `b2ff28e` (t0 Step 1: `[trap]` prints `x0`–`x7`). Binary
`target/aarch64-apple-darwin/debug/retrace` built from it and ad-hoc signed with
`retrace.entitlements`, sha256 `0b6784d3c6f98d8014ecec82c00e573bee7ecaf11d7f1cbd3e6498ded64cc76d`,
run through `tools/codesign-run.sh` or signed by `m5-lldb.sh`. The one exception is
`m1-automationmodetool.err`, which a throwaway build wrote (below). Host: macOS 26.5.2 (25F84),
`/usr/bin/lldb` `lldb-2100.0.17.203`.

## Files

| file | what it is |
|---|---|
| `m1-automationmodetool.err` | stderr of `RETRACE_TRACE=1 … record-dyn /usr/bin/automationmodetool -o <scratch> </dev/null`, exit 101, on a **throwaway** build: `b2ff28e` plus, inside `record_box`'s `if trace_log` block, a `[kevent_qos changelist]` dump of `72 × x2` bytes at `va_to_ipa(x1)` and a `[kevent_qos kq]` line (the fd table's view of `x0`) for syscall 374, and a `[ret]` line after the generic forward. No row for 374 was added. Restored with `git checkout`. |
| `m2-ls.err` | stderr of `RETRACE_TRACE=1 … record-dyn /bin/ls -o <scratch> </dev/null` from the worktree root, exit 101; its `[trap] num=461` line carries `x3 = 0x8000` |
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
