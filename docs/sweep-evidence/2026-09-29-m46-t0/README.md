# M46 t0 evidence — measurements M1–M4, run 2026-09-29

This directory is the evidence for `docs/superpowers/specs/2026-09-29-retrace-m46-gcdtimers-measurements.md`.

**When and where.** Every file was produced on **2026-09-29**, on branch `worktree-m46-gcdtimers`
at commit **`2bc0730`**. The ruling commit `3d40e41` came after these runs and changed only the plan
and spec. The machine:
- macOS 26.5.2 (25F84), kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`;
- `Apple clang version 21.0.0 (clang-2100.1.1.101)`;
- `lldb-2100.0.17.203`.

**What is not here.**
- **Traces.** The two scratch recordings (`/private/tmp/claude-501/m46-clock.bin` and
  `m46-after0.bin`) are not committed.
- **Built binaries.** The fixture binaries are not committed. They were built with
  `clang -arch arm64 -o <f> <f>.c`. sha256: `after_dyn` `0a981c4c…6778e`, `timer_dyn` `0826a5df…60d26`.
- **Sources.** The copies of the libdispatch, xnu, libpthread and libmalloc sources stay in the
  ledger (`.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers/t0/src/`). They are at tags
  `libdispatch-1542.100.32`, `xnu-12377.121.6`, `libpthread-539.100.4` and `libmalloc-812.100.31`.
  - They are the brainstorming copies from `/private/tmp/claude-501/m46-research`.
  - One file was added from its tag: `xnu/libsyscall/wrappers/__commpage_gettimeofday.c`.

**The `retrace` binary** was built from `2bc0730` with `cargo build -p retrace` (`t0-m1-build.log`,
exit 0). It is `target/aarch64-apple-darwin/debug/retrace`, ad-hoc signed by
`tools/codesign-run.sh` (sha256 after signing
`220a378e9ff12e22bef70de53e9c6820355ff30238ff02ff8cc302c8b48802d6`). No source was edited.

**How the commands ran.**
- `L` is the ledger directory `.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers`.
- The `t0-*.sh` files are the shell scripts as run. The worktree's shell guard refuses some
  one-line loop forms, so the commands went into scripts. The scripts wrote into `$L/t0/`, and the
  files were copied here unchanged.
- The lldb runs use a watchdog that kills lldb after a fixed number of seconds, because a livelocked
  guest never exits (see `m1b-attempt2-head.log`).

## Files

| file | produced by |
|---|---|
| `after_dyn.c`, `timer_dyn.c` | the fixtures, byte copies of the plan's Task 1 Step 1 and Task 5 Step 1 texts (`cmp`-identical). No text changed |
| `native-after-{default,two,wall,clock}.out`, `native-timer.out` | `t0-step2.sh`: build both fixtures, run each mode natively |
| `t0-m1-build.log` | `t0-step3.sh`: `cargo build -p retrace` |
| `m1a-clock.{out,err}` | `t0-step3.sh`: `RETRACE_TRACE=1 retrace record-dyn after_dyn -o …/m46-clock.bin -- clock` (exit 0) |
| `m1a-after.{out,err}` | `t0-step3.sh`: `RETRACE_TRACE=1 retrace record-dyn after_dyn -o …/m46-after0.bin` (exit 101, M45's refusal). The `.out` is empty because the CLI prints guest stdout only after a record ends |
| `m1a-clock-fallback-where.out` | `retrace debug …/m46-clock.bin --script "break 0x1804b1b30; continue; where; regs"` |
| `m1a-pc-lookup.log` | `lldb -b -o "image lookup -a 0x1804b1b30" -o "… 0x1804b1b60" -o "… 0x1804afd34" -o "… 0x1804afc24" -- after_dyn`: symbolizes retrace's trap pcs against the unslid shared cache |
| `m1b-attempt1-head.log` | the brief's Step 4 script, run as written. It is the first 200 lines of a 14 MB log. `breakpoint command add -o "bt 12" -o "continue" 1 2 3 4 5` kept only `continue`, so no `bt` printed, and the run livelocked. Killed after 10 minutes |
| `m1b-attempt2-head.log` | the first 120 lines of a run with `breakpoint set -n <fn> -C "bt 12" -C "continue"` on the five clock functions. 21,637 consecutive `mach_absolute_time` hits in 90 s, all at `__commpage_gettimeofday_internal + 44`. Watchdog 90 s |
| `m1b-attempt3.log` | the same, with `mach_absolute_time` given `-c "$lr != 0x181dafc24"`. It still livelocked. Watchdog 240 s |
| `m1b-attempt4.log` | the final `m1b.lldb` before its address breakpoints gained `-s libdispatch.dylib`. They bound to unslid load addresses and never hit |
| `m1b-disasm-timers.log` | `lldb -b -o "disassemble -n <fn>" … -- after_dyn`, for `_dispatch_event_loop_drain_timers`, `_dispatch_event_loop_timer_arm`, `_dispatch_event_loop_timer_program`, `_dispatch_event_loop_timer_delete`, `_dispatch_mgr_queue_drain` and `_dispatch_event_loop_drain` |
| `m1b-libdispatch-clock-callsites.txt` | `lldb -b -o "disassemble -s 0x180336140 -e 0x180372844" -- after_dyn` (libdispatch's whole `__text`; the 64,677-line output is not kept), then `awk` for every call to a clock function, with its containing function |
| `m1b.lldb` | the M1(b) script as finally run: breakpoints on `mach_get_times`, `mach_approximate_time`, `mach_continuous_time`, `mach_continuous_approximate_time` and `gettimeofday`, plus the 15 libdispatch `bl mach_absolute_time` sites (`-s libdispatch.dylib -a <file addr>`), each `-C "bt 12" -C "continue"` |
| `m1b.log`, `m1b-after-two.log`, `m1b-timer.log` | `t0-step4-m1b.sh 300 [mode] [log] [binary]` with `m1b.lldb`, on `after_dyn`, `after_dyn two` and `timer_dyn`. All exited 0 |
| `m1c-disasm.log` | `t0-step5.sh`: `lldb -b -o "disassemble -n mach_absolute_time" -o "disassemble -n __commpage_gettimeofday_internal" -- after_dyn` |
| `m1c-commpage.out` | `t0-step5.sh`: `retrace debug …/m46-clock.bin --script "x 0xfffffc080 0x60"` |
| `m1c-gtod-and-fallback.out` | `retrace debug …/m46-clock.bin --script "x 0xfffffc120 0x28; break 0x1804b1b30; continue; x 0x27ff5e8 8; stepi; regs; x 0x27ff5e8 8"` |
| `m2.lldb` | `breakpoint set -n start_wqthread -C "register read x0 x1 x2 x3 x4 x5 sp" -C "memory read -s8 -fx -c27 $x0-0x480" -C "continue"` |
| `m2-after-default.log`, `m2-after-two.log`, `m2-timer.log` | `t0-step6-m2.sh` (lldb with `m2.lldb` on each fixture; watchdog 300 s; all exited 0) |
| `m3.lldb` | breakpoints on `kevent_qos`, `__workq_kernreturn` and `kevent_id` (a sentinel, never hit). Registers, `memory read … '$x1 ? $x1 : $sp'`, `thread info`, `continue`, all as `-C` |
| `m3-after-{default,two,wall}.log`, `m3-timer.log` | `t0-step7-m3.sh` (lldb with `m3.lldb` on each mode; watchdog 300 s; all exited 0) |
| `m3parse.py`, `m3-calls.txt` | `t0-m3parse.sh` runs `python3 m3parse.py` on each `m3-*.log`, giving the ordered, decoded call lists |
| `m3-origin.lldb`, `m3-origin-{default,wall}.log` | `t0-m3-origin.sh`: breakpoints on `task_get_special_port`, `kevent_qos` and `host_request_notification`, with backtraces, on `after_dyn` and `after_dyn wall` |
| `m4-count.out` | `grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' \| awk -F: '{s+=$2} END {print s}'` |
