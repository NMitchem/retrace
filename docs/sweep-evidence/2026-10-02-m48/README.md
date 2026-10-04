# Sweep evidence — M48 Task 9, run 2026-10-04

The directory keeps the plan's 2026-10-02 name. **Every file here was produced on 2026-10-04,
between 05:37 and 06:17 (-03), and fix round 1's `echo` measurement ran at 06:37**, on this machine:
- Apple M4 Pro, 12 CPUs, 24 GiB;
- macOS 26.5.2 (25F84), kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`;
- Homebrew node 25.6.1 (`/opt/homebrew/Cellar/node/25.6.1/bin/node`).

**The commit.** Every binary was built at **`4e37b88`** (M48 Task 8's last commit, Task 9's base) by
`t9-build.sh`. The scripts' own `commit=` fields print later hashes (`f5789bb`, `c695111`,
`dc43f4b`), because this task commits its evidence step by step; none of those commits touches
`crates/`, `Cargo.toml`, `Cargo.lock` or the sweep's scripts (`git diff --stat 4e37b88 HEAD --
crates Cargo.toml Cargo.lock tools/apple-sweep.sh tools/apple-sweep-binaries.txt` is empty), so
every run measured `4e37b88`'s code. `tools/bench.py` gained its node row in Step 4; that is the only
non-evidence file this task changed.

**The binaries** (sha256 after signing with `retrace.entitlements`):

| name | path | build | sha256 |
|---|---|---|---|
| debug (the sweep's) | `/private/tmp/claude-501/m48-t9-debug-retrace` | `cargo build -p retrace` at `4e37b88` | `0eece64c8cf6c2c9311f3d2015b95f083e65af32061073af39a9893a80070487` |
| release (the walk's) | `/private/tmp/claude-501/m48-t9-release-retrace` | `cargo build --release -p retrace` at `4e37b88` | `b420e90a114b55749f150fa0c9fcd66ce54987c3e2885adec8f18ef8eda3a637` |
| base (the controls') | `/private/tmp/claude-501/m48-base-retrace` | t0's debug build of `50e716f` (M47's code) | `7b309adc92652421aff14ac8345c5a6adc324cc901471f0d03479c714df5027c` (t0's record, unchanged) |

The bench signs its own copy of `target/aarch64-apple-darwin/release/retrace`, the release
binary above before signing. The base and the debug binary carry the same `TRACE_MAGIC`
(`RT\x00\x0b`), but every control run records and replays with one binary.

**Results in one paragraph.** Every node walk matches t0: `e`, `t10`, `t2000` and `natives` print
`1`, `2`, `2`, `3` and exit 0; `crash` prints its `CRASHJS` marker and exits 139; every replay
exits as its record did, with the same stdout, twice. Every census count M48 is measured by is t0's
count. `reverse-continue` on the crash demo's cell stops at a `hit watch` inside the `MAP_JIT` code
range, the cell reads 2 and then the target. The nine parked gates all still fail, each at the wall
its `#[ignore]` reason names. The bench timed all seven rows, node's included. **The Apple sweep
tallied `pass=49 fail=5 skip=0` on an idle host**, the expected figure; its only moved outcome is
`dddiagnose`'s known coin flip, attributed to host state by the controls. One finding: in the
controls, `dddiagnose` showed a **third face** (a libswiftCore data abort), once in 16 runs on the M48
binary and never in 16 on the base binary. It is **a second sighting of a documented, unattributed
class**: M47 saw the same class once, on M46's code (below, Verdicts).

## Files

