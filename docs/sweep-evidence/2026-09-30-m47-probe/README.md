# 2026-09-30 probe: `git` and `node` under retrace (M47's brainstorming evidence)

This is the first time the 2026-07-05 vision spec's other two v1 targets were pointed at retrace. The
host is macOS 26.5.2 (25F84). Every run used `main` at `427fa0a`, the M46 merge. Two builds of that
commit were used:

- **the unpatched CLI**: a signed scratchpad copy of `cargo build -p retrace`, with sha256
  `20f6c7824678625ea3550bb8e8bde2b267ba456a72b1540b54d89581c205adfa`;
- **the scratch CLI**: the same commit with `probe-scratch.patch` applied in a detached worktree that
  was removed afterwards. **None of the patch landed.** Each hunk in it says `PROBE SCRATCH`.

The traces (14–355 MB) stayed in the session scratchpad and are not committed. The excerpts here were
cut from them with `td-main.rs`, a throwaway trace dumper over `retrace_trace::Reader`, and with
`retrace debug`.

| File | What it holds |
|---|---|
| `git-runs.txt` | Every git run: its stdout head, its panic, record-error or terminated line, and the replay compare. Tags: `git1`–`git4`, unpatched with `-C`; `git11`–`git15`, unpatched in the repo; `git21`–`git25`, scratch with `chdir`; `g30`–`g41`, scratch, the `add`/`commit` walk. |
| `commit-abort.txt` | `g35`: `git commit` with `madvise(…, 7)` forwarded. It holds libmalloc's message, from `x22` at the abort, the symbolicated frame chain, and the madvise landmark. |
| `node-amfi.txt` | `node -e 'console.log(1)'` on the unpatched CLI: dyld's own error line, the 521 panic, and every `__mac_syscall` (381) landmark with its EFAULT. |
| `node-kevent.txt` | `node` on the scratch CLI (AMFI fixed): the stop at `kevent` (363) after two `kqueue()` calls, the trap count and the thread count. |
| `amfi.c`, `amfi.out` | The native AMFI dyld-policy answer, `__mac_syscall("AMFI", 0x5a, …)`, for an ad-hoc binary with and without `retrace.entitlements`, for inFlags 0, 2, 4 and 6. |
| `amfi-disasm.txt` | dyld's `amfi_check_dyld_policy_self` (call `0x5a`, struct `{inFlags, outFlags*}`) and `SyscallDelegate::amfiFlags`, which returns 0 when the call fails. |
| `fork-disasm.txt` | libc `fork`: prepare handlers, then `__fork` (syscall 2), then the parent handlers on failure, which return −1 with errno set. |
| `jq-ab.txt` | The jq 300k abort, A/B with madvise forwarded and no-op'd. It aborts both ways, so it is **not** this bug. |
| `lt.c`, `lt-control.txt` | A minimal `localtime_r` guest. It records and replays cleanly, so git's abort needs git's heap history. |
| `sweep-427fa0a.log`, `sweep/` | The owed M46 T6-a re-sweep on an idle host: load 1.93 at the start, 1.85 at the end, 5 minutes, `TALLY pass=49 fail=5`. It uses the unpatched CLI and `sweep-run.sh`, and `sweep/` holds the non-clean rows' stderr. It is M47's sweep baseline. |
| `sandbox-call2.txt` | Added while the plan was being written. It covers both `__mac_syscall("Sandbox", 2, …)` callers (dyld's `sandbox_check_common`, which gets EFAULT, and libsystem_sandbox's `rootless_check_trusted_internal`, which gets EINVAL). For each it gives the argument struct, with nested guest pointers at +0 and +16, and the operation name at `*(arg + 16)`, which is the key R7's continuity rule uses (spec §11 item 5). |
| `sym2.py` | The symbolicator `sandbox-call2.txt` used: `sym.py` with the host's shared-cache slide added. |
| `gp.sh`, `sweep-run.sh`, `td-main.rs`, `sym.py`, `probe-scratch.patch` | The scripts and the scratch patch, so that each result can be re-run. |

## Findings

1. **Read-only git works on `main` today.** `--version`, `rev-parse HEAD`, `log -1` and
   `show --stat` record and replay with byte-identical stdout. Use Xcode's
   `/Applications/Xcode.app/Contents/Developer/usr/bin/git`, because `/usr/bin/git` is an xcrun
   shim, and a shim reaches M38's `posix_spawn` refusal.
2. **Five arg_kinds rows are missing:** `chdir` (12), `mkdir` (136), `link` (9), `rename` (128) and
   `utimes` (138). git calls `chdir` even without `-C`. With the rows added, `status` (short and
   long), `diff`, `-C` and `add` record and replay identically. The `utimes` path was reached only
   after a crashed run had left objects behind, so the minimal row set for a clean repo is
   unmeasured.
3. **`madvise` is unsafe to forward.**
   - `MADV_ZERO` (11) over 512 KiB (`g33`) trips the M30 guard band, which fails loudly.
   - A forwarded `MADV_FREE_REUSABLE` (7) is followed by a libmalloc abort in `tzload` (`g35`).
     Replay reproduces the abort identically, so the oracle cannot see it.
   - With 7 and 8 no-op'd (`g36`), the abort vanishes and the commit is written.
4. **Syscall 333 is `__pthread_canceled(2)`**, which is `pthread_setcancelstate(DISABLE)`, and not a
   timed wait. It comes before git's auto-maintenance fork, whose first sign is `mach_msg2` 3403
   (`g40`). With `-c maintenance.auto=false`, `commit` records to exit 0 and replays identically
   (`g41`). Neither 333 nor 334 appears.
5. **`__mac_syscall` (381) is forwarded with a nested out-pointer.**
   - AMFI `0x5a` writes through `outFlags*`, which lies inside the struct.
   - The forward returns EFAULT only because the guest stack VA falls in retrace's own
     `__PAGEZERO`. As a result `amfiFlags()` is 0 for every dyld guest, and dyld refuses every
     `@rpath`/`@executable_path`/`@loader_path` load.
   - Natively the answer is `0x1df` (inFlags 0).
   - `Sandbox` call 2 is also forwarded, returning EFAULT from dyld and EINVAL from a second caller.
6. **node** stops first at finding 5. Past it, node stops at `kevent` (363) on a guest `kqueue()`
   (libuv), after 1,101 traps, with no threads and no `MAP_JIT` yet.
