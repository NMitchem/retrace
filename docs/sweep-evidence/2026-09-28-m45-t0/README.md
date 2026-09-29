# t0 evidence — M45 Task 0, run 2026-09-28

The kept files behind `docs/superpowers/specs/2026-09-28-retrace-m45-kqinit-measurements.md`. The
directory is named for the day t0 ran, as M37–M44's are. No trace (`.bin`) is committed: every
recording these files came from was a scratch file `/private/tmp/claude-501/m45-*.bin`.

## Method and binaries

Branch `worktree-m45-kqinit` at **`a78f28f`** (the M45 plan commit), in the worktree
`.claude/worktrees/m45-kqinit`. Host: macOS 26.5.2 (25F84), kernel
`xnu-12377.121.10~1/RELEASE_ARM64_T6041`, `Apple clang version 21.0.0 (clang-2100.1.1.101)`.
Every command ran on 2026-09-28.

- **M1 and M2's recordings ran a throwaway build**: `a78f28f` plus four lines in
  `crates/retrace-core/src/lib.rs`, inside `record_box`'s `if trace_log { if let Stop::Syscall
  { num, args } = &stop {` block, directly after the `[trap]` `eprintln!`:

  ```rust
  if *num == 374 {
      let n = 72 * (args[2] as u32 as usize).min(8);
      eprintln!("[kevent_qos] thread={thread} entry={:02x?}", b.read_va_prefix(args[1], n));
  }
  ```

  No row and no arm for 374 was added. `target/aarch64-apple-darwin/debug/retrace`, built with
  `cargo build -p retrace` and ad-hoc signed by `tools/codesign-run.sh` on each run: sha256
  `897cd5f9b6c86b70fca2d73ef90a27fa52cbf3b206f9e90548dce1b4f23cce26` (hashed after the M2 runs).
  The edit was restored with `git checkout -- crates/retrace-core/src/lib.rs` before anything was
  committed, and `git status --short` then printed nothing.
- **M2's candidates, M3's fixture and M4 ran natively**, not under retrace. The candidates and the
  fixture were built with `clang -arch arm64 -o <bin> <src>.c`.
- **M5 reads the source tree at `a78f28f`**; nothing was built for it.

## Files

| file | command that produced it | what it shows |
|---|---|---|
| `m1-automationmodetool.err` | `export RETRACE_TRACE=1`, then `tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /usr/bin/automationmodetool -o /private/tmp/claude-501/m45-amt.bin` (throwaway build; exit 101, printed by `echo "exit=$?"` and not kept) | the recorder's stderr: 359 `[trap]` lines, the 374 trap and its `[kevent_qos]` dump, then the M33 panic |
| `m1-vs-m44-traps.diff` | `diff` of the `[trap] num=… pc=…` prefixes (arguments cut) of M44's `2026-09-27-m44-t0/m1-automationmodetool.err` against this directory's | the two runs differ by two `gettimeofday` (116) traps only |
| `m2-timer.c`, `m2-after.c`, `m2-signal.c` | the brief's candidate sources, written verbatim | a `DISPATCH_SOURCE_TYPE_TIMER` source, `dispatch_after`, a `DISPATCH_SOURCE_TYPE_SIGNAL` source |
| `m2-native.log` | `for c in timer after signal; do clang -arch arm64 -o $L/t0-m2/$c $L/t0-m2/$c.c; echo "$c build=$?"; $L/t0-m2/$c > $L/t0-m2/$c.native.out; echo "$c native=$?"; done`, then `od -c` of each `.native.out` | each candidate's build and native rc. `signal` hung and was killed with SIGTERM after more than 120 s, hence its `native=143` and its empty `od`. The last line, `[exited with code 0]`, is the session harness's, not the script's |
| `m2-timer.native.out`, `m2-after.native.out`, `m2-signal.native.out` | the native run above | each candidate's native stdout (`signal`'s is empty) |
| `m2-signal-repeat.log` | five native runs of `signal`, each `perl -e 'alarm 10; exec @ARGV' $L/t0-m2/signal`, printing rc and stdout bytes (not in the brief; added because the first native run hung) | 5 of 5 hit the 10 s alarm (rc 142) with 0 stdout bytes |
| `m2-timer.err`, `m2-after.err`, `m2-signal.err` | `export RETRACE_TRACE=1`, then `tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0-m2/$c -o /private/tmp/claude-501/m45-$c.bin` (throwaway build) | each candidate's recorder stderr, ending at its 374 and the M33 panic |
| `m2-record.log` | the brief's record loop: each candidate's `record=$?`, its `[trap] num=374`/`[kevent_qos]` lines, and its last `[trap]` line | all three exit 101 at 374 |
| `m2-compare.log` | a script comparing each candidate's 374 `[trap]` line (with `x1` masked) and its `[kevent_qos]` entry against M1's as text; it also counts each file's 374 and `[trap]` lines, and prints the three traps before 374 and the panic line | all three equal M1's, on thread 0 |
| `m3.log` | `clang -arch arm64 -o $L/t0-m3/kqinit_dyn $L/t0-m3/kqinit_dyn.c; echo "build=$?"`, then for modes `""` and `straddle`: `$L/t0-m3/kqinit_dyn $m > $L/t0-m3/native-${m:-plain}.out 2>&1; echo "mode '${m}' rc=$?"; cat …` | build 0; both modes rc 0 |
| `m3-native-plain.out`, `m3-native-straddle.out` | the M3 runs above | `kqinit rc=0 carry=0` |
| `m4.log` | `/usr/bin/automationmodetool </dev/null > $L/t0-m4-native.out 2> $L/t0-m4-native.err; echo "rc=$?"`, then `wc -c`, `head -3` of each | rc 0, 97 stdout bytes |
| `m4-native.out`, `m4-native.err` | the M4 run above | its stdout (two lines) and its stderr (empty) |
| `m5-test-count.txt` | `grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' \| awk -F: '{s+=$2} END {print s}'` at `a78f28f`, then the same `grep` per file (non-zero files, sorted) | total 839, and the per-file counts it sums |

`$L` is `.superpowers/sdd/2026-09-28-retrace-m45-kqinit` in the worktree (the ledger, not
committed). M3's source `$L/t0-m3/kqinit_dyn.c` is the controller-ruled fixture
(`kqinit_dyn.ruled.c`, Ruling P1: `badptr` passes `1ull << 47`), copied unchanged: sha256
`5001f2c326742e086fa6cb959b7c3dd489465431a39fb96c8d3ce17c98435ed1`. Task 2 commits the identical
text as `crates/retrace-guest/c/kqinit_dyn.c`, so it is not copied here.