| file | produced by | binary | time (-03) |
|---|---|---|---|
| `t9-build.sh`, `t9-build.out` | `bash t9-build.sh \| tee t9-build.out` (Step 1) | builds both | 05:37:39–05:37:54 |
| `t9-parked.sh`, `parked.txt` | `bash t9-parked.sh > parked.txt` (Step 2) | the test harness's debug build (`CARGO_BIN_EXE_retrace`, built by Step 1) | 05:38 |
| `parked-apple.log`, `parked-stackoverflow.log`, `parked-symbols.log` | t9 addition: copies of the three cargo logs `t9-parked.sh` writes to the (untracked) ledger as `t9-parked-*.log` | as above | 05:38 |
| `t9-walk.sh`, `t9-census.sh`, `t9-walks.sh`, `walks.log` | `nohup bash t9-walks.sh > walks.log` (Step 3) | release | 05:39:16–05:40:36 |
| `walk-<tag>.{out,err,status,rp1.out,rp1.err,rp2.out,rp2.err}` | `t9-walk.sh <tag> -- <node args>` for `e`, `t10`, `t2000`, `natives`, `crash` (each `.err` whole) | release | 05:39–05:40 |
| `walk-<tag>.census` | `t9-census.sh <tag>` | — (reads `walk-<tag>.err`) | 05:40 |
| `walk-crash.dbg.out`, `walk-crash.dbg.err` | `/usr/bin/time -l … debug m48-t9-crash.bin --script "continue; watch $CELL 8; reverse-continue; x $CELL 8; stepi; x $CELL 8"` | release | 05:40 |
| `bench.txt` | `caffeinate -i bash -c '… python3 tools/bench.py --runs 5 --retrace target/aarch64-apple-darwin/release/retrace …'` (Step 4) | release | 05:41:44–05:43:34 |
| `sweep-attempt1-slept/` (`sweep.log`, `sweep-load.txt`, `sweep-wrapper.log`, `sweep/*.err`) | the first `caffeinate -i bash t9-sweep.sh` (Step 5); **invalid: the host slept 05:44:24–06:01:34 in the middle of it**; kept, traces deleted | debug | 05:43:55–06:06:13 |
| `t9-sweep.sh`, `sweep.log`, `sweep-load.txt`, `sweep-wrapper.log`, `sweep/*.err` | the second `caffeinate -s -i bash t9-sweep.sh` (Step 5); **the sweep this README reports** (`sweep/*.bin` are on disk, never committed) | debug | 06:06:32–06:11:41 |
| `t9-rowdiff.sh`, `t9-rowdiff.out`, `rows-base.txt`, `rows.txt`, `rowdiff.txt`, `rowdiff-norm.txt` | `bash t9-rowdiff.sh > t9-rowdiff.out` (Step 6) | — | 06:12 |
| `moved-rows.txt` | written by hand from `rowdiff-norm.txt`, plus `dddiagnose` always (Step 6) | — | 06:12 |
| `t9-controls.sh`, `controls.txt` | `caffeinate -s -i bash t9-controls.sh > controls.txt` (Step 6) | base and debug | 06:12:19–06:13:43 |
| `ctl-d5-t9/` | t9 addition: the kept `rec.err`, `rp.err` and `rp.out` of `controls.txt`'s round `d5-t9` (the third face) | debug | 06:13 |
| (no file; the figures are inline under The bench) | fix round 1: two `record-dyn /bin/echo … -- hi` runs per binary, to attribute `echo`'s trace size | base and release | 06:37 |
| `t9-controls-extra.sh`, `controls-extra.txt` | t9 addition: `caffeinate -s -i bash t9-controls-extra.sh > controls-extra.txt`, ten more `dddiagnose` rounds per binary | base and debug | 06:15:02–06:16:49 |

## The walk

**Command.** `t9-walks.sh` ran the five walks one after another on the release binary, at a 1-minute
load of 2.05 at the start and 1.52 at the end. Each walk records under `RETRACE_TRACE=1` and
`RETRACE_SPRR=1`, stdin `/dev/null` and stdout through `| cat`, then replays twice. t0's figures
are measurements §M2 (`docs/superpowers/specs/2026-10-02-retrace-m48-node-measurements.md`), taken
on t0's walk binary (`50e716f` + the probe's stubs, release).

| walk | record rc | prints | trace (bytes) | traps | record | replay 1 | replay 2 | t0 §M2 (rc, traps, trace) |
|---|---|---|---|---|---|---|---|---|
| `e` | 0 | `1` | 525 662 853 | 1470 | 3 s | rc 0, `same_stdout=yes`, 4 s | rc 0, yes, 3 s | 0, 1470, 525 840 709 |
| `t10` | 0 | `2` | 526 120 779 | 1476 | 4 s | rc 0, yes, 4 s | rc 0, yes, 3 s | 0, 1471, 525 718 006 |
| `t2000` | 0 | `2` | 525 741 881 | 1474 | 4 s | rc 0, yes, 3 s | rc 0, yes, 4 s | 0, 1482, 530 151 961 |
| `natives` | 0 | `3` | 526 105 443 | 1476 | 3 s | rc 0, yes, 4 s | rc 0, yes, 4 s | 0, 1471, 525 766 782 |
| `crash` | **139** | `CRASHJS cell=0x700c59600 target=0x4000dead0000 rows=2 opt=101001` (no `UNREACHED`) | 1 203 946 763 | 1407 | 8 s | rc 139, yes, 9 s | rc 139, yes, 8 s | 139, 1407, 1 203 864 923 |

