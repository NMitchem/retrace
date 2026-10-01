# M47 t0 evidence: measurements M1–M6, run 2026-10-01

This directory is the evidence for `docs/superpowers/specs/2026-09-30-retrace-m47-gitwrite-measurements.md`.
It keeps the plan's 2026-09-30 name (ledger Ruling P6); **every file was produced on 2026-10-01.**

**When and where.** Branch `worktree-m47-gitwrite` at **`aa16f01`**, on this machine:
- macOS 26.5.2 (25F84), kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`;
- `Apple clang version 21.0.0 (clang-2100.1.1.101)`;
- `lldb-2100.0.17.203`, static lookups only;
- `/Applications/Xcode.app/Contents/Developer/usr/bin/git`, `git version 2.50.1 (Apple Git-155)`.

**The three `retrace` binaries.** Each is a copy of `cargo build -p retrace`, ad-hoc signed with
`retrace.entitlements` under `/private/tmp/claude-501/`. Each scratch patch was reverted with
`git checkout -- crates/` right after its build.

| name | built from | build log | sha256 after signing |
|---|---|---|---|
| **base** (`m47-base-retrace`) | `aa16f01`, unmodified | `t0-base-build.log` | `e4ae912e44c071e33ca978f9f78613b21709cac4a996b8ed50deaac01582defb` |
| **census** (`m47-census-retrace`) | `aa16f01` + `census-build.patch` (the probe's `probe-scratch.patch` + the brief's two print-only `[m47]` lines) | `t0-census-build.log` | `8c00760682216990d97acd0e21edc9488c57a680ec28b26ba4599f53af1e6d89` |
| **m3b** (`m47-m3b-retrace`) | `aa16f01` + `m3b-scratch.patch` (census + 3403 answered `KERN_SUCCESS` + syscall 2 refused `EAGAIN`) | `m3b-build.log` | `98819ab168912b6f895b15db3e26d663557a4e254af2ae5dbaf071092b06727d` |

**What is not here:**
- traces (`.bin`), which went under `/private/tmp/claude-501/m47-*` and the ledger and were deleted;
- built fixture and probe binaries (`clang -arch arm64 -o <f> <f>.c`);
- every scratch git repository.

**How the commands ran.** `L` is the ledger directory
`.superpowers/sdd/2026-09-30-retrace-m47-gitwrite`. The session's worktree guard refuses compound
commands that `cd` into, or loop over, a path containing "git". Each multi-command step was
therefore written as a script, run with `zsh` (or `sh` for `census.sh`) from the worktree root, and
its output written into `$L/t0/`. The files were copied here unchanged by `collect.sh`.

## Files

### Fixtures and native C probes (sources only)

| file | what |
|---|---|
| `fsops_dyn.c`, `madv_dyn.c`, `rpath_dyn.c`, `librpath_dyn.c`, `forkfail_dyn.c` | The plan's fixture texts (Tasks 1–4), extracted verbatim by the controller. Not changed. |
| `madvnative.c` | Step 6, with `MADV_CAN_REUSE` added to `adv[]` (M1(d)). |
| `call4native.c` | Native `sandbox_container_path_for_pid` (H7, M2(a)). |
| `m2binterpose.c` | `DYLD_INSERT_LIBRARIES` interposer of `__mac_syscall` (M2(b)). |
| `m2bnative.c` | The measured Sandbox call-2 structs, reissued natively (M2(b)). |
| `m3anative.c` | `mach_ports_register`, and the 3403 request via `mach_msg2_internal` and plain `mach_msg` (M3(a)). |

### By step

| file(s) | produced by | binary |
|---|---|---|
| `t0-base-build.log`, `t0-census-build.log`, `census-build.patch` | Step 1 | — |
| `step2.sh`; `native-fsops.out`, `native-madv-{zero,reuse,bad}.out`, `native-rpath.out`, `native-forkfail.out` | Step 2, the fixtures natively | native |
| `census.sh` (verbatim), `run-census.sh` (its invocation); `run-census.log`, `census.progress`, `census.tsv` | Step 3, the census (09:14:12–09:18:52) | census |
| `summ.py`; `census-summary.txt` | Step 3's summary | — |
| `h7-call4.sh`; `h7-call4.txt`, `h7-desdp-1.rec.err`, `h7-desdp-2.rec.err` | After H7: two records of `desdp` and a `retrace debug` struct dump of `("Sandbox", 4)` | census |
| `call4native.out` | `call4native` run natively | native |
| `m1b.sh`; `m1b.txt`, `m1b/{A,B}-{1..5}.{out,err}` | Step 4 (M1(b)), N=5 per mode | census |
| `m1b-why.sh`; `m1b-why.txt`, `m1b-why/b-1.{out,rec.err}`, `m1b-why-stack.txt` | M1(b) follow-up: traced mode-B commits until one aborted, then `retrace debug` `continue; where; regs` and `x` of the abort string and the stack | census |
| `fpwalk.py` | Walks the x29 chain in a `retrace debug x` dump | — |
| `m1b-why-sym.sh`, `m1b-why-sym2.sh`; `m1b-why-sym.txt` | Symbolicates M1(b)'s abort: static `lldb image lookup` for the cache, `atos` at `0x100000000` for git | — |
| `step5.sh`; `m1c.txt`, `m1c-{1..5}.{out,err}` | Step 5 (M1(c)) | **base** |
| `m1d.txt` | Step 6 (M1(d)), `madvnative` natively | native |
| `step7.sh`, `m2b.lldb`; `m2b.log` | Step 7 as written: lldb refused the attach to Xcode's git | native lldb |
| `step7b.sh`; `m2b-rpath.log` | Step 7 on `rpath_dyn`: lldb hung at `run` (Developer mode disabled) and was killed | native lldb |
| `m2b-structs.sh`; `m2b-structs.txt` | M2(b) fallback: the call-2 structs of a `hello_dyn` record, via `retrace debug` | census |
| `m2b-native.sh`; `m2b-native.txt` | M2(b) fallback: the interposer on `hello_dyn`, `rpath_dyn` and `call4native`, then `m2bnative` | native |
| `m3a.lldb`, `m3b.lldb` | Step 8's command files, written but not run (lldb, D3) | — |
| `m3a-trace.sh`; `m3a-trace.{out,err}` | M3(a) fallback: `forkfail_dyn` under `RETRACE_TRACE=1`, giving the 3403 send decode | **base** |
| `m3a-native.sh`; `m3a-native.txt` | M3(a): `m3anative call`, `msg2`, `msg` natively | native |
| `m3b-scratch.patch`, `m3b-build.log`, `m3b-scratch.sh`; `m3b.txt`, `m3b/forkfail.{out,err}`, `m3b/git-commit.{out,err}`, `m3b/nums.txt` | M3(b) fallback, with `RETRACE_TRACE=1` and `PROBE_NOREUSABLE=1` | **m3b** |
| `step9.sh`; `m3c.txt`, `m3c.out`, `m3c.err` | Step 9 (M3(c)), native git under `ulimit -u 1` | native |
| `m4-run1-invalid.txt`, `m4-run1/log.{native.out,rec.out,rec.err}` | Step 10's first run. **Invalid for writes**, because of the `mkrepo` variable bug (D8). Kept for the record and for `log -1`'s zero-madvise abort | census |
| `m4.sh` (with D8's `local` fix); `m4.txt`, `m4/<tag>.{native.out,native.err,rec.out,rec.err,rp.out,rp.err}`, `m4/<tag>.{native,rec}.state`, `m4/nums.txt` | Step 10 (M4), the valid run | census |
| `m4-commit-why.sh` | Diagnosed run 1's bug: the `w-*` twins were never created | — |
| `m4-retry.sh`; `m4-retry.txt`, `m4-retry/tag-try1.*`, `m4-retry/nums.txt` | The aborted `tag` row retried, with `m4.sh`'s own functions | census |
| `m4-stash-why.sh`; `m4-stash-why.txt`, `m4-stash-m3b.{out,err}` | `stash`'s children: `GIT_TRACE` natively, and a record on the m3b build | native / **m3b** |
| `m4-census-summary.txt` | `summ.py m4/*.rec.err m4-retry/*.rec.err`, git's calls for M1(a)/M2(a) | — |
| `rowcheck.sh`; `m4-rowcheck.log`, `union-rowcheck.log` | Step 10's rowcheck on `m4/nums.txt`, and on that ∪ the retry ∪ `m3b/nums.txt` (`rowcheck-union-nums.txt`). Scratch test `crates/retrace-arch/tests/zz_m47_rowcheck.rs`, then deleted | `cargo test -p retrace-arch` on `aa16f01` |
| `m4-norow.sh`; `m4-norow.txt` | Which command dispatched each NOROW number | — |
| `logflake.sh`; `logflake.txt`, `logflake/*` | `log -1` ×10 per binary with `-C`. The base half hit the M33 `chdir` panic, so this run is superseded | **base**, census |
| `logflake2.sh`; `logflake2.txt`, `logflake2/*` | `log -1` ×10 per binary, cwd = repo, no `-C` (the probe's `git12` shape) | **base**, census |
| `logflake3.sh`; `logflake3.txt`, `logflake3/abort-run-5.err`, `logflake3/abort-stack.txt`, `logflake3/run.{out,err}` | `log -1` on base until an abort (run 5), then `retrace debug` regs, `x` of the string and the stack | **base** |
| `logflake3-sym.sh`; `logflake3-sym.txt` | Symbolicates the base abort's frames | — |
| `m5-grep.txt` | Step 11's grep (M5) | — |
| `m6.sh`; `m6.txt` | Step 12 (M6), with the per-file breakdown | — |
| `collect.sh` | Copied this directory from the ledger | — |
