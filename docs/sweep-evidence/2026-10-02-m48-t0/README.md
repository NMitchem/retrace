# M48 t0 evidence: the measurements behind M1–M9, 2026-10-03

This directory is the evidence for M48 Task 0. The measurements file that reads it is
`docs/superpowers/specs/2026-10-02-retrace-m48-node-measurements.md`. The directory keeps the plan's
2026-10-02 name; **every file outside `probe/` was produced on 2026-10-03**.

**Where it ran.** This machine: Apple M4 Pro, macOS 26.5.2 (25F84), kernel
`xnu-12377.121.10~1/RELEASE_ARM64_T6041`, `Apple clang version 21.0.0 (clang-2100.1.1.101)`,
Homebrew node 25.6.1 (`/opt/homebrew/Cellar/node/25.6.1/bin/node`, libuv 1.52.1, V8
14.1.146.11-node.19). Branch `worktree-m48-node` was at **`50e716f`** for every run, and its
`crates/` has no diff from `e6caa65`, where the probe was taken.

**What is not here:** traces (`.bin`), built binaries (the fixtures, the native probes, the addon,
the interposer dylib), and the scratch worktrees. Each was deleted after use.

**Capping.** The five `m2-<walk>.err` files keep their last 400 lines, under a first line
`[capped: the last 400 of N lines of <source path>]`. Each `m2-<walk>.probe.txt` is the uncapped
extract the censuses read (below). Every other file is whole.

## The `retrace` binaries

Each is a signed copy (`codesign -s - -f --entitlements retrace.entitlements`) under
`/private/tmp/claude-501/`. sha256 values were taken after signing.

| name | build | source | sha256 |
|---|---|---|---|
| **base** `m48-base-retrace` | `cargo build -p retrace` (debug) in the m48 worktree | `50e716f` | `7b309adc92652421aff14ac8345c5a6adc324cc901471f0d03479c714df5027c` |
| **walk** `m48-walk-retrace` | `cargo build --release -p retrace` in scratch worktree `m48-t0-walk` | `50e716f` + `probe/probe-final.patch` | `81689f39bb893b588f94ceae126cbbd037acc542de9d39cb7bdc8d71447b31ad` |
| **baserel** `m48-baserel-retrace` (t0 addition) | `cargo build --release -p retrace` in `m48-t0-walk`, before the patch | `50e716f` | `20f8ea5141084017d9ffa16b37cf19f5dd6c12723be0dc7948e1f86ed5485d27` |
| **walkdbg** `m48-walkdbg-retrace` (t0 addition) | `cargo build -p retrace` (debug) in `m48-t0-walk`, after the patch | `50e716f` + `probe/probe-final.patch` | `563bee286427a5194a32d3ddfb7f6a0281de811a5b3f186c4f931d7fea8c5d66` |
| **hvopt** `m48-hvopt-retrace` (t0 addition) | `cargo build -p retrace` (debug) in scratch worktree `m48-t0-hvopt` | `50e716f` + `[profile.dev.package.hv-sys] opt-level = 1` appended to the workspace `Cargo.toml` | `c8e4c621031088e398d81bd1a8d3966aefb0ad0b3e7caae3f41c7a9ee9247ae8` |

Build logs: `t0-base-build.log`, `t0-baserel-build.log`, `t0-walk-build.log`,
`t0-walkdbg-build.log`, `t0-hvopt-build.log`. Each scratch worktree was removed with
`git worktree remove --force`; `git worktree list` no longer showed it and `git status --short` in the
m48 worktree was empty each time.

## Files

### `probe/` — the controller's scratch probe, copied in (brief Step 1)

`walls.md`, `probe-final.patch`, `run.sh`, `census.sh`, `savepatch.sh`, `native/` (the two replicas'
sources and outputs), `logs/` (the probe's own walk logs) and `crash/` (the addon source, `crash.js`,
`crash.json`), copied unchanged from
`/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/4b336710-6279-49f2-be58-212ed366d476/scratchpad/m48-probe/`
on 2026-10-03; their own dates are 2026-10-02. The built `kqdetect`, `kqpipe` and
`crash_addon.node` were not copied. sha256: `probe-final.patch`
`dd941ed40c15128bd08361ee1462dec405733c724802c3b5d31f5f070bf66101`, `walls.md`
`a67513f59855aa803a2ef1cc2efef761defb2915406e0e85f277eacb07ae5259`. `probe.patch` beside it in the
scratchpad has the same hunks in a different file order (sha256 `f882525d…`), so it is not
byte-identical; it was not copied.

### M1 — SPRR and EL0 cache maintenance (base binary; native)

| file | produced by |
|---|---|
| `sprrprobe.s`, `sprrprobe-msr.s` | brief Step 3's heredoc and `sed '/ic  *ivau/d'` |
| `m1-sprrprobe.log`, `m1-sprrprobe-msr.log` | `base record <probe> -o …` (Step 3), alarm 60 |
| `m1-icache.txt` | `xcrun dyld_info -arch arm64e -disassemble /usr/lib/system/libsystem_platform.dylib`, `_sys_icache_invalidate` to `_sys_dcache_flush` |
| `sprr.c`, `m1-sprr-native.out` | `docs/sweep-evidence/2026-10-02-m48-static/sprr.c` plus one line (a second `pthread_jit_write_protect_np(1)` and a read), `cc -O1`, run natively |

### M2 — the walks (walk binary)