Every row matches the brief's table. Each walk printed the one expected refusal, M23's
`refusing mach_msg2 message-queue send (msgh_id 0x400000cf …)`, and carried on (P1). Each replay's
stderr is its `fall-throughs` line only (`crash`'s adds its `guest crashed:` line); `e`'s record and
replays counted 6 fall-throughs and every other walk 5 (t0: 5 in each walk), equal between each
record and its replays.

**Where the trap counts differ from t0.** A per-number diff of every walk's `[trap]` lines against
t0's uncapped logs (the ledger's `t0/m2-<walk>.err`, of which the committed t0 evidence keeps the
last 400 lines) differs only in:
- `gettimeofday` (116), by −1 to +6 in every walk: the guest's own clock reads, which move run to
  run (M45 t0 M1);
- `munmap` (73), by ±1–2 in every walk (t0 measured 56–58 partial `munmap`s, varying run to run);
- `t2000`: `-15` 32 → 31;
- `crash`: `open` (5) 103 → 100. Task 8 moved the demo from the probe's directory to
  `crates/retrace-guest/node/` and the addon to `/private/tmp/claude-501/m48-t9-crash_addon.node`
  (T8-a), and node's module resolution opens a path-dependent number of files. Not traced further.

No other number's count moved, and the sets of distinct syscall numbers and distinct `msgh_id`s are
**identical to t0's in every walk**.

**The censuses** (`walk-<tag>.census`) against §M3–§M6:

| count | `e` | `t10` | `t2000` | `natives` | `crash` | t0 | verdict |
|---|---|---|---|---|---|---|---|
| `kevent` (363) | 11 | 12 | 14 | 11 | 7 | 11, 12, 14, 11, 7 (§M3; uncapped t0 logs) | same, walk by walk |
| `psynch_cvbroad` (303) | 1 | 1 | 1 | 1 | 0 | 1, 1, 1, 1, 0 (§M4) | same |
| `psynch_cvsignal` (304) | 9 | 9 | 10 | 9 | 8 | 9, 9, 10, 9, 8 (§M4) | same |
| `psynch_cvwait` (305) | 16 | 16 | 18 | 16 | 9 | 16, 16, 18, 16, 9 (§M4) | same |
| other psynch (297–302, 306–309, 312) | 0 | 0 | 0 | 0 | 0 | none (§M4) | only 303–305 occur |
| `bsdthread_create` (360) | 6 | 6 | 6 | 6 | 6 | 6 (§M6) | same |
| `bsdthread_terminate` (361) | 5 | 5 | 5 | 5 | 0 | 5, 5, 5, 5, 0 | same |
| `setsockopt` (105) | 1 | 1 | 1 | 1 | 1 | 1 (§M2 row 6) | same |
| `getsockname` (32) | 0 | 0 | 0 | 0 | 0 | 0 (§M2 row 0) | no socket |
| `munmap` (73) | 132 | 129 | 127 | 131 | 97 | 131, 130, 129, 132, 98 | ±1–2, run-to-run |
| `mprotect` (74) | 34 | 34 | 34 | 34 | 34 | 34 each | same |
| `madvise` (75) | 32 | 32 | 32 | 34 | 18 | 32, 32, 32, 34, 18 | same |
| `MAP_JIT` mmaps | 1: `len=0x10000000 prot=0x0 flags=0x41842` | 1, same | 1, same | 1, same | 1, same | 1, the same shape (§M5) | same |
| SPRR writes | 264 | 264 | 264 | 274 | 20 | 264, 264, 264, 274, 20 (§M5) | same |
| SPRR writes by thread | main 16, tid 2 248 | 16 / 248 | 16 / 248 | 22 / 252 | main 20 | the same splits (§M5) | same |
| SPRR values | `0x2010002030100000` 132, `0x2010002030300000` 132 | 132 / 132 | 132 / 132 | 137 / 137 | 10 / 10 | only commpage `+0x118` and `+0x110` (§M1(c)) | every value is one of the two |
| `msgh_id`s | 0x400000cf, 200, 206, 3405, 3409, 3410, 3418, 3419, 412, 4811, 4822, 8000, 8001 | same set | same set | same set | same set | the same set in each walk | same |

t0's kevent counts per walk are its §M3 census (`kevent calls`: 11, 12, 14, 11, 7), which the
`[trap] num=363` lines of its uncapped logs reproduce exactly; t0's walk-to-walk range is 7–14. The
production build logs no idle jump (that line was the probe's), so `t2000`'s idle jump is not
counted here; the walk printed `2` and replayed.

**The crash session** (`walk-crash.dbg.out`, `walk-crash.dbg.err`):

```
> continue
guest crashed: pc=0xa3da8c738 far=0x4000dead0000 esr=0x92000005
> watch 0x700c59600 8
watch at 0x700c59600 len 8
> reverse-continue
hit watch 0x700c59600 (write at 0xa2d000948) at (1378, 1406401)
> x 0x700c59600 8
0x700c59600: 02 00 00 00 00 00 00 00
> stepi
> x 0x700c59600 8
0x700c59600: 00 00 ad de 00 40 00 00
```

- **The hit is a `hit watch` line.** The writing pc `0xa2d000948` is inside the `MAP_JIT` range's
  committed RWX part: the walk's one `mprotect(0xa2d000000, 0xffc0000, RWX)` (`walk-crash.err`, the
  `num=74` line right after the `MAP_JIT` mmap) starts at `0xa2d000000`. The store's offset into the
  range is not reported as a fact here (ledger: it is unexplained).
