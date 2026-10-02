# M47 abort evidence: the `mach_vm_map` mask diagnosis and the post-fix validation, 2026-10-01

This directory is the evidence for M47 Task 3b: retrace ignored `mach_vm_map`'s alignment mask, so
libmalloc's xzone segment landed off a 4 MiB boundary and git intermittently aborted with "pointer
being freed was not allocated". **`abort-diagnosis.md` is the diagnosis** (agent m47-abort), copied
unchanged from the ledger. Its `$A/` is `/private/tmp/claude-501/m47-abort/`. The fix is commit
`4915a18` and the `TRACE_MAGIC` bump to `RT\x00\x0b` is `0d8a0a2`.

The directory keeps the plan's 2026-09-30 name (ledger Ruling P6). **Every file was produced on
2026-10-01.**

**Where it ran.** This machine: macOS 26.5.2 (25F84), with
`/Applications/Xcode.app/Contents/Developer/usr/bin/git`, `git version 2.50.1 (Apple Git-155)`.

**What is not here:**
- traces (`.bin`), including `abortA.bin`, `cleanA.bin`, `commit-abort.bin` and every run set's;
- the scratch repositories;
- the source copies (`src`, `src-fix`, `src-mut`, `src-cfix`, `src-fixdiag`). `fix.diff` is the fix
  as the diagnosis wrote it, against `090bf5e`.

**Capping.** A log with more than 400 lines keeps its last 400, under a first line
`[capped: the last 400 of N lines of <source path>]`. 18 files are capped: `gate-fix.log`, and the
RETRACE_TRACE logs and `.seq` files under `l1/` and `l2/`. Three files are not logs and are kept
whole: `abort-free.regs` and `clean-free.regs` (about 4,500 lines each; M1's divergence is at step
60, near the start), and `tzload.dis` (587 lines). Every other file is under 400 lines and was
copied as-is.

## The `retrace` binaries

Each binary is a debug `cargo build -p retrace`, ad-hoc signed with `retrace.entitlements`. The
sha256 values were taken on 2026-10-01 from the files on disk. The run logs do not record which
binary produced them, so the run-set → binary column is the diagnosis's attribution. Where the
diagnosis names only "a diagnostic build", the binary is inferred from build time against run time,
and the table says so. The cargo logs of the diagnosis's builds are here as `build0.log`–`build3.log`,
`build-fix.log`, `build-fixdiag.log` and `build-cfix.log`.