| file | produced by |
|---|---|
| `walk.sh` | brief Step 6's text, plus `secs=` on each status line (Step 9 needs the record time) |
| `census.sh` | `probe/census.sh` through brief Step 6's `sed` |
| `m2-<walk>.out`, `.err` (capped), `.status`, `.rp1.out/.err`, `.rp2.out/.err` | `bash walk.sh <walk> -- <node args>` for `e`, `t10`, `t2000`, `natives`, `crash` (Step 6) |
| `m2-<walk>.census` | `bash census.sh <walk>` |
| `extract.sh`, `m2-<walk>.probe.txt` | t0's extractor over the uncapped `.err`: every `[probe]`, `[fault]`, refusal and forwarding line, plus the trap lines for 363, 301–305, 312, 73, 197, 74, 75, 105, 32, 360, 6, 399, 362, −36 and −33, each prefixed `T<n>`, the ordinal of the `[trap]` line it follows |
| `munmap-census.sh`, `m2-e.munmap.txt`, `m2-crash.munmap.txt` | t0's classifier over the `partial munmap` lines (head / tail / interior, unaligned end, past the backing) |

### M3 — kevent native replicas (native)

| file | produced by |
|---|---|
| `kqprobe.c`, `m3-kqprobe.out` | brief Step 7's text, `cc -O1`, `./kqprobe \| cat` |
| `m3-kqdetect.out`, `m3-kqpipe.out` | `probe/native/kqdetect.c` and `kqpipe.c` rebuilt and re-run; `diff` against the probe's outputs is empty |

### M4 — psynch (native)

| file | produced by |
|---|---|
| `m4-pin.txt` | `dyld_info -load_commands` on `libsystem_pthread.dylib`; `git clone --depth 1 --branch libpthread-539.100.4` of `apple-oss-distributions/libpthread` and `shasum -a 256` of three files; `sw_vers`, `uname -v`, `sysctl` |
| `m4-stubs.txt` | brief Step 8's `dyld_info -disassemble` of `libsystem_kernel.dylib` |
| `cvprobe.c` | brief Step 8's text, `cc -O1` |
| `m4.lldb`, `m4-lldb-launchtest.out` | the brief's lldb script; **lldb could not launch any process** (`run` hangs until the alarm, exit 142, on `cvprobe` and on `kqprobe` alike) |
| `m4interpose.c`, `alarmexec.c` | t0's replacement instrument: a `DYLD_INSERT_LIBRARIES` dylib interposing the psynch stubs (it issues the same `svc` itself and logs the raw `x0` and carry), and a non-SIP `alarm; execvp` (`perl` is SIP-protected, so dyld strips `DYLD_*` from it and its children) |
| `m4-waitsignal.out`, `m4-broad3.out`, `m4-timeout.out`, `m4-onens.out`, `m4-layout.out` | `./alarmexec 120 ./cvprobe <mode>` with the interposer inserted |
| `m4-node-onens.out` | `./alarmexec 300 node -e 'console.log(1)' < /dev/null` with the interposer, stdout piped |
| `m4-uv-cond-destroy.txt` | `otool -tV` of `_uv_cond_destroy` in Homebrew libuv 1.52.1, and its `{0, 1}` constant |

### M5–M7 — JIT, threads, crash demo (walk and walkdbg binaries; native)

| file | produced by |
|---|---|
| `m5-debug-record.out/.err`, `m5-debug-replay.out/.err` | `/usr/bin/time -l` over `walkdbg record-dyn node -e 'console.log(1)'` and its `replay` |
| `thread-timeline.sh`, `m6-e.timeline.txt`, `m6-t2000.timeline.txt`, `m6-crash.timeline.txt` | t0's timeline over the uncapped `.err` (lines with `tid=` are the probe's; trap lines show `tid=?`) |
| `m7-native.out` | `node --allow-natives-syntax probe/crash/crash.js <addon> probe/crash/crash.json`, natively |
| `m7-debug.out`, `m7-debug.err` | `/usr/bin/time -l` over `walk debug m48-crash.bin --script "continue; watch <cell>; reverse-continue; x <cell> 8; stepi; x <cell> 8"` (Step 9) |
| `m7-where.rec.out/.err`, `m7-where.out/.err` | a second crash recording on the walk binary and `debug --script "continue; where; threads; watch <cell>; reverse-continue; where; x <cell> 8"`, for the crash's thread (t0 addition) |

### M9 — the SIMD fixture and Task 1's tests

| file | produced by |
|---|---|
| `simd_dyn.c`, `m9-native.out` | the plan's Task 1 Step 5 text, byte for byte; `clang -arch arm64`; run natively |
| `m9-base-<mode>.out/.err`, `m9-walk-<mode>.out/.err` | brief Step 5's loop on base, then on walk |
| `m9-baserel-<mode>.out/.err` | the same loop on baserel (t0 addition) |
| `simd_dyn_sa.c`, `m9sa-native.out`, `m9sa-<bin>-<mode>.out/.err`, `.rp.out/.err` | t0's candidate: `simd_dyn.c` with the signal mode on `sigaction(SA_SIGINFO)`; recorded and replayed on base, walk, baserel, walkdbg and hvopt (hvopt record only) |
| `m9-hvsys-test.rs`, `m9-hvsys-fix.patch`, `m9-hvsys-*.log` | a scratch package `/private/tmp/claude-501/m48-t0-simdtest` running the plan's Task 1 Step 2 test against the worktree's `crates/hv-sys` (`base`) and a copy carrying the probe patch's hv-sys hunk (`fixed`), on dev, `--release`, and dev with `[profile.dev.package.hv-sys] opt-level = 1/2/3` |
| `m9-simdctx-*.log` | the plan's Task 1 Step 3 `simdctx.rs`, placed in scratch worktree `m48-t0-simdctx` and run with `cargo test -p retrace-box --test simdctx`: base dev, base with hv-sys opt-level 1, base `--release`, and the same three with the hv-sys hunk applied |

### M8 — the base counts

Not a file: brief Step 10's three commands, quoted in the measurements file.