- **The cell reads `02 00 …`** (the warm-up value) at the hit, and the target `0x4000dead0000` one
  `stepi` later.
- **Cost:** `12.14 real`, `3 516 219 392` maximum resident set size, `4 214 478 944` peak memory
  footprint (t0 §M7: 13.63 s, 4 135 682 048, 4 196 702 256). `debug rc=0`.
- The crash itself: pc `0xa3da8c738` (the addon's text), FAR `0x4000dead0000`, ESR `0x92000005`, as
  t0 §M7. The record's two `[fault]` lines (node's own SIGSEGV handler, then the terminal crash) are
  both at that pc.

## The bench

**Command.** `tools/bench.py --runs 5` on the release build, under `caffeinate -i`, 05:41:44–05:43:34.
The 1-minute load was **1.45 at the start** and **1.52 at the end**. `pmset -g log` shows no sleep
between 05:41:44 and 05:44:24 (the first sleep came 50 s after the bench ended), so the figures stand.

| workload | native | record | replay | rec x | rep x | rec RSS | trace |
|---|---|---|---|---|---|---|---|
| echo | 0.001 s | 0.196 s | 0.202 s | 131.8x | 136.0x | 104 MiB | 28 MiB |
| ls /usr/bin | 0.004 s | 0.231 s | 0.236 s | 53.1x | 54.2x | 107 MiB | 33 MiB |
| jq, 5 MB file | 0.080 s | 1.979 s | 2.050 s | 24.8x | 25.7x | 446 MiB | 286 MiB |
| jq, compute | 0.019 s | 0.297 s | 0.306 s | 15.3x | 15.7x | 127 MiB | 40 MiB |
| python -c 'print(1)' | 0.012 s | 0.600 s | 0.616 s | 49.9x | 51.2x | 214 MiB | 85 MiB |
| python, 30M-step loop | 0.954 s | 1.633 s | 1.644 s | 1.7x | 1.7x | 215 MiB | 85 MiB |
| node -e 'console.log(1)' | 0.041 s | 3.453 s | 3.533 s | 85.0x | 87.0x | 733 MiB | 502 MiB |

Every row was timed: no `FAILED`, no `SKIPPED`. **The node row is new.** Against the table in
`docs/current-state.md` (2026-09-28, branch `readme-launch`), new ÷ old:

| workload | native | record | replay | RSS | trace |
|---|---|---|---|---|---|
| echo | 0.002 → 0.001 s | 0.238 → 0.196 s (0.82) | 0.262 → 0.202 s (0.77) | 104 → 104 MiB | 32 → 28 MiB |
| ls /usr/bin | 0.006 → 0.004 s | 0.273 → 0.231 s (0.85) | 0.269 → 0.236 s (0.88) | 107 → 107 MiB | 32 → 33 MiB |
| jq, 5 MB file | 0.089 → 0.080 s (0.90) | 2.246 → 1.979 s (0.88) | 2.299 → 2.050 s (0.89) | 435 → 446 MiB | 282 → 286 MiB |
| jq, compute | 0.022 → 0.019 s (0.86) | 0.344 → 0.297 s (0.86) | 0.351 → 0.306 s (0.87) | 126 → 127 MiB | 40 → 40 MiB |
| python `print(1)` | 0.014 → 0.012 s (0.86) | 0.692 → 0.600 s (0.87) | 0.681 → 0.616 s (0.90) | 215 → 214 MiB | 84 → 85 MiB |
| python loop | 0.973 → 0.954 s (0.98) | 1.739 → 1.633 s (0.94) | 1.696 → 1.644 s (0.97) | 215 → 215 MiB | 85 → 85 MiB |

Every record and replay figure is 3–23% faster than on 2026-09-28, and every native figure is
faster too (2–14% for the four workloads above 10 ms; `echo` and `ls` are at the script's
millisecond resolution). Since the native column moves with the rest, the move is not evidence that
retrace got faster.