| binary | built from | sha256 |
|---|---|---|
| `m47-base-retrace` (base) | `aa16f01` (t0's base) | `e4ae912e44c071e33ca978f9f78613b21709cac4a996b8ed50deaac01582defb` |
| `m47-census-retrace` (census) | `aa16f01` + `census-build.patch` | `8c00760682216990d97acd0e21edc9488c57a680ec28b26ba4599f53af1e6d89` |
| `$A/rt0` | `$A/src`, the diagnosis's source copy, unpatched (`build0.log`, 18:26) | `9301e7717ca66ffdc71e3f2cf715730b839bfc74d295e71b2041bfba3a365ce0` |
| `$A/rt-vmlog` | `$A/src` + `diag-core.patch` (placement log `[vmlog]`; `build1.log`, 18:34) | `1e93ddf3d04d6c266077ca10396c8099a398e966a5114ba629e9b5464fdc90b4` |
| `$A/rt-ent` | the above + `diag-box.patch` (`RETRACE_FIXENTROPY`, `RETRACE_FIXTIME`; `build3.log`, 18:40) | `5f61a1a0ed6794321d3ba681dae85d855e76d75cd05b9646c2e78180defc00ea` |
| `$A/rt-fix` | `$A/src-fix` = `090bf5e` + `fix.diff` (`build-fix.log`, 18:45) | `43f00cafab5b41d88b40c0374fd83e54262c16b18f68aedc14148883e3d37a8e` |
| `$A/rt-fixdiag` | the fix + both diagnostic patches (`build-fixdiag.log`, 18:45) | `96b0777383685d6bffce7f2f1036f7ea57619845feb8211523cf54d81f2874f7` |
| `$A/rt-cfix` | the fix + `census-build.patch` (`build-cfix.log`, 18:50) | `75afceb56195083c3c3a1c726ce554082ff6b37f534389eaf796298c938691ec` |
| **`/private/tmp/claude-501/m47-t3b-retrace`** (t3b) | **`0d8a0a2`** (the fix + the magic bump) | **`156a0e85851a43a623d7e97bb6c884f08ece857cd154deee7ed6d7fae5166450`** |

## Files

### The diagnosis's reproduction and run sets (`git log -1`, cwd = `$A/repo`, no `-C`)

| file(s) | command | binary |
|---|---|---|
| `mkrepo.sh`, `native-log.out` | `zsh mkrepo.sh`: the one-commit repo and its native `git log -1` | none (Xcode's git) |
| `l1/`, `l2/` (`run-*.err` are RETRACE_TRACE logs; `run-*.seq`) | `loop.sh <bin> $A/l1 <N>` with `RETRACE_TRACE=1` | "base", per the diagnosis. That is `m47-base-retrace` or `rt0`: the runs started at 18:27, a minute after `rt0` was built, and the logs do not say which |
| `vm1/` | `loopvm.sh <bin> $A/vm1 12` with `RETRACE_VMLOG=1` | `rt-vmlog` (inferred: built 18:34:55, run 18:35–18:36) |
| `ent1/` | `loopent.sh <bin> $A/ent1 <seeds>` (getentropy pinned) | the entropy-only build `build2.log` made at 18:36:55. `build3.log` overwrote it at 18:40:01 with `rt-ent`, so it has no surviving hash |
| `ent2/`, `ent3/`, `ent3.txt` | `loopent2.sh <bin> <dir> time <seeds>` (getentropy and gettimeofday pinned) | `rt-ent` (inferred: run 18:40–18:44) |
| `ctl-base/` | `loopent2.sh` on seeds 16 ×3, 28, 34, 39 | `rt-ent` (inferred: "diagnostic build without the fix") |
| `ctl-fix/` | the same seeds | `rt-fixdiag` |
| `rule.py` | `python3 rule.py`: the prediction rule over the surviving placement logs | none |

### The free path (diagnosis M1–M4)

| file | what | binary |
|---|---|---|
| `pctrace.sh`, `abort-free.regs`, `clean-free.regs`, `pcs.py` | `pctrace.sh <bin> {abortA,cleanA}.bin <bp> <hits> 400 regs`, then `pcs.py` diffs the two | the `debug` CLI of the build named in the diagnosis's M1 |
| `tzload.dis`, `lldbdis.sh` | static `lldb` over Xcode's git | none |
| `gtod-stack.txt`, `fpwalk.py` | the frame walk at the `gettimeofday` before the first 4811 reservation (M4) | as M1 |
| `diag-core.patch`, `diag-box.patch`, `census-build.patch` | the diagnostic patches and t0's census patch | — |
| `commit-abort.sh` | records `commit` on the census binary until one aborts | census |

### The diagnosis's fix, guards and gate (all against `090bf5e`)

| file | command | binary |
|---|---|---|
| `fix.diff`, `vmalign_dyn.c` | `git diff` inside `$A/src-fix`; the fixture | — |
| `t-vmalign-unit.log`, `t-vmalign-e2e.log`, `t-vmalign-guest.log` | the guards, `cargo test … -- --test-threads=1` in `$A/src-fix` | cargo-built |
| `t-mut-unit.log`, `t-mut-e2e.log` | the same guards in `$A/src-mut` (both `let m` lines ignore the mask) | cargo-built |
| `clippy-fix.log` | `cargo clippy --workspace --all-targets -- -D warnings` in `$A/src-fix` | — |
| `gate-fix.log`, `gatesum.py` | `cargo test --workspace --no-fail-fast -- --test-threads=1` in `$A/src-fix` (904/0/9); `gatesum.py` sums it | cargo-built |
| `oldtrace-on-fix.err`, `.out` | `rt-fix replay $A/cleanA.bin`: exit 3, `DIVERGENCE at landmark 524 … mach_vm_map ipa mismatch` | `rt-fix` |

### The diagnosis's validation

| file(s) | command | binary | tally |
|---|---|---|---|
| `val-log.sh`, `val-log-base.txt`, `val-log-base/` | `val-log.sh <bin> $A/val-log-base 30 no` | base | 30 runs, **2 aborts** |
| `val-log-fix.txt`, `val-log-fix/` | `val-log.sh <bin> $A/val-log-fix 30 yes` | `rt-fix` | 30 runs, 0 aborts, replay identical 30/30 |
| `val-commit.sh`, `vc-census.txt`, `vc-census/` | `val-commit.sh <bin> $A/vc-census 20 no` | census | 20 runs, **9 aborts** |
| `vc-cfix.txt`, `vc-cfix/` | `val-commit.sh <bin> $A/vc-cfix 20 yes` | `rt-cfix` | 20 runs, 0 aborts, `fsck` clean |

### Task 3b's validation (step 4), on `m47-t3b-retrace` (`0d8a0a2`)

Each run gets a fresh repo made by Xcode's git under `env -i`, and git is pointed at it with
`-C <repo>`. The scripts are adapted from the diagnosis's `val-log.sh` and `val-commit.sh`. A trace
is kept only for a run that is not clean, and none was kept.

| file(s) | command | tally |
|---|---|---|
| `t3b-val-log.sh`, `t3b-val-log/` (`tally.txt`; per run `native-i.out`, `run-i.{out,err}`, `rp-i.{out,err}`) | `zsh t3b-val-log.sh m47-t3b-retrace /private/tmp/claude-501/m47-t3b-val/log 20`: `git -C <repo> log -1`, recorded then replayed | `N=20 aborts=0 stdout_mismatch=0 replay_failures=0`: record stdout == native 20/20, replay rc 0 and stdout == record 20/20 |
| `t3b-val-commit.sh`, `t3b-val-commit/` (`tally.txt`; per run `run-i.*`, `rp-i.*`, `fsck-i.out`) | `zsh t3b-val-commit.sh m47-t3b-retrace /private/tmp/claude-501/m47-t3b-val/commit 10`: `git -C <repo> -c user.name=r -c user.email=r@x.invalid -c maintenance.auto=false commit -q -m m`, recorded then replayed; then `git fsck --strict` | `N=10 aborts=0 no_commit=0 fsck_unclean=0 replay_failures=0`: 10/10 committed, `fsck --strict` exit 0 with no output 10/10, replay rc and stdout == record 10/10 |
| `oldtrace-on-t3b.err`, `.out` | `m47-t3b-retrace replay $A/cleanA.bin` (a pre-M47 `RT\x00\x0a` trace) | exit 3, `DIVERGENCE at landmark 0 pc=0x0: empty/torn trace: no readable records`. The bump refuses the trace at open, where `rt-fix` without the bump diverged at landmark 524. |
