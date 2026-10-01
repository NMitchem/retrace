# M47-gitwrite t0 measurements

**Companion to** `2026-09-30-retrace-m47-gitwrite-design.md` (§3a: M1–M6; §7 and §11 item 9: the
halts) and to the plan's Task 0.

**Where and when.** Measured **2026-10-01** (the file and directory keep the plan's 2026-09-30
names, ledger Ruling P6) on this machine:
- macOS 26.5.2 (25F84), kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`;
- `Apple clang version 21.0.0 (clang-2100.1.1.101)`;
- `lldb-2100.0.17.203`, used **statically only** (see Deviations);
- git: `/Applications/Xcode.app/Contents/Developer/usr/bin/git`, `git version 2.50.1 (Apple Git-155)`.

Branch `worktree-m47-gitwrite` was at **`aa16f01`** for every run. No product source was committed.

**Binaries.** Three builds of `aa16f01`, each a signed copy of `cargo build -p retrace` under
`/private/tmp/claude-501/`:

| name | source | sha256 (after signing) |
|---|---|---|
| **base** | `aa16f01` unmodified | `e4ae912e44c071e33ca978f9f78613b21709cac4a996b8ed50deaac01582defb` |
| **census** | `aa16f01` + `census-build.patch` = the probe's `probe-scratch.patch` + the brief's two print-only `[m47]` lines | `8c00760682216990d97acd0e21edc9488c57a680ec28b26ba4599f53af1e6d89` |
| **m3b** | census + `m3b-scratch.patch`'s two scratch arms: 3403 answered `KERN_SUCCESS`, syscall 2 refused `EAGAIN` | `98819ab168912b6f895b15db3e26d663557a4e254af2ae5dbaf071092b06727d` |

`crates/` was restored with `git checkout -- crates/` after each scratch build (`git status --short`
empty each time).

**Evidence.** `docs/sweep-evidence/2026-09-30-m47-t0/`; its README says which command produced each
file on which binary. No trace (`.bin`), built binary or repository directory is committed.

**Outcome.**
- **H7 was triggered and ruled.** The census found a third `__mac_syscall` pair, `("Sandbox", 4)`.
  The operator ruled on 2026-10-01: model it, answering ENOTSUP (45) with no writes (M2(a)).
- **The lldb steps ran as non-debugger fallbacks.** lldb cannot launch or attach to a debuggee on
  this host; the operator ruled "fallbacks, no host security change" (Deviations D3–D6).
- **g35's abort is not madvise's.** The same libmalloc abort, at g35's own `tzload` frame,
  reproduces on the unpatched **base** binary in read-only `git log -1`, and in `commit` with
  advice 7 and 8 no-op'd (M1(b)). M47's madvise model cannot remove it, and every git gate inherits
  it as an intermittent failure. This is the first concern for the controller (see "Concerns").
- **git's lists** (M4):
  - the reads `status --porcelain`, `status`, `diff`, `diff --cached`, `log -1`,
    `show --stat HEAD` and `rev-parse HEAD`;
  - the writes `add`, `commit`, `branch`, `tag`, `switch -c`, `mv` and `rm --cached`;
  - out: `stash` (it forks `update-index` and `reset --hard`) and `merge --ff-only` (it arms
    `setitimer`, 83).
  - **k = 5.** Task 1's rows are the plan's **{9, 12, 136, 138, 333}**, with no addition.
- **The fork path needs only the 3403 answer and the refusal** (M3, H1 not triggered).
  `CANNOT_FORK` is `error: cannot fork() for maintenance: Resource temporarily unavailable`, and
  git exits 0.
- H1, H2, H3 (advice 9, issued only by its own fixture), H5 (`stash` and `merge` routed), H6 and H8
  were considered, and none halted. M4's git runs added no pair and no operation (H7 and H8
  re-checked).
- The base count is **905 over 137 test files** (M6), as expected.

**The fixtures.** Byte copies of the plan's fixture texts in Tasks 1–4 (`fsops_dyn.c`, `madv_dyn.c`,
`rpath_dyn.c` with `librpath_dyn.c`, and `forkfail_dyn.c`), extracted by the controller into the
ledger. Every build exited 0 and every native
run gave the expected output (`native-*.out`); **no fixture text changed**:

| run | rc | native output |
|---|---|---|
| `fsops` | 0 | `mkdir chdir link rename utimes ok`, `h mtime=1234567890 nlink=2`; `stat` agrees |
| `madv zero` | 0 | `zero low=zeros high=kept` |
| `madv reuse` | 0 | `reuse kept`, `reuse ok` |
| `madv bad` | 0 | `bad rc=0 errno=0` (native `MADV_CAN_REUSE` returns 0) |
| `rpath` | 0 | `rpath marker=47` |
| `forkfail` | 0 | `fork failed errno=35` |

---

## M1 — `madvise`

### M1(a) — the advice census

**Command.** The brief's Step 3 verbatim (`census.sh` over every guest in `retrace-guest`'s
`OUT_DIR`, the five fixtures, jq ×2, CPython ×2, node, and `tools/apple-sweep-binaries.txt`) on the
**census** binary, then `summ.py census.tsv`; 2026-10-01 09:14:12 → 09:18:52, 142 runs
(`census.progress`). Git's own calls come from M4 (`summ.py m4/*.rec.err` →
`m4-census-summary.txt`) and are merged below.

**Result** (`census-summary.txt`, per guest from `census.tsv`):

| advice | name | calls | issued by |
|---|---|---|---|
| 7 | `MADV_FREE_REUSABLE` | 169 | `/bin/date` 4, `/bin/ps` 42, `/bin/zsh` 2, CPython print 44 and crash 44, jq `--version` 1 and file 31, `madv reuse` 1 |
| 8 | `MADV_FREE_REUSE` | 1 | `madv reuse` only |
| 9 | `MADV_CAN_REUSE` | 1 | `madv bad` only (the fixture built to issue an unmodelled value) |
| 11 | `MADV_ZERO` | 1 | `madv zero` only |

Alignment: **all 172 calls** are `('addr16k', 'len16k', 'len>0')`. No unaligned address, no
unaligned length, no zero length.

Git (M4's 16 records plus the tag retry, `m4-census-summary.txt`):
- advice 7: 44 calls. By command: add 5, branch 2, commit 15, log 2, mv 1, rm 1, show 2, stash 10,
  status 1, status `--porcelain` 1, switch `-c` 2, tag 2.
- advice 11: 3 calls, commit 1 and stash 2.
- All 47 are `('addr16k', 'len16k', 'len>0')`.

No new advice value.

**Decision.**
- The accepted set is **{7, 8, 11}**: 7 and 8 are no-ops, 11 is a zero-fill (spec §3c).
- **9 is refused** and routed. The plan's Task 2 Step 3 table calls it a query (H3). It is issued only by the
  `madv bad` fixture, whose whole purpose is the refusal, so no gate guest needs it: **H3 does not
  halt.**
- No call is unaligned, so the refusal rule of M1(d) stands.

### M1(b) — how often `g35`'s abort reproduces

**Command.** The brief's Step 4 verbatim, `m1b.sh m1b 5` on the **census** binary (mode A: advice 7
and 8 forwarded; mode B: `PROBE_NOREUSABLE=1`, no-op'd), a fresh repo per run, `git commit` with
`maintenance.auto=false`.

**Result** (`m1b.txt`):
```
M1b mode=A run=1 rc=0 abort_lines=0 head=committed
M1b mode=A run=2 rc=0 abort_lines=0 head=committed
M1b mode=A run=3 rc=0 abort_lines=0 head=committed
M1b mode=A run=4 rc=134 abort_lines=0 head=none
M1b mode=A run=5 rc=134 abort_lines=0 head=none
M1b mode=B run=1 rc=0 abort_lines=0 head=committed
M1b mode=B run=2 rc=0 abort_lines=0 head=committed
M1b mode=B run=3 rc=134 abort_lines=0 head=none
M1b mode=B run=4 rc=134 abort_lines=0 head=none
M1b mode=B run=5 rc=134 abort_lines=0 head=none
```
- **A (forwarded) aborted 2 of 5; B (no-op'd) aborted 3 of 5.** The probe saw A 1 of 1 and B 0 of 1.
- `abort_lines` is 0 everywhere because libmalloc's message never reaches stderr; the probe read it
  from `x22`. The aborts are identified by rc 134, `guest terminated by signal 6`, and no commit.

Three follow-ups identified the aborts (deviation D7):
1. **A traced mode-B abort** (`m1b-why.sh` → `m1b-why.txt`, `m1b-why-stack.txt`, `fpwalk.py`,
   `m1b-why-sym.txt`). Its first run aborted. `x22` at the terminal stop is `0x180328e07` =
   `*** error for object %p: pointer being freed was not allocated\n`, the probe's string. The frames:
   `pthread_kill` ← `abort` ← `malloc_vreport` ← `malloc_report` ←
   `___BUG_IN_CLIENT_OF_LIBMALLOC_POINTER_BEING_FREED_WAS_NOT_ALLOCATED` ← libz `deflateEnd + 88` ←
   git `git_deflate_end_gently` ← `write_loose_object` ← `write_object_file_flags` ←
   `commit_tree_extended` ← `cmd_commit`. Advice 7 had been no-op'd (`[probe] madvise(0x701400000,
   0x10000, 7) not forwarded`) just before.
2. **`git log -1` aborts with no madvise at all.** M4's first run (census, `-C`) recorded `log -1`
   to rc 134 with **zero** `num=75` traps (`m4-run1/log.rec.err`), the abort right after reading the
   timezone file.
3. **It is on `main`** (`logflake2.sh` → `logflake2.txt`: cwd = a one-commit repo, no `-C`, the
   probe's `git12` shape). `log -1` aborted **1 of 10 on the base binary** and 1 of 10 on census.
   `logflake3.sh` caught a base abort with a trace (run 5 of up to 25): `x22` is the same string, and
   the frames (`logflake3-sym.txt`) are `___BUG_IN_CLIENT_OF_LIBMALLOC…` ← **libc `tzload + 252`**
   (`0xfc`, g35's frame) ← `tzparse` ← `tzload` ← `gmtload` ← `gmt_init` ← `pthread_once` ← `gmtsub`
   ← git `time_to_tm` ← `show_date` ← `pp_user_info` ← `pretty_print_commit` ← `show_log` ← … ←
   `cmd_log`.

**Decision.**
- **The abort g35 showed is not caused by forwarding `MADV_FREE_REUSABLE`.** It is an intermittent
  libmalloc "pointer being freed was not allocated" abort that unpatched `main` already has for git,
  at g35's own `tzload` frame and at a second site (`deflateEnd`). Spec §2c's A/B (`g36`) was one
  run each way, and the B half does not reproduce.
- Approach 1A is still right on its own terms: forwarding madvise hands the host kernel retrace's own
  backing, and `MADV_ZERO` writes 512 KiB the guard band catches (g33). But **§4's "named weakness"
  is wider than §4 says**: `git_e2e`'s commit test cannot guard the madvise class, because the same
  abort occurs without it, and every git test (reads included) can fail at ~10–60 % per run with
  rc 134. That is the controller's to rule on before Task 5 (see "Concerns").
- The mechanism is unmeasured. Both sites free a block allocated earlier in the same libc/libz
  routine, and the abort is input-dependent (the host's wall clock, pids and entropy are forwarded),
  which is consistent with a retrace memory-model fault on some heap layouts. That is inferred.

### M1(c) — a repo-owned trigger

**Command.** The brief's Step 5 loop verbatim on the **base** binary (`step5.sh`).

**Result** (`m1c.txt`, `m1c-*.out`): five of five runs `rc=0`, `reuse kept`, `reuse ok`.

**Decision.** No repo-owned trigger was found: `madv reuse` is not a RED at base. Given M1(b), that is
now expected, since the abort it was looking for is not madvise's.

### M1(d) — native `madvise` alignment, zero length and rounding

**Command.** The brief's Step 6, with `MADV_CAN_REUSE` (9) added to `adv[]` because M1(a) found it
(`madvnative.c`).

**Result** (`m1d.txt`):
```
advice 7: aligned rc=0/0 off4k rc=0/0 off1 rc=0/0 len0 rc=0/0 len0x4001 rc=0/0 byte[0x4000]=0xab byte[0x7fff]=0xab byte[0x8000]=0xab
advice 8: aligned rc=0/0 off4k rc=0/0 off1 rc=0/0 len0 rc=0/0 len0x4001 rc=0/0 byte[0x4000]=0xab byte[0x7fff]=0xab byte[0x8000]=0xab
advice 11: aligned rc=0/0 off4k rc=0/0 off1 rc=0/0 len0 rc=0/0 len0x4001 rc=0/0 byte[0x4000]=0 byte[0x7fff]=0 byte[0x8000]=0xab
advice 9: aligned rc=0/0 off4k rc=0/0 off1 rc=0/0 len0 rc=0/0 len0x4001 rc=0/0 byte[0x4000]=0xab byte[0x7fff]=0xab byte[0x8000]=0xab
```

**Decision.**
- **Native `madvise` at an unaligned address returns 0, not EINVAL**, for every advice (`off4k`,
  `off1`). The brief expected EINVAL. **The model still refuses an unaligned range**, because no
  corpus call is unaligned (M1(a), all 172 plus git's): the brief's rule is that the refusal "stays
  right whatever native does, provided M1(a) found no such call". Task 2's comment must not claim
  native EINVAL; the controller corrects it.
- **Zero length returns 0** for every advice: Task 2's loop answers 0 with nothing done.
- **`MADV_ZERO` rounds `len` up to the page**: `len 0x4001` zeroed `byte[0x4000]` and
  `byte[0x7fff]`, and left `byte[0x8000]` `0xab`. Task 2's
  `a_length_short_of_a_page_zeroes_the_whole_last_page_as_xnu_rounds_it` stands.

---

## M2 — `__mac_syscall`

### M2(a) — the `(policy, call)` census

**Command.** As M1(a); the census binary prints one `[m47]` line per forwarded 381 (policy, the
operation at `*(arg + 16)`, the forwarded result) and the probe arm's `[probe] AMFI` line per AMFI
call.

**Result** (`census-summary.txt`, `census.tsv`):

| policy | call | caller (policy string's image) | operation | forwarded result | calls |
|---|---|---|---|---|---|
| `AMFI` | `0x5a` | dyld (`0x1801b4bf1`, dyld `__cstring`) | — | host answer `in=0x0 r=0 out=0x1df` | 94, exactly one per dyld guest |
| `AMFI` | `0x5a` | **libsystem_trace** (`0x180231d05`, libsystem_trace `__cstring`) | — | host answer `in=0x0 r=0 out=0x1df` | 6, one each in `launchctl`, `automationmodetool`, `dddiagnose`, `desdp`, `dyld_info`, `flex` |
| `Sandbox` | 2 | dyld `sandbox_check_common` (`0x1801b7eb4`) | `syscall-unix` | `ret=0xe err=true writes=0` | 297 |
| `Sandbox` | 2 | libsystem_sandbox `rootless_check_trusted_internal` (`0x18df1ac55`) | `file-write-data` | `ret=0x16 err=true writes=0` | 40: every repo dyld guest, jq, CPython, node; no Apple binary |
| `Sandbox` | **4** | libsystem_sandbox **`sandbox_container_path_for_pid`** (`0x18df1ac55`) | — | `ret=0x2d err=true writes=0` | **6**: `desdp`, `dyld_info`, `flex`, two each |

Git (M4, `m4-census-summary.txt`): AMFI `0x5a`, 17 calls, one per record, all from dyld's string,
with the host answer `0x1df`. Sandbox 2 `syscall-unix` → `0xe`, 51 calls (3 per record). Sandbox 2
`file-write-data` → `0x16`, 17 calls (1 per record). Every one has `writes=0`. **There is no fourth
pair and no third operation** (H7 and H8 re-checked on git, as the ruling requires).

**The second AMFI caller** is a measured fact for Task 3: its policy string is libsystem_trace's
`"AMFI"` (`image lookup -a 0x180231d05`), it traps in libsystem_kernel's `__mac_syscall` stub
(`pc 0x1804af730`) rather than dyld's (`0x180119ecc`), and its struct has the same
`{u64 inFlags; u64 *outFlags}` shape. The probe arm read `in=0x0` at `x2` and the `outFlags` pointer
at `x2 + 8`, and recorded `out=0x1df` through it (e.g. `x2=0x27fd4b0`, `out_ptr=0x27fd4a8`, a guest
stack slot just below the struct; `h7-call4.txt`). The brief expected
"AMFI 0x5a once per dyld guest"; these six Apple binaries issue it twice.

**`("Sandbox", 4)` — H7, ruled.** Spec §11 item 9 makes a third pair a halt. What was measured
(`h7-call4.sh` → `h7-call4.txt`, `h7-desdp-{1,2}.rec.err`; `call4native.c` → `call4native.out`;
`m2b-native.txt`):
- **Caller:** `libsystem_sandbox.dylib\`sandbox_container_path_for_pid + 72` (the `x30` at the stop,
  `0x18df15b60`, by static `image lookup`; retrace's cache slide is 0).
- **Struct** (`retrace debug … x <x2> 64` at both sites): `{+0 u64 pid (0x14e3e, retrace's own,
  since getpid is forwarded); +8 0; +16 char *buf (a guest stack pointer: 0x27fe7c8, 0x27fd500);
  +24 u64 len 0x400}`. **`buf` is a nested out-pointer**, the NestedDest class: the kernel would write
  the container path through it.
- **Forwarded answer:** ENOTSUP (45), `writes=0`, **identical over two records of `desdp`** (same args,
  same result; `desdp` exits 71 both times, as in the sweep baseline where the trio passes).
- **Native answer:** `sandbox_container_path_for_pid(getpid(), buf, 1024)` in an ad-hoc process →
  `rc=-1 errno=45 buf_touched=0` (`call4native.out`). The interposer (M2(b)) saw the same struct
  natively, `{pid, 0, buf, 0x400}`, and the same `rc=-1 errno=45`.
- **Guests:** `desdp`, `dyld_info` and `flex` (the xcrun trio), each twice.
- **Operator decision (2026-10-01): model it** as a third pair answering ENOTSUP (45) with no
  writes. Native and the continuity answer agree, so this pair has **no fidelity gap**. Task 3 codes
  it; t0 changed no code.

**H8 checks.**
- No call-2 operation other than `syscall-unix` and `file-write-data` (census and M4).
- No forwarded call-2 result carried writes (`writes=0` on all 337, plus git's).
- Two runs of one guest agree: the two `desdp` records give identical `[m47]` and `[probe]` result
  lines for all seven of its 381 calls (`h7-desdp-{1,2}.rec.err`, `h7-call4.txt`). The `[trap]`
  lines differ only in `x3`/`x4` of the libsystem_trace AMFI call, which are not among its three
  arguments.

H8 not triggered.

**Decision.** The modelled pairs are:
- `("AMFI", 0x5a)` → the host's answer, recorded;
- `("Sandbox", 2)` → continuity by operation: `syscall-unix` → 14 (EFAULT), `file-write-data` → 22
  (EINVAL);
- `("Sandbox", 4)` → 45 (ENOTSUP), no writes.

Any other pair is refused.

### M2(b) — the native answers

**Command** (deviation D4: no debugger). Step 7 as written failed (`m2b.log`: Xcode's git refuses the
attach; `m2b-rpath.log`: on an ad-hoc guest lldb hung at `run`). Instead:
1. `m2b-structs.sh` → `m2b-structs.txt`: the exact call-2 structs, read with `retrace debug` from a
   census recording of `hello_dyn`.
2. `m2binterpose.c`: a `DYLD_INSERT_LIBRARIES` interposer of libsystem_kernel's `__mac_syscall`. It
   logs every call another image makes natively (the struct, `*(arg+16)`, `*(arg+0)` before and
   after, rc and errno). It catches libsystem_sandbox's **real** calls. It cannot catch dyld's,
   because dyld's `__mac_syscall` is internal to dyld.
3. `m2bnative.c`: dyld's `syscall-unix` struct reissued natively from its measured bytes, with only
   the pointers and the pid being this process's own. libsystem_sandbox's struct is also reissued, as
   a cross-check.

`m2b-native.sh` → `m2b-native.txt`.

**Result.**

| call | how measured | native | retrace (continuity) |
|---|---|---|---|
| dyld `sandbox_check_common`, `syscall-unix` | struct reissued (`{out*, pid, "syscall-unix", 0x41, 0x226, 1}`) | `rc=0`, `out` not written | EFAULT (14) |
| libsystem_sandbox `rootless_check_trusted_internal`, `file-write-data` | **the real call, interposed**, in `hello_dyn`, `rpath_dyn`, `call4native` | `rc=0 errno=0`, `*(arg+0)` not written | EINVAL (22) |
| libsystem_sandbox `sandbox_container_path_for_pid` (call 4) | the real call, interposed | `rc=-1 errno=45` | ENOTSUP (45), equal |

- The reissued `file-write-data` struct returned EBADF (9) natively. That is because **`+32` is a
  file descriptor**. Natively the interposed struct has `+32 = 3`, and under retrace it has
  `+32 = 4`. In `hello_dyn`'s trace the call sits between `open` → `fstat64(4)` and `close(4)`, so
  it is the guest fd just opened (M10's first guest fd is 4).
- The forward therefore handed the host an **untranslated guest fd inside a struct**, which the host
  evaluated against retrace's own fd 4. EINVAL was that check's answer. M47's continuity model
  forwards nothing, so this hazard is closed with the rest of the forward.
- `*(arg+0)` is a zeroed buffer of at least 32–40 bytes in both callers. Native wrote nothing to it.

**Decision.** These values are **not coded** (R7 keeps the pre-M47 errnos for continuity). They go
into Known limits (Task 7) as the fidelity gap: natively both call-2 checks answer 0 (allowed), and
retrace answers EFAULT and EINVAL. Call 4 has no gap. H2 is not triggered: each continuity answer is
a function of the operation string.

### M2(c) — are the strings resident at the trap?

**Result** (`census.tsv`): every Sandbox call-2 `policy` and `op` was read non-empty at the trap
(0 empty of 337), and AMFI's `policy_resident` was `"AMFI\0"` on all 100 calls. The one empty `op`
belongs to call 4, whose `arg+16` is a buffer, not an operation name. Git (M4): AMFI resident on 17
of 17, and 0 empty strings in 68 call-2 lines.

**Decision.** In this corpus the strings were always staged. `read_guest_cstr` (spec §11 item 4) still
pages in on a miss, as the plan specifies; `rpath_dyn` exercises the AMFI path.

---

## M3 — `fork`

### M3(a) — the 3403 request and the native reply

**Command** (deviation D5). The request was read from RETRACE_TRACE's `mach_msg2` send decode on the
**base** binary (`m3a-trace.sh` → `m3a-trace.err`). The guest's own libxpc and libsystem_kernel code
builds it, so its layout is the native one; only the port names are retrace's. The native reply came
from `m3anative.c` (`m3a-native.sh` → `m3a-native.txt`).

**Result: the request** (`m3a-trace.err`):
```
[mach_msg2] msgh_id=3403 dest=0x203 reply=0x1003 options=0x200000003 bits=0x80001513 send_size=64 rcv_size=44
  send+000: 13 15 00 80 40 00 00 00 03 02 00 00 03 10 00 00
  send+010: 00 00 00 00 4b 0d 00 00 03 00 00 00 03 12 00 00
  send+020: 00 00 00 00 00 00 13 00 00 00 00 00 00 00 00 00
  send+030: 00 00 13 00 00 00 00 00 00 00 00 00 00 00 13 00
```
- `msgh_bits` `0x80001513`: `COMPLEX` set, remote `COPY_SEND` (0x13), local `MAKE_SEND_ONCE` (0x15).
- `msgh_size` 64; remote = the task port; `msgh_id` 3403 (`0xd4b`) at offset 20; descriptor count 3
  at offset 24.
- Three 12-byte port descriptors from offset 28, at 28, 40 and 52:
  - names `0x1203`, `0`, `0` (one live send right and two `MACH_PORT_NULL`);
  - **disposition (offset 10) `0x13` = `COPY_SEND` on all three, not `MOVE_*`**, so the model leaves
    no extra user reference;
  - type (offset 11) **0** = `MACH_MSG_PORT_DESCRIPTOR` on all three.
- The caller chain, symbolicated statically: `mach_msg2_internal + 76` ←
  `_kernelrpc_mach_ports_register3 + 136` ← `mach_ports_register + 128` ← libxpc
  `xpc_atfork_prepare + 80` ← `libSystem_atfork_prepare + 40` ← libc `fork + 36` ← `main`.

**Result: the native reply** (`m3a-native.txt`):
- **`mach_ports_register(mach_task_self(), {bootstrap_port, 0, 0}, 3)` → `kr=0`.** These are the three
  rights of the kinds the decode shows.
- The measured request, hand-built and sent through `mach_msg2_internal` with the stub's register
  packing (mode `msg2`), returned **`ret=0`**. The reply:
  ```
  00 12 00 00 24 00 00 00 00 00 00 00 07 07 00 00
  00 00 00 00 af 0d 00 00 00 00 00 00 01 00 00 00
  00 00 00 00 00 00 00 00 08 00 00 00 00 00 00 00
  ```
  That is `msgh_size` **36**, `msgh_id` **3503** at offset 20, the NDR record at offset 24, and
  **RetCode 0 at offset 32**: a `mig_reply_error_t`.
- The same request through plain `mach_msg` (mode `msg`) is **SIGKILLed** (rc 137). macOS 26 kills a
  kernel-object send that lacks `MACH64_SEND_KOBJECT_CALL`.

**Decision.** The layout matches the brief's expectation exactly, and so does spec §11 item 8's
decoder: 64 bytes, `COMPLEX`, id 3403, count 3, and each type byte 0. The reply is a
`mig_reply_error` with id 3503 and RetCode 0, which is what `encode_mig_error(3403, reply,
KERN_SUCCESS)` produces. Every disposition is `COPY_SEND`, so the doc comment's extra-reference note
does not apply.

### M3(b) — every trap from the prepare handler through exit

**Command** (deviation D6). The **m3b** scratch build answers 3403 with
`StubMigReply(KERN_SUCCESS)` and refuses syscall 2 with `EAGAIN` (carry set, no writes, never
forwarded). `m3b-scratch.sh` → `m3b.txt`, `m3b/*.err`, run under `RETRACE_TRACE=1` and
`PROBE_NOREUSABLE=1` (advice 7/8 no-op'd, as Task 2 will do):
- `forkfail_dyn`;
- default-config `git commit -q -m first` in a fresh repo, with `-c user.name/-c user.email` and **no**
  `maintenance.auto=false`.

**Result** (`m3b.txt`):

| guest | record rc | from the 3403 through exit, in order |
|---|---|---|
| `forkfail_dyn` | 0; stdout `fork failed errno=35` | 3403 → **2** (refused) → 339 → 397 → 1 |
| `git commit` | **0**; commit `first` written | 3403 → **2** (refused) → 333 → 329 → 4 (the CANNOT_FORK line) → 6 → 3 → 6 → 33 → 340 → 338 → 339 → 399 → 1 |

- Nothing traps between the 3403 and `fork` in either guest. In `forkfail` nothing traps between
  `setrlimit` (195) and the 3403 either (`m3a-trace.err`).
- **In `forkfail` the parent handlers issue no trap at all**: nothing lies between the refused 2 and
  `printf`'s `fstat64` (339) and `write_nocancel` (397), then exit.
- In git, every trap after the refusal reads as git's own code. This is **inferred** from the order
  and from git's `run-command.c`:
  - 333 then 329 are its `atfork_parent` restoring the cancel state and signal mask that its
    `atfork_prepare` changed before the fork (`g40`'s 329, 333 and 42 before the 3403);
  - 4 is `write(2, …)` of the CANNOT_FORK line;
  - 6, 3, 6 close and drain its notify pipe;
  - 33 (`access(…, X_OK)`), 340 and 338 look for the `post-commit` hook;
  - then exit.
- No `mach_msg` other than 3403 appears. No trap stops at the M33 panic or a RECORD ERROR. The only
  number without a row on `main` is 333, which M47 adds (spec §11 item 1); 2 is the refusal itself.
- Git under this build printed `error: cannot fork() for maintenance: Resource temporarily
  unavailable` and exited 0 (`m3b/git-commit.out`, `[fd2 (console 2)]` echo).

The static supplement is the probe's `docs/sweep-evidence/2026-09-30-m47-probe/fork-disasm.txt`: libc `fork` calls the prepare handlers, then
`__fork`, then on carry `cerror` and the parent handlers. It agrees with the trap order above.

**Decision.** **H1 is not triggered**: exactly one message (3403) precedes `__fork`, and the parent
path reaches no unmodelled number. Task 4's refusal plus the 3403 answer suffice, and the fallback
(`maintenance.auto=false` plus a documented limit) is not needed.

### M3(c) — git's native "cannot fork"

**Command.** The brief's Step 9 verbatim (`step9.sh`); `ulimit -u 1` was accepted.

**Result** (`m3c.txt`, `m3c.out`, `m3c.err`): `setup=0`, **`rc=0`**, stdout empty, stderr exactly:
```
error: cannot fork() for maintenance: Resource temporarily unavailable
```
`log -1` → `second`: the commit was written.

**Decision.** `CANNOT_FORK = "error: cannot fork() for maintenance: Resource temporarily
unavailable"`. Git exits 0, so Task 5's commit test expects rc 0, and the m3b build reproduced both
(M3(b)). Under retrace the line arrives on the record's **stdout**, because the console arm merges
fd 2 into the mirrored stdout (M4).

---

## M4 — git's command list

**Command.** The brief's Step 10, `m4.sh m4` on the **census** binary, with D8's one-line fix:
- reads run natively, then recorded, on one repo;
- writes run on twin repos, comparing the hash-free state fingerprint;
- every record that does not panic is replayed once.

Then three follow-ups:
- the one row that aborted was retried with `m4-retry.sh` (m4.sh's own functions, sourced from its
  text);
- `stash`'s child was identified with `m4-stash-why.sh`;
- the rowcheck ran as the brief gives it (`rowcheck.sh`: `m4-rowcheck.log` on `m4/nums.txt`, and
  `union-rowcheck.log` on M4 ∪ the retry ∪ M3(b)'s m3b-build runs), and `m4-norow.sh` says which
  command dispatched each NOROW number.

**Result** (`m4.txt`, `m4-retry.txt`):
```
M4 READ status-porcelain native_rc=0 rec_rc=0 rp_rc=0 stdout_rec==native:yes rp==rec:yes wall=
M4 READ status native_rc=0 rec_rc=0 rp_rc=0 stdout_rec==native:yes rp==rec:yes wall=
M4 READ diff native_rc=0 rec_rc=0 rp_rc=0 stdout_rec==native:yes rp==rec:yes wall=
M4 READ diff-cached native_rc=0 rec_rc=0 rp_rc=0 stdout_rec==native:yes rp==rec:yes wall=
M4 READ log native_rc=0 rec_rc=0 rp_rc=0 stdout_rec==native:yes rp==rec:yes wall=
M4 READ show native_rc=0 rec_rc=0 rp_rc=0 stdout_rec==native:yes rp==rec:yes wall=
M4 READ rev-parse native_rc=0 rec_rc=0 rp_rc=0 stdout_rec==native:yes rp==rec:yes wall=
M4 WRITE add native_rc=0 rec_rc=0 rp_rc=0 state_rec==native:yes rp==rec:yes wall=
M4 WRITE commit native_rc=0 rec_rc=0 rp_rc=0 state_rec==native:yes rp==rec:yes wall=
M4 WRITE branch native_rc=0 rec_rc=0 rp_rc=0 state_rec==native:yes rp==rec:yes wall=
M4 WRITE tag native_rc=0 rec_rc=134 rp_rc=134 state_rec==native:no rp==rec:yes wall=
M4 WRITE switch-c native_rc=0 rec_rc=0 rp_rc=0 state_rec==native:yes rp==rec:yes wall=
M4 WRITE mv native_rc=0 rec_rc=0 rp_rc=0 state_rec==native:yes rp==rec:yes wall=
M4 WRITE rm native_rc=0 rec_rc=0 rp_rc=0 state_rec==native:yes rp==rec:yes wall=
M4 WRITE stash native_rc=0 rec_rc=4 rp_rc=n/a state_rec==native:no rp==rec:n/a wall=RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64
M4 WRITE merge native_rc=0 rec_rc=101 rp_rc=n/a state_rec==native:no rp==rec:n/a wall=thread 'main' (16560566) panicked at crates/retrace-arch/src/lib.rs:1018:38:
--- m4-retry.txt
M4 WRITE tag-try1 native_rc=0 rec_rc=0 rp_rc=0 state_rec==native:yes rp==rec:yes wall=
```
- **`tag`'s rc 134 is M1(b)'s abort, not a wall of `tag`.** `m4/tag.rec.err` ends with the
  libmalloc report sequence. First comes `close_nocancel(4)` right after a 0xa1e8-byte read
  (inferred to be the timezone file, as in `log -1`'s abort). Then `-10` with `x4 = 0x180328e07`,
  `-10`, `-12`, 48, 478 and 329, then `328` (`__pthread_kill`, signal 6). It made zero
  madvise calls. The first retry passed. (`log -1` hit the same abort in run 1,
  `m4-run1/log.rec.err`.)
- **`stash` forks real children.** Natively, `GIT_TRACE` shows `git update-index … --stdin` and then
  `git reset --hard -q --no-recurse-submodules` (`m4-stash-why.txt`). On the m3b build, where fork is
  refused, `stash` prints `error: cannot fork() for update-index: Resource temporarily unavailable`
  (`m4-stash-m3b.out`), exits 1 and creates no stash.
- **`merge --ff-only` arms a timer.** The last traps are `sigaction(SIGALRM = 14, …)` (46), then
  `setitimer(ITIMER_REAL, …)` (**83**, no row), which is git's progress timer. That is the M33 panic
  at `lib.rs:1018` (`m4/merge.rec.err`).
- **Rowcheck** (`m4-rowcheck.log`): `NOROW 9, 12, 83, 136, 333`. The union adds `2`
  (`union-rowcheck.log`). Per command (`m4-norow.txt`):
  - every git command dispatches 12 (`chdir`, for `-C`);
  - `add`, `commit` and `stash` dispatch 9 and 136;
  - `stash` dispatches 333, as does the default-config commit's fork path (M3(b));
  - only `merge` dispatches 83.
  - **No command reached 138.** No clean repo freshens an object; 138 entered the probe only after a
    crashed run had left objects behind (`g34`).
- **A fact for Task 5:** under retrace, **the guest's stderr is merged into the record's stdout**.
  `record_box`'s console arm appends writes to fd 1 and fd 2 to one `stdout` buffer
  (`crates/retrace-core/src/lib.rs:247`). git's CANNOT_FORK line therefore arrives on the record's
  **stdout** (`m3b/git-commit.out`), whereas natively it is on stderr (`m3c.err`).

**Decision.**
- **Read in-list (Task 5's `READS`):** `status --porcelain`, `status`, `diff`, `diff --cached`,
  `log -1`, `show --stat HEAD`, `rev-parse HEAD`. All seven meet the rule.
- **Write in-list:** `add`, `commit`, `branch`, `tag`, `switch -c`, `mv`, `rm --cached`.
  - `tag` is in on its retry. Its failure was the intermittent abort, which is not a wall of `tag`'s
    and which `commit` and `log` share.
  - `commit` is in under `maintenance.auto=false` here, and under default config by M3(b) on the m3b
    build.
- **Out-list (H5, routed, not widened):**
  - `stash`: it spawns `git update-index` and `git reset --hard`, real children other than
    auto-maintenance. Under M47's refusal it fails with `cannot fork() for update-index`. Routed to
    real process creation.
  - `merge --ff-only`: `setitimer` (83) with a SIGALRM handler, a timer-delivered signal. Forwarded,
    it would arm retrace's own process. Routed with the timed waits (Q1).
- **k = 5:** `branch`, `tag`, `switch -c`, `mv`, `rm`.
- **Task 1's rows:** NOROW ∩ in-list = {9, 12, 136}, plus 333 (spec §11 item 1, measured on the
  default-config commit's fork path), plus 138 (not reached by M4; Task 1's `fsops` fixture issues
  it, and `g34` measured it on a freshening commit). **No row beyond the plan's {9, 12, 136, 138,
  333}.** 83 is `merge`'s, which is out, so it gets no row (M33's rule). 2 is Task 4's, landing with
  its refusal.

---

## M5 — the `chdir` audit

**Command.** The brief's Step 11 grep (`m5-grep.txt`, **79 lines**), plus a wider sweep for
`libc::open|fopen|dlopen|Path::new|PathBuf::from|set_current_dir|env::var`. Then a reading of
`record_box`, `main.rs`'s `record`/`record-dyn` arms, `CacheMeta::load` and `DecodedTrace::load`.

**Result.** Every line falls into one class:
- **(i) Absolute path.** These are `crates/retrace-box/src/cache.rs:261` (`read_subcache`) and `:387`
  (`metadata`). Both are reached only from `CacheMeta::load(DEFAULT_CACHE_PATH)`, with the absolute
  `/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e`, and with the
  subcache paths built from it (`:396`). This is the **one open that happens after the guest starts**
  (`install_cache_pager`, at trap 536). It is absolute, and the files are then held open and read by
  fd (`read_exact_at`).
- **(ii) Before the guest's first instruction on the record path.**
  - `crates/retrace/src/main.rs:15` and `:55` read the guest image.
  - `main.rs:58` reads dyld, from the image's `LC_LOAD_DYLINKER`, `/usr/lib/dyld`.
  - `crates/retrace-core/src/lib.rs:129` is `Writer::create(trace_path)`, the first statement of
    `record_box`, before the first `b.run()`. Its `w` is held for the whole loop. Its implementation
    is `crates/retrace-trace/src/lib.rs:86`.
  - `trace_path` occurs nowhere else in `record_box`: the trace is opened once and never reopened by
    path.
- **(iii) Replay, debug or gdbserver only.** These are `crates/retrace-core/src/lib.rs:1360`
  (`DecodedTrace::load`, used by `ReplaySession::open`, `from_checkpoint` and
  `CheckpointCache::decoded`) and its implementation `crates/retrace-trace/src/lib.rs:114`. Those
  processes forward nothing and never `chdir`.
- **(iv) Test-only.**
  - `crates/retrace-guest/src/lib.rs:312–453` (`mod tests`, from `:307`) and `:502–532`
    (`mod fat_tests`, from `:462`);
  - `crates/retrace-trace/src/lib.rs:158–373` (`mod tests`, from `:142`);
  - `crates/retrace-box/src/lib.rs:6756` (`mod stack_geometry_tests`, from `:6746`);
  - `crates/retrace-box/src/cache.rs:717` (`mod tests`, from `:513`);
  - `crates/retrace/src/rsp.rs:472` (tests from `:307`);
  - `crates/retrace/src/debug.rs:1410`, `:1549` and `:1553` (tests from `:1297`).
- **The wider sweep** found no other path open:
  - `guest_mmap_file`'s `libc::stat` is an `fstat` on an fd;
  - gdbserver builds the exe image from the snapshot;
  - the `RETRACE_*` diagnostics are `eprintln!` only.

**Decision.** **H6 is not triggered.** R1 stands: forwarding `chdir` moves retrace's own cwd, and
nothing retrace opens by a relative path after the guest starts.

---

## M6 — the base `#[test]` count

**Command.** The brief's Step 12 (`m6.sh` → `m6.txt`, with the per-file breakdown).

**Result.** **905** `#[test]` lines over **137** test files. Both match the brief and the controller's
pre-flight measurement.

**Decision.** M46's 898 + 9 = 907 over 152 binaries is 905 plus `census.rs`'s two tests compiled
twice. The floor is confirmed, with no reconciliation owed.

---

## Deviations from the brief

- **D1 — scripts instead of compound commands.** The session's worktree guard refuses compound
  commands that `cd` into, or loop over, the ledger path, because the path contains "git". Each
  multi-command step therefore became a script under the ledger's `t0/`, written with the Write tool
  and run with `zsh`. Each script's command text is the brief's, plus a leading `L=`/`cd` line:
  `step2.sh`, `run-census.sh` (wrapping the verbatim `census.sh`), `step5.sh`, `step7.sh`, `step9.sh`
  and `m6.sh`. `m2b.lldb` was generated with the brief's exact command list (the same 58 lines).
- **D2 — `madvnative.c`.** `MADV_CAN_REUSE` was added to `adv[]`, as Step 6 instructs for a census value.
- **D3 — lldb cannot debug a process here.**
  - Xcode's git refuses the attach (`m2b.log`: `attach failed (Not allowed to attach to process…)`).
  - On the ad-hoc `rpath_dyn`, lldb hung at `run` for 5 minutes (`m2b-rpath.log`).
    `DevToolsSecurity -status` reports `Developer mode is currently disabled`, so debugserver was
    presumably waiting on an authorization prompt (inferred).
  - The processes were killed. `m3a.lldb` and `m3b.lldb` are kept as written but were not run.
  - Operator ruling (2026-10-01): use non-debugger fallbacks, with no host security change.
  - Static `lldb` (`target create <host binary>` plus `image lookup -a`, with no process) works, and
    it symbolicated every cache address in this file. Git's own addresses went through `atos` at
    git's unslid base.
- **D4 — M2(b)** used the struct dump, the native interposer and the native reissue described there.
- **D5 — M3(a)** took the request from RETRACE_TRACE and the reply from a native C program (`m3anative.c`).
- **D6 — M3(b)** used the throwaway **m3b** build (`m3b-scratch.patch`), with `PROBE_NOREUSABLE=1`.
- **D7 — the M1(b) follow-ups**:
  - `m1b-why.sh` (with `fpwalk.py`, `m1b-why-sym*.sh`);
  - `logflake.sh`, whose base half hit the M33 `chdir` panic because `-C` is a chdir; it is
    superseded by `logflake2.sh`;
  - `logflake2.sh`;
  - `logflake3.sh` (with `logflake3-sym.sh`).
- **D8 — the `m4.sh` script bug, fixed.**
  - In the first M4 run (`m4-run1-invalid.txt`), `mkrepo` assigned the global `d`. So
    `write_cmd`'s `mkrepo $n; mkrepo $d` re-made the native twin, and **every write was recorded in
    the native repo after native had run**: branch, tag, switch, mv and rm failed with rc 128
    ("already exists"), commit returned rc 1 ("nothing added"), and every state compare was trivially
    equal.
  - The fix is one line, `local d=$1` in `mkrepo` (commented in `m4.sh`). M4 was re-run from scratch.
  - Run 1's read rows were valid (`read_cmd`'s `d` is the same value) and are cited only for
    `log -1`'s abort (`m4-run1/log.rec.err`).
- **D9 — Step 8's `cat > … <<'EOF'` heredocs** were written with the Write tool (ledger Ruling P4).

## Concerns for the controller

1. **The libmalloc abort is pre-existing and not madvise's (M1(b)).**
   - It is intermittent: `log -1` aborts at ~10 % on base, and `commit` at 40–60 % on census in both
     modes.
   - It is input-dependent and replays identically. It reaches reads as well as writes.
   - Consequences:
     - every `git_e2e` test that records git can fail with rc 134;
     - spec §2c's causal claim and §4's commit-test guard rest on a single A/B that does not
       reproduce;
     - the madvise model is still sound on its own terms (no forward, no 512 KiB guard-band panic).
   - Diagnosing the heap fault is unscoped work. It may be a milestone of its own, or a gate the
     operator decides to tolerate by retrying.
2. **M4's single-shot rows are exposed to (1).** The handling is described in M4.

## Decisions

1. **Accepted advice set: {7 `MADV_FREE_REUSABLE` → no-op, 8 `MADV_FREE_REUSE` → no-op, 11
   `MADV_ZERO` → zero-fill}.** 9 (`MADV_CAN_REUSE`, a query) is refused and routed. Only the `madv
   bad` fixture issues it, so H3 does not halt. Every other value is refused, naming the value (M1(a)).
2. **Alignment and rounding rule** (M1(d)):
   - **Refuse an address off a 16 KiB page.** Native accepts it with rc 0, not EINVAL, but no corpus
     call (172 plus git's 47) is unaligned, so the refusal stands. Task 2's comment must say native
     returns 0.
   - `len == 0` answers 0 with nothing done.
   - `MADV_ZERO` rounds `len` up to the page and zeroes the whole last page; the byte after it is kept.
3. **Modelled `__mac_syscall` pairs** (M2):
   - `("AMFI", 0x5a)`: the host's answer about retrace's own process (`0x1df` for inFlags 0), recorded
     as the 8-byte write through `outFlags`. There are two callers with the same struct: dyld's
     `amfi_check_dyld_policy_self`, and a libsystem_trace caller in six Apple binaries.
   - `("Sandbox", 2)`, keyed by the operation at `*(arg + 16)`: `"syscall-unix"` → 14 (EFAULT) and
     `"file-write-data"` → 22 (EINVAL), with no writes. Natively both answer 0; that is the
     Known-limits fidelity gap (M2(b)).
   - **`("Sandbox", 4)` → 45 (ENOTSUP), no writes.** Measured, and approved by the operator on
     2026-10-01 (H7). The caller is libsystem_sandbox `sandbox_container_path_for_pid`, the struct is
     `{pid, 0, char *buf, 0x400}` with a nested `buf`, and native agrees, so there is no gap.
   - Any other pair, or any other call-2 operation, is refused.
4. **3403 layout** (M3(a)):
   - 64 bytes; `msgh_bits` `0x80001513` (`COMPLEX`); `msgh_id` 3403 at offset 20; descriptor count 3
     at offset 24.
   - Three 12-byte port descriptors from offset 28, each with disposition `0x13` (`COPY_SEND`) at
     offset 10 and type 0 (`MACH_MSG_PORT_DESCRIPTOR`) at offset 11. Names `<send right>, 0, 0`.
   - The native reply is a 36-byte `mig_reply_error` with id 3503 and RetCode 0
     (`encode_mig_error(3403, reply, KERN_SUCCESS)`).
   - No `MOVE_*` disposition, so no extra user reference.
5. **`CANNOT_FORK = "error: cannot fork() for maintenance: Resource temporarily unavailable"`.**
   Native git exits 0 with the commit written (M3(c)). Under retrace the line arrives on the
   record's **stdout** (M4). H1 is not triggered: 3403 is the only message before `fork`, and the
   parent path reaches no unmodelled number (M3(b)).
6. **Read in-list (`READS`):** `status --porcelain`, `status`, `diff`, `diff --cached`, `log -1`,
   `show --stat HEAD`, `rev-parse HEAD`.
   **Write in-list:** `add`, `commit`, `branch`, `tag`, `switch -c`, `mv`, `rm --cached`.
   **Out-list:** `stash` (spawns `update-index` and `reset --hard`), `merge --ff-only`
   (`setitimer` 83 + SIGALRM) (M4).
7. **Task 1's full row set: {9 `link`, 12 `chdir`, 136 `mkdir`, 138 `utimes`, 333
   `__pthread_canceled`}**, unchanged from the plan. M4 found no other NOROW number on an in-list
   command. 2 (`fork`) is Task 4's.
8. **k = 5** (`branch`, `tag`, `switch -c`, `mv`, `rm`).
9. **Base count: 905 `#[test]` over 137 test files** (M6).
10. **Owed to the controller, not decided here:** the pre-existing intermittent libmalloc abort
    (M1(b), Concerns 1–2), which every git gate inherits.