**The `echo` trace size, 32 → 28 MiB, is run-to-run variation, not a binary difference** (fix
round 1).
- **The measurement.** At 06:37, under `caffeinate -s -i`, I recorded `echo` twice with each binary,
  using the bench's own shape: `<binary> record-dyn /bin/echo -o <trace> -- hi`, with stdin, stdout
  and stderr on `/dev/null`. The two binaries were the base `m48-base-retrace` (M47's code) and
  `m48-t9-release-retrace`. Every record exited 0.

  | binary | first record | second record |
  |---|---|---|
  | base (M47's code) | 29 212 331 B (27.9 MiB) | 33 303 042 B (31.8 MiB) |
  | t9 release (M48's code) | 29 165 051 B (27.8 MiB) | 33 375 268 B (31.8 MiB) |

- **The reading.** Both binaries write both sizes, about 28 MiB and about 32 MiB. The bench reports
  the trace of its last record only, one sample. So the 2026-09-28 table's 32 MiB and this run's
  28 MiB are two draws from the same spread. The difference is neither M48's nor a difference
  between these two binaries.
- **Not measured:** what makes the size vary.
- **Clean-up:** I deleted the four traces.

Node's trace (502 MiB) is the
`console.log(1)` walk's ~525 MB, and its record RSS 733 MiB; its 3.45 s record is about 3.4 s of
retrace over a 0.04 s native run (P10: V8's reservations are backed in full, so they are in both
snapshots).

## The parked gates

`t9-parked.sh` (`parked.txt`): `#[ignore] lines: 9`, and nine `FAILED` lines (`apple_walls_e2e
exit=101`, `stackoverflow_rust_e2e exit=101`, `symbols_e2e exit=101`). `dddiagnose` failed, so no
re-runs were owed.

| test | the reason's wall | observed failure | same or moved |
|---|---|---|---|
| `csh_records_and_replays` | `wait4` (7), no `arg_kinds` row (M47) | record exited 101: `panicked at crates/retrace-arch/src/lib.rs:1069:38: M33: syscall 7 (7) has no arg_kinds row` | **same** (the panic's source line moved, 1036 → 1069) |
| `tcsh_records_and_replays` | `wait4` (7), no `arg_kinds` row (M47) | record exited 101: the same panic at `lib.rs:1069:38`, syscall 7 | **same** (source line only) |
| `automationmodetool_records_and_replays` | `kevent_id` (375), no `arg_kinds` row (M46) | record exited 101: `panicked at crates/retrace-arch/src/lib.rs:1069:38: M33: syscall 375 (375) has no arg_kinds row` | **same** (source line only) |
| `desdp_records_and_replays` | exec-in-place, `posix_spawn` (244) refused (M44) | record exited 71: `[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14 without forwarding` | **same** |
| `dyld_info_records_and_replays` | exec-in-place, `posix_spawn` (244) refused (M44) | record exited 71, the same refusal line | **same** |
| `flex_records_and_replays` | exec-in-place, `posix_spawn` (244) refused (M44) | record exited 71, the same refusal line | **same** |
| `dddiagnose_records_and_replays` | the I/O Kit main port, `host_get_io_main` (msgh_id 205) (M44; the `mfm_alloc` face M45) | record exited 4: `RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 205 dest 0xc03 (guest task port Some(515)) send_size 24` | **same** (the 205 face) |
| `a_rust_stack_overflow_strikes_its_own_guard_page` | the blocked-signal wall (M21) | `panicked at crates/retrace-core/src/lib.rs:216:21: raising blocked signal 10 synchronously is not modelled …` | **same** (the reason cites `:203`; the line was already 216/217 at M47's `e6caa65`, so that drift predates M48) |
| `cache_symbol_e2e` | the shared-cache symbol wall (M19/M20) | `a shared-cache pc must resolve once the cache's local symbols reach the recording` (`symbols_e2e.rs:214`) | **same** |

## The sweep

**The first attempt is invalid.** It started at 05:43:55 (1-minute load 1.60) under `caffeinate -i`,
and the host went to sleep at **05:44:24** (`pmset -g log`: `Entering Sleep state due to
'Maintenance Sleep'`) and woke at **06:01:34** (`DarkWake from Deep Idle`). Its load sampler shows the
17-minute gap (`05:44:25` → `06:02:03`). The lid had been closed since 04:17:50 (`Entering Sleep
state due to 'Clamshell Sleep'`; `AppleClamshellState = Yes` when checked at 06:02), and the host had
been in DarkWake since 04:42:54, so **every run in this directory ran with the lid closed, in
DarkWake**; `caffeinate -i`'s idle-sleep assertion did not stop the 05:44:24 sleep. It ran to the end (`TALLY pass=50 fail=4 skip=0`,
`dddiagnose` on its `mfm_alloc` face, PASS 139/139), and it is kept in `sweep-attempt1-slept/`
for the record only: its traces were deleted, and no figure here comes from it.

**The sweep reported here** is the second run of `t9-sweep.sh`, under `caffeinate -s -i` (a
system-sleep assertion, held on AC power), with no `cargo` running and nothing else of this task's
running:

| `pidstart` | recpid range | `TALLY` | `SWEEP_EXIT` | ran | 1-minute load |
|---|---|---|---|---|---|
| 38302 | 38330–39980 | `pass=49 fail=5 skip=0` | 0 | 06:06:32–06:11:41 | 1.67 at the start, 1.21–2.27 during it (`sweep-load.txt`, every 30 s), 2.02 at the end |

No sleep entry in `pmset -g log` after 06:01:34. No refusal by Ruling T9-a's load gate, so there is
no `sweep-refusals.txt`. One unrelated process, a `Virtualization.framework` VM service (5–24% of a
CPU), ran throughout; it is not this task's and was not touched.

The five non-clean rows: `csh` and `tcsh` (`wait4`, 7), `automationmodetool` (`kevent_id`, 375),
`dddiagnose` (FAIL 4/3 at msgh_id 205, landmark 456), and `yes` (timed out after 30 s, as always).
The xcrun trio passes 71/71 and `ps` passes, as in M47's run.

**The row diff** (`t9-rowdiff.sh`, against M47's `a8a1ecd` sweep,
`docs/sweep-evidence/2026-09-30-m47/sweep.log`, `TALLY pass=50 fail=4`):
- `rowdiff.txt` (path and result only) marks one moved outcome: `/usr/bin/dddiagnose` PASS → FAIL.
- `rowdiff-norm.txt` (path, result, rc, rp, landmark, rec_reason, thread ids normalised): **50
  rows identical, 4 differ**: `dddiagnose` (PASS 139/139 → FAIL 4/3 at 205, landmark 456), and
  `csh`, `tcsh` and `automationmodetool`, whose only difference is the panic's source line,
  `crates/retrace-arch/src/lib.rs:1036:38` → `:1069:38` (M48 Task 2 added `arg_kinds` rows above
  `forwarded_shape`, the M44/M47 precedent).
- `moved-rows.txt` lists those four.

`t9-rowdiff.sh` is M47's `t6-rowdiff.sh` with the brief's lines changed (`W`, `E`, `S`, `A`, `B`, the
temp names, the header comment) and **one deviation**: its four output labels read `M47 a8a1ecd`
and `M48` where the copied body said `427fa0a` and `M47`, which would have misnamed both sides of
`rowdiff-norm.txt`. `t9-controls.sh` differs from `t6-controls.sh` only in the brief's variables
and header (`diff` of the two is those lines and the `t6` → `t9` labels).

**The controls** (`t9-controls.sh` → `controls.txt`), base and t9 alternating, through
`tools/apple-sweep.sh` itself, 06:12:19–06:13:43, 1-minute load 1.61–2.60:

| round | `csh` | `tcsh` | `automationmodetool` | `dddiagnose` |
|---|---|---|---|---|
| r1-base | FAIL, `wait4` panic at `lib.rs:1038:38` | the same | FAIL, 375 panic at `lib.rs:1038:38` | FAIL at 205 |
| r1-t9 | FAIL, `wait4` panic at `lib.rs:1069:38` | the same | FAIL, 375 panic at `lib.rs:1069:38` | FAIL at 205 |
| r2-base | FAIL, `wait4` at `:1038:38` | the same | FAIL, 375 at `:1038:38` | PASS 139 (`mfm_alloc`, far `0x6000050040`) |
| r2-t9 | FAIL, `wait4` at `:1069:38` | the same | FAIL, 375 at `:1069:38` | PASS 139 (`mfm_alloc`, far `0x4000050070`) |
| d3 | — | — | — | base PASS 139; t9 FAIL at 205 |
| d4 | — | — | — | base FAIL at 205; t9 PASS 139 |
| d5 | — | — | — | base PASS 139; **t9 FAIL rc 4: `RECORD ERROR: non-syscall exit: data abort (EC=0x24 ISS=0x7 FSC=0x7) far/ipa=0x10 (UNMAPPED) pc=0x193bc20ec elr=0x1804ae10c`** |
| d6 | — | — | — | base PASS 139; t9 PASS 139 |

(The base binary's panic line is `lib.rs:1038:38`, not M47's swept `1036:38`, because t0 built it
from `50e716f`, whose `retrace-arch` carries two more lines than `a8a1ecd`'s, from M47's fix wave
`fea497e`; the message sits at line 1037 in `a8a1ecd`, 1039 in `50e716f` and 1070 in `4e37b88`. The
wall is the same.)

**Attribution:**
- **`csh`, `tcsh`, `automationmodetool`: the same wall on both binaries, every round.** Only the
  panic's source line differs, which is the code's line numbering, not an outcome. Not a moved row.
- **`dddiagnose`: host state.** Both faces occur on both binaries: base 4 PASS (`mfm_alloc`) / 2 FAIL
  at 205; t9 3 PASS / 2 FAIL at 205 / 1 third face. The sweep's FAIL at 205 is M45's coin flip.
- **The third face: a second sighting of a documented, unattributed class.** `controls-extra.txt` (a
  t9 addition) ran ten more rounds per binary: base 5 PASS / 5 at 205, t9 4 PASS / 6 at 205, no third
  face. Over 16 samples per binary:

  | binary | PASS 139 (`mfm_alloc`) | FAIL 4/3 at msgh_id 205 | FAIL 4: data abort at far `0x10` |
  |---|---|---|---|
  | base (`50e716f`) | 9 | 7 | 0 |
  | t9 (`4e37b88`) | 7 | 8 | **1** |

  Counting the parked run (205) and both sweep attempts (attempt 1 `mfm_alloc`, attempt 2 205), the
  M48 binary met the third face once in 19 runs.

  **What it is.** Symbolicated with lldb against the host's shared cache
  (`target create /usr/bin/dddiagnose; image lookup -a`), its pc `0x193bc20ec` is **libswiftCore
  `swift::RefCounts<…>::incrementSlow + 88`**: a load at `0x10`, read here as a null side-table
  pointer. (`elr=0x1804ae10c` is `mach_absolute_time + 108`, presumably a stale `ELR_EL1`.) It occurs
  at landmark 393; the last line the recorder printed before it is the refused message-queue
  receive, the region where M45 placed the `mfm_alloc` face ("about 9 landmarks after the refused
  receive"). The record ends with a recorder error (exit 4), not an `Event::Crash`. Replay reports
  `DIVERGENCE at landmark 393 … data abort …` (`ctl-d5-t9/`), so it reproduces the abort.

  **M47 recorded the same class, on pre-M47 code.** `docs/sweep-evidence/2026-09-30-m47/README.md:126`
  (the table row "d5: a third face") and `:135-143` ("A third face, on the base binary only, once"),
  with `dddiagnose.face3.txt`, `dddiagnose.d5-base.rec.err` and `controls.txt:45` there. It was a data
  abort at far `0x1bf0` in libswiftCore `_swift_release_dealloc+48` (a Swift object whose isa read as
  `0x1c00`), on the base binary `427fa0a` (M46's code), in 1 of 6 runs at a 1-minute load of 11.60.
  M47 wrote that whether it is the heap-corruption class its `mach_vm_map` mask fix removed "was not
  measured" (also `docs/current-state.md:1116-1119`, `docs/status-log.md:15440-15441` and `:15609`).
  This sighting is the same class at a different site. By generation:

  | code | runs | third-class faces |
  |---|---|---|
  | M46 (`427fa0a`, M47's base) | 6 | 1 (`_swift_release_dealloc+48`, far `0x1bf0`) |
  | M47 (`a8a1ecd` 7, `50e716f` 16) | 23 | 0 |
  | M48 (`4e37b88`) | 19 | 1 (`incrementSlow+88`, far `0x10`) |

  About 2 in 48 runs overall. This one occurred at a 1-minute load of about 1.8 (round `d5-t9`
  started at 1.83), so load alone does not explain the class. And it occurred after M47's mask fix,
  so that fix did not remove the class.

  **What its trace shows** (parsed by the Task 9 reviewer, from the kept trace):
  - 1 `Snapshot` and 392 `Syscall` landmarks, all on thread 0;
  - none of 360/361, 303–305, 363, 515/516, −36/−33, 367/368 or 374/375;
  - its two mmaps are not `MAP_JIT`.

  So M48's SIMD restore at thread switches (Task 1), `kevent` (Task 4), psynch (Task 5) and the
  `MAP_JIT`/SPRR paths (Tasks 6–7) were never exercised in this run. The M48 changes it did run
  through are the global ones:
  - SCTLR UCI;
  - Task 3's trimming, on dyld's piecewise unmap of its executable, the partial-`munmap` path every
    dyld guest runs. The abort is at VA `0x10`, not at a released page.

  `getpid` appears at landmarks 15, 40 and 195, exactly as in the 205-face trace, so it is not the
  lost-store-exclusive class.

  **What it does not show:**
  - the face's rate on either binary: 1 of 16 against 0 of 16 is not a significant difference;
  - its root cause;
  - the guest memory at the fault;
  - anything beyond lldb's symbol for the pc.

  By the brief's rule it is **unattributed**. It was not on both t9 rounds, and the sweep's own row
  did not take it. Its trace is kept, not committed, at
  `/private/tmp/claude-501/m48-t9-ctl/ctl/d5-t9/dddiagnose.bin`.

## Verdicts

- **No new wall (H5 does not fire).** Every node walk reached t0's outcome, and no walk met a
  refusal, panic or divergence that t0 did not. The censuses match t0 count for count, except the
  run-to-run counts named above (`gettimeofday`, `munmap`, the crash demo's `open`s).
- **No row moved because of M48.** The sweep's one moved outcome, `dddiagnose`, is host state; the
  three other differing rows differ only in a panic's source line.
- **The headline stays 49 of 54** (`TALLY pass=49 fail=5 skip=0`), on an idle host, measured on the
  M48 binary.
- **Step 7 was not triggered.** No parked gate moved to another wall, none passed, and no
  `#[ignore]` was added (the count is 9). `apple_walls_e2e.rs` is unchanged.
- **Findings, each named:**
  1. **The host slept in the first sweep attempt** (05:44:24–06:01:34, lid closed, `caffeinate -i`
     only). It was re-run under `caffeinate -s -i`; the walks (05:39–05:40) and the bench
     (05:41:44–05:43:34) finished before the sleep. Every run here ran in DarkWake with the lid
     closed; nothing in the figures suggests throttling (the bench is faster than 2026-09-28's).
  2. **`dddiagnose`'s third face is a second sighting of a documented, unattributed class.**
     - **This sighting:** libswiftCore `incrementSlow + 88` loads from `0x10`, once in 16 control
       runs on the M48 binary and never in 16 on the base binary. That is not a significant
       difference.
     - **The class:** M47 saw it once on M46's code (`_swift_release_dealloc+48`, far `0x1bf0`;
       `docs/sweep-evidence/2026-09-30-m47/README.md:126`, `:135-143`). By generation it is M46
       1/6, M47 0/23 and M48 1/19.
     - **What the trace rules out:** it exercised none of M48's thread, `kevent`, psynch or
       `MAP_JIT` paths, and it is not the lost-store-exclusive class.
     - **What stays unknown:** its root cause and its rate.
     - Unattributed (above).
  3. **A guest's null-page load ended the record with a recorder error, not a crash.** The third
     face shows it: a load at `0x10` ends the record with `RECORD ERROR … far/ipa=0x10 (UNMAPPED)`,
     exit 4, where a native process would take SIGSEGV. The stage-2 behaviour behind it **predates
     M48**: `crates/retrace/tests/trim_e2e.rs:55-63` says a stage-2 translation fault has been a
     recorder error (exit 4) and never an `Event::Crash` since M2, and is not what Task 3 changes.
     `0x10` is not a released page. It is observed here, not measured further.
  4. **The `#[ignore]` reasons quote stale source lines.** `csh`'s and `tcsh`'s quote
     `lib.rs:1036:38` (the panic is now at `:1069:38`), `automationmodetool`'s `:1002:38` and
     `:1036:38`, and the stack-overflow reason `retrace-core/src/lib.rs:203` (now `:216`, a drift
     that predates M48). Each wall is the same, so Step 7 does not apply.
  5. **The bench moved every row by 2–23%**, natives included, so the table moves to one date
     (Ruling T9-b) without implying retrace got faster. `echo`'s trace (32 → 28 MiB) is run-to-run
     variation: both the base and the M48 binary write about 28 MiB on one record and about 32 MiB
     on another (The bench, fix round 1).
  6. **Walk `e` counted 6 EL1 vector fall-throughs where t0 and the other walks count 5.** The count
     was the same on `e`'s record and both its replays, so it is deterministic per recording (a
     difference would be a divergence, `Box_::fall_throughs`). It varies between recordings and is
     not a divergence.
  7. **Five untracked traces sit in this directory, and they are not committed.** They are
     `sweep/automationmodetool.bin`, `csh.bin`, `dddiagnose.bin`, `tcsh.bin` and `yes.bin`: 402 384 068
     bytes (383.7 MiB), of which `yes.bin` is 369 929 439. The sweep keeps them
     (`RETRACE_SWEEP_KEEP`), and the evidence commits exclude `*.bin`. **They are to be deleted at the
     milestone close.**

**Open questions (not conclusions):**
- **The crash demo's store offset.** The release walk's own hit is at `0xa2d000948`, and the
  `MAP_JIT` RWX `mprotect` base is `0xa2d000000`, so the offset is `+0x948`. That equals Task 8's
  debug-build figure and differs from t0's release `+0x9a8` (§M7). So "release vs debug" is unlikely
  to explain the difference. Nothing documents the offset as a fact, and nothing here does either.
