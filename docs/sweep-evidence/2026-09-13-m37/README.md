# Sweep evidence — M37 Task 4, 2026-09-13

Three full post-fix runs of `tools/apple-sweep.sh` over the 54-entry corpus, one per recorder-pid
regime, with **every** row's trace kept (`RETRACE_SWEEP_KEEP_ALL=1`, M37 t1); the three audit
measurements spec §3b owes (static table, `scalar-writes`, baseline-vs-N); and the acceptance
criterion spec §5 states (self-pid `ESRCH` zero in every kept trace). The decisive stderr of the
nine non-clean rows is committed beside this file, verbatim. Traces are not committed; they live
in the SDD workspace (`.superpowers/sdd/2026-09-13-retrace-m37-classb/sweeps/keep-{N,I,S}/
<basename>.bin`, `git`-excluded locally) with the three logs `sweeps/sweep-{N,I,S}.log` and the
audit outputs under `audit/`; the pre-fix baseline (M37 t1) is `baseline/keep-base/` +
`baseline/sweep-base.log`.

**Binary commit:** `aa8d7b8` (branch `m37-classb`, the tree after M37 t2 `dup2` + t3 `Scalar`
skip and their fix commits; `git diff aa8d7b8 --stat -- crates/` is empty at build time).
`target/aarch64-apple-darwin/debug/retrace` sha256
`58387f16509d9ed01c8230a87bdc855d6f182aae6eb6628f9d83c789d69aa1d7`, copied to
`sweeps/retrace-aa8d7b8` and ad-hoc signed with `retrace.entitlements` (sha256 after signing
`d991f7c0edf7574ed90ce978408afe3b78f8ab539283baaf8fe287807852b3a4`); the script then signs its own
per-run copy of that. **Baseline binary** (Task 1): `baseline/retrace-648d4cf`, the pre-M37 `main`,
sha256 `f14844eccccd58af8fe9b1fa1725c9bb2c124ff11c3d1123521d54ffb773f026`. **Script:**
`tools/apple-sweep.sh` at `83dcc5c` (M37 t1, unchanged since), list `tools/apple-sweep-binaries.txt`
at `c1e4eb4`.

## The runs

The pid counter was read with `sh -c 'echo $$'` and advanced with a loop of `/usr/bin/true`
(~1.1 ms per spawn); a sweep advances the counter by ~2,500–2,900 (other sessions on the machine spawn too). Run N came after wrapping the counter past xnu's
`PID_MAX` (99998 → 100); I after advancing to ~17,100; S after advancing to ~66,100 (the
`os_alloc_once` slab band, M36's run-O regime). Each run was one detached invocation, polled
(`W` = the SDD workspace, `T` = the worktree, `<R>` ∈ N, I, S):

```sh
nohup sh -c "sh -c 'echo pidstart=\$\$' > $W/sweeps/sweep-<R>.log; RETRACE_SWEEP_KEEP=$W/sweeps/keep-<R> RETRACE_SWEEP_KEEP_ALL=1 $T/tools/apple-sweep.sh $W/sweeps/retrace-aa8d7b8 >> $W/sweeps/sweep-<R>.log 2>&1; echo SWEEP_EXIT=\$? >> $W/sweeps/sweep-<R>.log" >/dev/null 2>&1 &
```

| run | `pidstart` | recpid range | regime (M36's window `[0x4000,0x18000)`) | `TALLY` | `SWEEP_EXIT` |
|---|---|---|---|---|---|
| N | 754 | 765–3291 (`0x2fd`–`0xcdb`) | non-colliding (below `0x4000`) | `TALLY pass=45 fail=9 skip=0` | 0 |
| I | 17113 | 17124–20042 (`0x42e4`–`0x4e4a`) | colliding, inside `[0x4000,0x10000)` — the trampoline page | `TALLY pass=45 fail=9 skip=0` | 0 |
| S | 66152 | 66163–68793 (`0x10273`–`0x10cb9`) | colliding, inside `[0x10000,0x18000)` — the `os_alloc_once` slab | `TALLY pass=45 fail=9 skip=0` | 0 |

Every `ROW` line's `recpid` is inside its run's band — `awk` over the 54 `ROW` lines of each log,
output verbatim (the band test is the third field of the `outside` label):

```
run N: ROW lines=54 recpid min=765 (0x2fd) max=3291 (0xcdb) outside [1,0x4000)=0 empty=0
run I: ROW lines=54 recpid min=17124 (0x42e4) max=20042 (0x4e4a) outside [0x4000,0x10000)=0 empty=0
run S: ROW lines=54 recpid min=66163 (0x10273) max=68793 (0x10cb9) outside [0x10000,0x18000)=0 empty=0
```

The slab is where M36 said: in every kept §4b-row trace of all three runs the
`mach_vm_map(size 0x8000, flags 0x49000001)` landmark returns `0x10000` (reader `slabmap`; `csh`
#128, `dddiagnose` #182 — the same indices M36 measured), so run S's pids fall inside a backing the
guest has mapped at the time of its pid-carrying calls, and run I's inside the trampoline page.
Pre-M37 those regimes answered every self-pid call `ESRCH` (M36: 11–12 per trace); now 0 (below).

**The nine non-clean rows, three regimes.** Labels are the harness's structural ones (M36 §3a); `rc`
= record exit, `rp` = replay exit, landmark = the replay's `DIVERGENCE at landmark N` (for a
`RECORD ERROR` row the refused call is the stop and is never recorded, so the trace has exactly N
events and replay runs out at N — checked: `events == landmark` for all 24 record-error traces).

| row | N (recpid, landmark) | I (recpid, landmark) | S (recpid, landmark) | label (identical in N/I/S) |
|---|---|---|---|---|
| `/bin/csh` | 1005, 331 | 17385, 330 | 66385, 334 | `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64`, rc/rp 4/3 |
| `/bin/tcsh` | 2342, 329 | 18983, 332 | 67896, 332 | same line as `csh`, rc/rp 4/3 |
| `/bin/launchctl` | 1601, 338 | 18162, 330 | 66837, 331 | `record error, rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102: message-queue send without the send+rcv RPC shape`, rc/rp 4/3 |
| `/usr/bin/automationmodetool` | 2593, 343 | 19193, 340 | 68104, 339 | same RCV line, rc/rp 4/3 |
| `/usr/bin/desdp` | 2626, 366 | 19226, 366 | 68136, 365 | same RCV line, rc/rp 4/3 |
| `/usr/bin/dyld_info` | 2655, 365 | 19255, 367 | 68167, 366 | same RCV line, rc/rp 4/3 |
| `/usr/bin/flex` | 2688, 362 | 19285, 369 | 68197, 366 | same RCV line, rc/rp 4/3 |
| `/usr/bin/dddiagnose` | 2718, 383 | 19315, 380 | 68226, 384 | same RCV line, rc/rp 4/3 |
| `/usr/bin/yes` | 3145, n/a | 19820, n/a | 68650, n/a | `timed out after 30s recording`, rc 137, no replay ran (the watchdog) |

The other 45 rows are `PASS` in all three runs (no `identical fault` row anywhere — `dddiagnose`'s
M36 run-I face, `rc=139` both sides after 11 self-pid `ESRCH`, did not recur). The
`yes.{N,I,S}.bin` traces (~350 MB each, a recorder SIGKILLed mid-`write` loop) were read for the
audits below and then deleted, as ruled; their `rec.err` is kept. The landmark at which a §4b row
stops varies by a few between runs of the same binary at the same regime (e.g. `launchctl` 338 / 330
/ 331) — that is run-to-run variation of the guest's own path (pairs of `sigprocmask` appear and
vanish, allocation addresses shift), and two PRE-fix runs show the same spread (M36 run L
`launchctl` 338 vs the Task 1 baseline 329; the M36-L-vs-baseline syscall sequences of `csh`,
`tcsh` and `launchctl` differ in 18/23/34 lines over their first 260 events, all of that kind), so
the landmark is not part of the label and no landmark difference is attributed to M37.

## Counting rules

M36's rules apply unchanged (`docs/sweep-evidence/2026-09-13-m36/README.md`, "Counting rules": a
landmark's index is its position in the event vector with the initial `Snapshot` #0; `err` = the
number of `Event::Syscall` with `err == true`; the `0x10000` map as there). Two rules are added:

- **`selfpid`** (the acceptance count): the number of `Event::Syscall` with `num ∈ {169, 170, 336}`,
  `ret == 3`, `err == true`, and the pid register equal to that row's `recpid` — `args[0]` for
  `csops` (169) / `csops_audittoken` (170), `args[1]` for `proc_info` (336); the prototypes are the
  row comments at `crates/retrace-arch/src/lib.rs:801` (`csops(pid_t pid, …)`) and `:839`
  (`proc_info(int32_t callnum, int32_t pid, …)`). The reader also prints the number of landmarks
  carrying the pid at all (`self-pid calls`), so a 0 is visibly "0 of 12", never "0 of 0". The
  rule was positive-controlled on M36's kept colliding traces before use: `dddiagnose` keep-I
  (pid 18781) → 12 calls, **11** `ESRCH`; keep-O (pid 74909) → 13 calls, **12** — M36's numbers.
- **`scalar-writes`** (audit measurement 2): for every `Event::Syscall` and every position `i`
  where `retrace_arch::arg_kinds(num)` marks `ArgKind::Scalar`, a *hit* is a `writes` region `r`
  with `r.ipa <= args[i] < r.ipa + r.bytes.len()`. Positions past a row's arity are not `Scalar`
  and are not counted. A hit means "the value in a `Scalar` register lies inside memory this
  landmark recorded as written"; it does NOT by itself mean the kernel wrote through it — see the
  adjudication under audit 2.

## The reader

Scratchpad crate `m37reader` (as M36's `errcount`), depending on this worktree's
`crates/retrace-trace` and `crates/retrace-arch` by path; `cargo build --release`. Every number in
this file comes from one of its modes over a kept trace.

`Cargo.toml`:

```toml
[package]
name = "m37reader"
version = "0.0.0"
edition = "2021"

[workspace]

[dependencies]
retrace-trace = { path = "/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m37-classb/crates/retrace-trace" }
retrace-arch = { path = "/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m37-classb/crates/retrace-arch" }
```

`src/main.rs`:

```rust
// M37 Task 4 reader — every number in docs/sweep-evidence/2026-09-13-m37/README.md is
// re-derivable from a kept trace with this and nothing else. Modelled on M36's `errcount`.
//
//   scalar-writes <trace>…   for every Event::Syscall, for every position i that
//                            retrace_arch::arg_kinds(num) marks Scalar, report any `writes`
//                            region containing args[i]  (audit measurement 2; expected: none)
//   errs <trace> [n]         err=true count per syscall number (M36's errcount; audit 3);
//                            with n, over the first n events only
//   selfpid <trace> <pid>    count of 169/170/336 landmarks whose pid register == pid and
//                            ret == 3 && err (the M36 counting rule; acceptance, expected 0).
//                            The pid register is args[0] for csops(169)/csops_audittoken(170)
//                            and args[1] for proc_info(336) — the prototypes at
//                            crates/retrace-arch/src/lib.rs:801 and :839.
//   calls <trace> <num>      every landmark of syscall <num>: index, args[0..4], ret, err
//   last <trace> [n]         the last n (default 3) events, summarised, with their indices
//   low <trace>              the final snapshot's regions below 0x200000
//   writes <trace> <idx> <dir>  dump landmark <idx>'s write regions to <dir>
//   slabmap <trace>          the os_alloc_once mach_vm_map (M36's 0x10000-map rule) and its result
use retrace_arch::{arg_kinds, ArgKind};
use retrace_trace::{Event, Reader};
use std::collections::BTreeMap;

fn open(path: &str) -> Vec<Event> {
    let (events, truncated) = Reader::open_checked(path).expect("open trace");
    if truncated { eprintln!("{path}: TRUNCATED tail (open_checked)"); }
    events
}

fn pid_reg(num: u64) -> Option<usize> {
    match num { 169 | 170 => Some(0), 336 => Some(1), _ => None }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("scalar-writes") => {
            let mut total = 0usize;
            for path in &args[1..] {
                let events = open(path);
                let (mut hits, mut positions, mut syscalls) = (0usize, 0usize, 0usize);
                for (idx, ev) in events.iter().enumerate() {
                    if let Event::Syscall { num, args, ret, writes, .. } = ev {
                        syscalls += 1;
                        let Some(shape) = arg_kinds(*num) else { continue };
                        for (i, kind) in shape.args.iter().enumerate() {
                            if *kind != ArgKind::Scalar { continue; }
                            positions += 1;
                            let v = args[i];
                            for r in writes {
                                let end = r.ipa + r.bytes.len() as u64;
                                if v >= r.ipa && v < end {
                                    hits += 1;
                                    println!("HIT {path} #{idx} syscall {} x{i}={v:#x} inside write [{:#x},{end:#x}) args=[{:#x}, {:#x}, {:#x}, {:#x}] ret={ret:#x}",
                                             *num as i64, r.ipa, args[0], args[1], args[2], args[3]);
                                }
                            }
                        }
                    }
                }
                println!("{path}: syscalls={syscalls} scalar_positions={positions} hits={hits}");
                total += hits;
            }
            println!("TOTAL hits={total}");
        }
        Some("errs") => {
            let mut events = open(&args[1]);
            // Optional: only the first <n> events (a prefix, to compare a trace that stopped
            // earlier against one that went on — csh/tcsh in audit 3).
            if let Some(n) = args.get(2) { events.truncate(n.parse().expect("prefix len")); }
            let mut by_num: BTreeMap<i64, usize> = BTreeMap::new();
            for ev in &events {
                if let Event::Syscall { num, err: true, .. } = ev { *by_num.entry(*num as i64).or_default() += 1; }
            }
            for (num, n) in &by_num { println!("syscall {num:>5}: err={n:>3}"); }
        }
        Some("selfpid") => {
            let events = open(&args[1]);
            let pid: u64 = args[2].parse().expect("pid");
            let (mut calls, mut esrch) = (0usize, 0usize);
            for ev in &events {
                if let Event::Syscall { num, args, ret, err, .. } = ev {
                    if let Some(i) = pid_reg(*num) {
                        if args[i] == pid {
                            calls += 1;
                            if *ret == 3 && *err { esrch += 1; }
                        }
                    }
                }
            }
            println!("{}: pid={pid} self-pid calls={calls} self-pid ESRCH={esrch}", args[1]);
        }
        Some("calls") => {
            let events = open(&args[1]);
            let want: i64 = args[2].parse().expect("num");
            for (idx, ev) in events.iter().enumerate() {
                if let Event::Syscall { num, args, ret, err, writes, .. } = ev {
                    if *num as i64 == want {
                        println!("#{idx} syscall {want} args=[{:#x}, {:#x}, {:#x}, {:#x}] ret={ret:#x} err={err} writes={}",
                                 args[0], args[1], args[2], args[3], writes.len());
                    }
                }
            }
        }
        Some("last") => {
            let events = open(&args[1]);
            let n: usize = args.get(2).map(|s| s.parse().expect("n")).unwrap_or(3);
            let start = events.len().saturating_sub(n);
            println!("{}: events={}", args[1], events.len());
            for (idx, ev) in events.iter().enumerate().skip(start) {
                match ev {
                    Event::Syscall { num, args, ret, err, thread, .. } =>
                        println!("#{idx} Syscall num={} args=[{:#x}, {:#x}, {:#x}] ret={ret:#x} err={err} thread={thread}", *num as i64, args[0], args[1], args[2]),
                    Event::Snapshot { mem, .. } => println!("#{idx} Snapshot regions={}", mem.len()),
                    Event::Exit { code, thread } => println!("#{idx} Exit code={code} thread={thread}"),
                    Event::Crash { pc, esr, far, thread } => println!("#{idx} Crash pc={pc:#x} esr={esr:#x} far={far:#x} thread={thread}"),
                    Event::Signal { sig, pc, thread } => println!("#{idx} Signal sig={sig} pc={pc:#x} thread={thread}"),
                    Event::SignalDelivery { sig, handler, thread, .. } => println!("#{idx} SignalDelivery sig={sig} handler={handler:#x} thread={thread}"),
                }
            }
        }
        Some("low") => {
            // The final snapshot's regions below 0x200000: what a low-valued Scalar could have
            // collided with pre-M37 (a mapped IPA is what `host_span` answers for).
            let events = open(&args[1]);
            if let Some(Event::Snapshot { mem, .. }) = events.last() {
                for r in mem { if r.ipa < 0x200000 { println!("  [{:#x}, {:#x}) len {:#x}", r.ipa, r.ipa + r.bytes.len() as u64, r.bytes.len()); } }
            } else { println!("{}: last event is not a Snapshot", args[1]); }
        }
        Some("writes") => {
            // Dump landmark <idx>'s write regions to <dir>/<idx>-<ipa>.bin (adjudication aid).
            let events = open(&args[1]);
            let idx: usize = args[2].parse().expect("idx");
            let dir = &args[3];
            if let Some(Event::Syscall { num, args: a, ret, err, writes, .. }) = events.get(idx) {
                println!("#{idx} syscall {} args=[{:#x}, {:#x}, {:#x}, {:#x}] ret={ret:#x} err={err} writes={}", *num as i64, a[0], a[1], a[2], a[3], writes.len());
                for r in writes {
                    let f = format!("{dir}/{idx}-{:#x}.bin", r.ipa);
                    std::fs::write(&f, &r.bytes).expect("write");
                    println!("  [{:#x}, {:#x}) len {:#x} -> {f}", r.ipa, r.ipa + r.bytes.len() as u64, r.bytes.len());
                }
            } else { println!("#{idx}: not a Syscall"); }
        }
        Some("slabmap") => {
            // M36's "0x10000 map" rule: `_kernelrpc_mach_vm_map_trap` (-15) with size 0x8000 and
            // flags 0x49000001 (VM_MEMORY_OS_ALLOC_ONCE); print the address its write returned.
            let events = open(&args[1]);
            for (idx, ev) in events.iter().enumerate() {
                if let Event::Syscall { num, args: a, writes, .. } = ev {
                    if *num as i64 == -15 && a[2] == 0x8000 && a[4] == 0x49000001 {
                        let out = writes.iter().find(|r| a[1] >= r.ipa && a[1] + 8 <= r.ipa + r.bytes.len() as u64)
                            .map(|r| { let o = (a[1] - r.ipa) as usize; u64::from_le_bytes(r.bytes[o..o+8].try_into().unwrap()) });
                        println!("#{idx} mach_vm_map(size {:#x}, flags {:#x}) -> {}", a[2], a[4],
                                 out.map(|v| format!("{v:#x}")).unwrap_or_else(|| "(out-pointer not in writes)".into()));
                    }
                }
            }
        }
        _ => { eprintln!("usage: m37reader scalar-writes <trace>… | errs <trace> | selfpid <trace> <pid> | calls <trace> <num> | last <trace> [n]"); std::process::exit(2); }
    }
}
```

## Audit measurement 1 — the static table (Task 3, pasted)

Taken at M37 t3 on branch `m37-classb` after its one row fix. `arg_kinds`
(`crates/retrace-arch/src/lib.rs`) has **129 rows**, **94** with at least one `Scalar` position,
**190 `Scalar` positions**; every one was checked against its prototype (BSD numbers: the SDK's
`sys/syscall.h` for the name, xnu `bsd/kern/syscalls.master` for the parameters; Mach traps:
`osfmk/mach/mach_traps.h`). No position lies beyond its prototype's arity. **One finding, fixed:**
`madvise` (75) x0 (`caddr_ut addr`) was `Scalar` and is forwarded; with `Scalar` meaning "never
probed" the raw IPA would reach the host as the range to `MADV_FREE_REUSABLE` in retrace's own
map — measured on CPython (44 calls): pre-fix `(0,false)` 26 / `(EINVAL,true)` 18; skip + old row
`(EINVAL,true)` 40 / `(EPERM,true)` 4 (the four at `0xa00020000`/`0xa0002c000`, guest IPAs mapped
in retrace's own process); skip + row `[Ptr, Scalar, Scalar]` → `(0,false)` 44. The 18 pre-fix
`EINVAL`s were the LENGTH register (`0x4000`/`0xc000`/`0x10000`/`0x14000`, all mapped IPAs)
probed as a pointer — §4b on a length, and the expected `madvise` `err` drop audit 3 sees below.
Seventeen pointer-TYPED positions are deliberately kept `Scalar` ([K1]–[K8] below); two
integer-typed address positions are listed as [A]. Residuals the audit named and did not act on:
`proc_info` x3 `arg` is an address in the TARGET's map for the region flavors (corpus issues only
callnums 2/5/15 — inert); `fcntl` x2 / `ioctl` x2 are `Ptr` (probed) and NUMBERS for some commands
(`F_SETFD`/`F_SETFL`/`F_NOCACHE`) — §4b's class behind a `Ptr`, unreached on the corpus (CPython's
`fcntl` commands measured: `F_GETPATH`, `F_ADDFILESIGS_RETURN`, `F_CHECK_LV`, `F_SETFD 1`, `F_GETFL`,
`F_GETFD`); and whether a lazily-reclaimed `MADV_FREE_REUSABLE` guest page can read back as zeros
under host memory pressure is unchanged by M37 and unmeasured.

`source` = `sys/syscall.h` for the name + `syscalls.master` for the parameter, unless stated.

| num | name | position | prototype parameter | source |
|---:|---|:--:|---|---|
| 1 | `SYS_exit` | x0 | `int rval` | sys/syscall.h + syscalls.master |
| 3 | `SYS_read` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 4 | `SYS_write` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 5 | `SYS_open` | x1 | `int flags` | sys/syscall.h + syscalls.master |
| 5 | `SYS_open` | x2 | `int mode` | sys/syscall.h + syscalls.master |
| 27 | `SYS_recvmsg` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 28 | `SYS_sendmsg` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 29 | `SYS_recvfrom` | x2 | `size_t len` | sys/syscall.h + syscalls.master |
| 29 | `SYS_recvfrom` | x3 | `int flags` | sys/syscall.h + syscalls.master |
| 33 | `SYS_access` | x1 | `int flags` | sys/syscall.h + syscalls.master |
| 37 | `SYS_kill` | x0 | `int pid` | sys/syscall.h + syscalls.master |
| 37 | `SYS_kill` | x1 | `int signum` | sys/syscall.h + syscalls.master |
| 37 | `SYS_kill` | x2 | `int posix` | sys/syscall.h + syscalls.master |
| 38 | `SYS_crossarch_trap` | x0 | `uint32_t name` | sys/syscall.h + syscalls.master |
| 46 | `SYS_sigaction` | x0 | `int signum` | sys/syscall.h + syscalls.master |
| 48 | `SYS_sigprocmask` | x0 | `int how` | sys/syscall.h + syscalls.master |
| 49 | `SYS_getlogin` | x1 | `u_int namelen` | sys/syscall.h + syscalls.master |
| 54 | `SYS_ioctl` | x1 | `u_long com` | sys/syscall.h + syscalls.master |
| 58 | `SYS_readlink` | x2 | `int count` | sys/syscall.h + syscalls.master |
| 60 | `SYS_umask` | x0 | `int newmask` | sys/syscall.h + syscalls.master |
| 65 | `SYS_msync` | x1 | `size_ut len` | sys/syscall.h + syscalls.master |
| 65 | `SYS_msync` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 73 | `SYS_munmap` | x0 | `caddr_ut addr` **[K1]** | sys/syscall.h + syscalls.master |
| 73 | `SYS_munmap` | x1 | `size_ut len` | sys/syscall.h + syscalls.master |
| 74 | `SYS_mprotect` | x0 | `caddr_ut addr` **[K1]** | sys/syscall.h + syscalls.master |
| 74 | `SYS_mprotect` | x1 | `size_ut len` | sys/syscall.h + syscalls.master |
| 74 | `SYS_mprotect` | x2 | `int prot` | sys/syscall.h + syscalls.master |
| 75 | `SYS_madvise` | x1 | `size_ut len` | sys/syscall.h + syscalls.master |
| 75 | `SYS_madvise` | x2 | `int behav` | sys/syscall.h + syscalls.master |
| 90 | `SYS_dup2` | x1 | `u_int to` | sys/syscall.h + syscalls.master |
| 92 | `SYS_fcntl` | x1 | `int cmd` | sys/syscall.h + syscalls.master |
| 97 | `SYS_socket` | x0 | `int domain` | sys/syscall.h + syscalls.master |
| 97 | `SYS_socket` | x1 | `int type` | sys/syscall.h + syscalls.master |
| 97 | `SYS_socket` | x2 | `int protocol` | sys/syscall.h + syscalls.master |
| 98 | `SYS_connect` | x2 | `socklen_t namelen` | sys/syscall.h + syscalls.master |
| 117 | `SYS_getrusage` | x0 | `int who` | sys/syscall.h + syscalls.master |
| 120 | `SYS_readv` | x2 | `u_int iovcnt` | sys/syscall.h + syscalls.master |
| 121 | `SYS_writev` | x2 | `u_int iovcnt` | sys/syscall.h + syscalls.master |
| 133 | `SYS_sendto` | x2 | `size_t len` | sys/syscall.h + syscalls.master |
| 133 | `SYS_sendto` | x3 | `int flags` | sys/syscall.h + syscalls.master |
| 133 | `SYS_sendto` | x5 | `socklen_t tolen` | sys/syscall.h + syscalls.master |
| 153 | `SYS_pread` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 153 | `SYS_pread` | x3 | `off_t offset` | sys/syscall.h + syscalls.master |
| 154 | `SYS_pwrite` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 154 | `SYS_pwrite` | x3 | `off_t offset` | sys/syscall.h + syscalls.master |
| 169 | `SYS_csops` | x0 | `pid_t pid` | sys/syscall.h + syscalls.master |
| 169 | `SYS_csops` | x1 | `uint32_t ops` | sys/syscall.h + syscalls.master |
| 169 | `SYS_csops` | x3 | `user_size_t usersize` | sys/syscall.h + syscalls.master |
| 170 | `SYS_csops_audittoken` | x0 | `pid_t pid` | sys/syscall.h + syscalls.master |
| 170 | `SYS_csops_audittoken` | x1 | `uint32_t ops` | sys/syscall.h + syscalls.master |
| 170 | `SYS_csops_audittoken` | x3 | `user_size_t usersize` | sys/syscall.h + syscalls.master |
| 184 | `SYS_sigreturn` | x1 | `int infostyle` | sys/syscall.h + syscalls.master |
| 184 | `SYS_sigreturn` | x2 | `user_addr_t token` **[K2]** | sys/syscall.h + syscalls.master |
| 191 | `SYS_pathconf` | x1 | `int name` | sys/syscall.h + syscalls.master |
| 194 | `SYS_getrlimit` | x0 | `u_int which` | sys/syscall.h + syscalls.master |
| 195 | `SYS_setrlimit` | x0 | `u_int which` | sys/syscall.h + syscalls.master |
| 197 | `SYS_mmap` | x0 | `caddr_ut addr` **[K1]** | sys/syscall.h + syscalls.master |
| 197 | `SYS_mmap` | x1 | `size_ut len` | sys/syscall.h + syscalls.master |
| 197 | `SYS_mmap` | x2 | `int prot` | sys/syscall.h + syscalls.master |
| 197 | `SYS_mmap` | x3 | `int flags` | sys/syscall.h + syscalls.master |
| 197 | `SYS_mmap` | x5 | `off_t pos` | sys/syscall.h + syscalls.master |
| 199 | `SYS_lseek` | x1 | `off_t offset` | sys/syscall.h + syscalls.master |
| 199 | `SYS_lseek` | x2 | `int whence` | sys/syscall.h + syscalls.master |
| 202 | `SYS_sysctl` | x1 | `u_int namelen` | sys/syscall.h + syscalls.master |
| 202 | `SYS_sysctl` | x5 | `size_t newlen` | sys/syscall.h + syscalls.master |
| 220 | `SYS_getattrlist` | x3 | `size_t bufferSize` | sys/syscall.h + syscalls.master |
| 220 | `SYS_getattrlist` | x4 | `u_long options` | sys/syscall.h + syscalls.master |
| 228 | `SYS_fgetattrlist` | x3 | `size_t bufferSize` | sys/syscall.h + syscalls.master |
| 228 | `SYS_fgetattrlist` | x4 | `u_long options` | sys/syscall.h + syscalls.master |
| 266 | `SYS_shm_open` | x1 | `int oflag` | sys/syscall.h + syscalls.master |
| 266 | `SYS_shm_open` | x2 | `int mode` | sys/syscall.h + syscalls.master |
| 274 | `SYS_sysctlbyname` | x1 | `size_t namelen` | sys/syscall.h + syscalls.master |
| 274 | `SYS_sysctlbyname` | x5 | `size_t newlen` | sys/syscall.h + syscalls.master |
| 328 | `SYS___pthread_kill` | x0 | `int thread_port` | sys/syscall.h + syscalls.master |
| 328 | `SYS___pthread_kill` | x1 | `int sig` | sys/syscall.h + syscalls.master |
| 329 | `SYS___pthread_sigmask` | x0 | `int how` | sys/syscall.h + syscalls.master |
| 331 | `SYS___disable_threadsignal` | x0 | `int value` | sys/syscall.h + syscalls.master |
| 336 | `SYS_proc_info` | x0 | `int32_t callnum` | sys/syscall.h + syscalls.master |
| 336 | `SYS_proc_info` | x1 | `int32_t pid` | sys/syscall.h + syscalls.master |
| 336 | `SYS_proc_info` | x2 | `uint32_t flavor` | sys/syscall.h + syscalls.master |
| 336 | `SYS_proc_info` | x3 | `uint64_t arg` | sys/syscall.h + syscalls.master |
| 336 | `SYS_proc_info` | x5 | `int32_t buffersize` | sys/syscall.h + syscalls.master |
| 337 | `SYS_sendfile` | x2 | `off_t offset` | sys/syscall.h + syscalls.master |
| 337 | `SYS_sendfile` | x5 | `int flags` | sys/syscall.h + syscalls.master |
| 344 | `SYS_getdirentries64` | x2 | `user_size_t bufsize` | sys/syscall.h + syscalls.master |
| 347 | `SYS_getfsstat64` | x1 | `int bufsize` | sys/syscall.h + syscalls.master |
| 347 | `SYS_getfsstat64` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 360 | `SYS_bsdthread_create` | x0 | `user_addr_t func` **[K3]** | sys/syscall.h + syscalls.master |
| 360 | `SYS_bsdthread_create` | x1 | `user_addr_t func_arg` **[K3]** | sys/syscall.h + syscalls.master |
| 360 | `SYS_bsdthread_create` | x2 | `user_addr_t stack` **[K3]** | sys/syscall.h + syscalls.master |
| 360 | `SYS_bsdthread_create` | x4 | `uint32_t flags` | sys/syscall.h + syscalls.master |
| 361 | `SYS_bsdthread_terminate` | x0 | `user_addr_t stackaddr` **[K4]** | sys/syscall.h + syscalls.master |
| 361 | `SYS_bsdthread_terminate` | x1 | `size_t freesize` | sys/syscall.h + syscalls.master |
| 361 | `SYS_bsdthread_terminate` | x2 | `uint32_t port` | sys/syscall.h + syscalls.master |
| 361 | `SYS_bsdthread_terminate` | x3 | `user_addr_t sema_or_ulock` **[K4]** | sys/syscall.h + syscalls.master |
| 366 | `SYS_bsdthread_register` | x0 | `user_addr_t threadstart` **[K5]** | sys/syscall.h + syscalls.master |
| 366 | `SYS_bsdthread_register` | x1 | `user_addr_t wqthread` **[K5]** | sys/syscall.h + syscalls.master |
| 366 | `SYS_bsdthread_register` | x2 | `uint32_t flags` | sys/syscall.h + syscalls.master |
| 366 | `SYS_bsdthread_register` | x4 | `user_addr_t targetconc_ptr` **[K5]** | sys/syscall.h + syscalls.master |
| 366 | `SYS_bsdthread_register` | x5 | `uint32_t dispatchqueue_offset` | sys/syscall.h + syscalls.master |
| 366 | `SYS_bsdthread_register` | x6 | `uint32_t tsd_offset` | sys/syscall.h + syscalls.master |
| 368 | `SYS_workq_kernreturn` | x0 | `int options` | sys/syscall.h + syscalls.master |
| 368 | `SYS_workq_kernreturn` | x2 | `int affinity` | sys/syscall.h + syscalls.master |
| 368 | `SYS_workq_kernreturn` | x3 | `int prio` | sys/syscall.h + syscalls.master |
| 381 | `SYS___mac_syscall` | x1 | `int call` | sys/syscall.h + syscalls.master |
| 396 | `SYS_read_nocancel` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 397 | `SYS_write_nocancel` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 398 | `SYS_open_nocancel` | x1 | `int flags` | sys/syscall.h + syscalls.master |
| 398 | `SYS_open_nocancel` | x2 | `int mode` | sys/syscall.h + syscalls.master |
| 401 | `SYS_recvmsg_nocancel` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 402 | `SYS_sendmsg_nocancel` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 403 | `SYS_recvfrom_nocancel` | x2 | `size_t len` | sys/syscall.h + syscalls.master |
| 403 | `SYS_recvfrom_nocancel` | x3 | `int flags` | sys/syscall.h + syscalls.master |
| 405 | `SYS_msync_nocancel` | x1 | `size_ut len` | sys/syscall.h + syscalls.master |
| 405 | `SYS_msync_nocancel` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 406 | `SYS_fcntl_nocancel` | x1 | `int cmd` | sys/syscall.h + syscalls.master |
| 411 | `SYS_readv_nocancel` | x2 | `u_int iovcnt` | sys/syscall.h + syscalls.master |
| 412 | `SYS_writev_nocancel` | x2 | `u_int iovcnt` | sys/syscall.h + syscalls.master |
| 413 | `SYS_sendto_nocancel` | x2 | `size_t len` | sys/syscall.h + syscalls.master |
| 413 | `SYS_sendto_nocancel` | x3 | `int flags` | sys/syscall.h + syscalls.master |
| 413 | `SYS_sendto_nocancel` | x5 | `socklen_t tolen` | sys/syscall.h + syscalls.master |
| 414 | `SYS_pread_nocancel` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 414 | `SYS_pread_nocancel` | x3 | `off_t offset` | sys/syscall.h + syscalls.master |
| 415 | `SYS_pwrite_nocancel` | x2 | `user_size_t nbyte` | sys/syscall.h + syscalls.master |
| 415 | `SYS_pwrite_nocancel` | x3 | `off_t offset` | sys/syscall.h + syscalls.master |
| 427 | `SYS_fsgetpath` | x1 | `size_t bufsize` | sys/syscall.h + syscalls.master |
| 427 | `SYS_fsgetpath` | x3 | `uint64_t objid` | sys/syscall.h + syscalls.master |
| 463 | `SYS_openat` | x2 | `int flags` | sys/syscall.h + syscalls.master |
| 463 | `SYS_openat` | x3 | `int mode` | sys/syscall.h + syscalls.master |
| 470 | `SYS_fstatat64` | x3 | `int flag` | sys/syscall.h + syscalls.master |
| 478 | `SYS_bsdthread_ctl` | x0 | `user_addr_t cmd` **[K6]** | sys/syscall.h + syscalls.master |
| 478 | `SYS_bsdthread_ctl` | x1 | `user_addr_t arg1` **[K6]** | sys/syscall.h + syscalls.master |
| 478 | `SYS_bsdthread_ctl` | x2 | `user_addr_t arg2` **[K6]** | sys/syscall.h + syscalls.master |
| 480 | `SYS_recvmsg_x` | x2 | `u_int cnt` | sys/syscall.h + syscalls.master |
| 480 | `SYS_recvmsg_x` | x3 | `int flags` | sys/syscall.h + syscalls.master |
| 481 | `SYS_sendmsg_x` | x2 | `u_int cnt` | sys/syscall.h + syscalls.master |
| 481 | `SYS_sendmsg_x` | x3 | `int flags` | sys/syscall.h + syscalls.master |
| 483 | `SYS_csrctl` | x0 | `uint32_t op` | sys/syscall.h + syscalls.master |
| 483 | `SYS_csrctl` | x2 | `user_addr_t usersize` **[K7]** | sys/syscall.h + syscalls.master |
| 500 | `SYS_getentropy` | x1 | `size_t size` | sys/syscall.h + syscalls.master |
| 515 | `SYS_ulock_wait` | x0 | `uint32_t operation` | sys/syscall.h + syscalls.master |
| 515 | `SYS_ulock_wait` | x2 | `uint64_t value` | sys/syscall.h + syscalls.master |
| 515 | `SYS_ulock_wait` | x3 | `uint32_t timeout` | sys/syscall.h + syscalls.master |
| 516 | `SYS_ulock_wake` | x0 | `uint32_t operation` | sys/syscall.h + syscalls.master |
| 516 | `SYS_ulock_wake` | x1 | `void *addr` **[K8]** | sys/syscall.h + syscalls.master |
| 516 | `SYS_ulock_wake` | x2 | `uint64_t wake_value` | sys/syscall.h + syscalls.master |
| 539 | `SYS_task_read_for_pid` | x0 | `mach_port_name_t target_tport` | sys/syscall.h + syscalls.master |
| 539 | `SYS_task_read_for_pid` | x1 | `int pid` | sys/syscall.h + syscalls.master |
| 540 | `SYS_preadv` | x2 | `int iovcnt` | sys/syscall.h + syscalls.master |
| 540 | `SYS_preadv` | x3 | `off_t offset` | sys/syscall.h + syscalls.master |
| 541 | `SYS_pwritev` | x2 | `int iovcnt` | sys/syscall.h + syscalls.master |
| 541 | `SYS_pwritev` | x3 | `off_t offset` | sys/syscall.h + syscalls.master |
| 550 | `SYS_map_with_linking_np` | x1 | `uint32_t region_count` | sys/syscall.h + syscalls.master |
| 550 | `SYS_map_with_linking_np` | x3 | `uint32_t link_info_size` | sys/syscall.h + syscalls.master |
| 0x8000_0000 | `MAC_SYSCALL_MAGIC (__mac_syscall shape)` | x1 | `int call` | retrace_arch (not a syscall number; dyld's inline `__mac_syscall`), shape of 381 per syscalls.master |
| -10 | `_kernelrpc_mach_vm_allocate_trap` | x0 | `mach_port_name_t target` | osfmk/mach/mach_traps.h |
| -10 | `_kernelrpc_mach_vm_allocate_trap` | x2 | `mach_vm_size_t size` | osfmk/mach/mach_traps.h |
| -10 | `_kernelrpc_mach_vm_allocate_trap` | x3 | `int flags` | osfmk/mach/mach_traps.h |
| -12 | `_kernelrpc_mach_vm_deallocate_trap` | x0 | `mach_port_name_t target` | osfmk/mach/mach_traps.h |
| -12 | `_kernelrpc_mach_vm_deallocate_trap` | x1 | `mach_vm_address_t address` **[A]** | osfmk/mach/mach_traps.h |
| -12 | `_kernelrpc_mach_vm_deallocate_trap` | x2 | `mach_vm_size_t size` | osfmk/mach/mach_traps.h |
| -14 | `_kernelrpc_mach_vm_protect_trap` | x0 | `mach_port_name_t target` | osfmk/mach/mach_traps.h |
| -14 | `_kernelrpc_mach_vm_protect_trap` | x1 | `mach_vm_address_t address` **[A]** | osfmk/mach/mach_traps.h |
| -14 | `_kernelrpc_mach_vm_protect_trap` | x2 | `mach_vm_size_t size` | osfmk/mach/mach_traps.h |
| -14 | `_kernelrpc_mach_vm_protect_trap` | x3 | `boolean_t set_maximum` | osfmk/mach/mach_traps.h |
| -14 | `_kernelrpc_mach_vm_protect_trap` | x4 | `vm_prot_t new_protection` | osfmk/mach/mach_traps.h |
| -15 | `_kernelrpc_mach_vm_map_trap` | x0 | `mach_port_name_t target` | osfmk/mach/mach_traps.h |
| -15 | `_kernelrpc_mach_vm_map_trap` | x2 | `mach_vm_size_t size` | osfmk/mach/mach_traps.h |
| -15 | `_kernelrpc_mach_vm_map_trap` | x3 | `mach_vm_offset_t mask` | osfmk/mach/mach_traps.h |
| -15 | `_kernelrpc_mach_vm_map_trap` | x4 | `int flags` | osfmk/mach/mach_traps.h |
| -15 | `_kernelrpc_mach_vm_map_trap` | x5 | `vm_prot_t cur_protection` | osfmk/mach/mach_traps.h |
| -18 | `_kernelrpc_mach_port_deallocate_trap` | x0 | `mach_port_name_t target` | osfmk/mach/mach_traps.h |
| -18 | `_kernelrpc_mach_port_deallocate_trap` | x1 | `mach_port_name_t name` | osfmk/mach/mach_traps.h |
| -19 | `_kernelrpc_mach_port_mod_refs_trap` | x0 | `mach_port_name_t target` | osfmk/mach/mach_traps.h |
| -19 | `_kernelrpc_mach_port_mod_refs_trap` | x1 | `mach_port_name_t name` | osfmk/mach/mach_traps.h |
| -19 | `_kernelrpc_mach_port_mod_refs_trap` | x2 | `mach_port_right_t right` | osfmk/mach/mach_traps.h |
| -19 | `_kernelrpc_mach_port_mod_refs_trap` | x3 | `mach_port_delta_t delta` | osfmk/mach/mach_traps.h |
| -24 | `_kernelrpc_mach_port_construct_trap` | x0 | `mach_port_name_t target` | osfmk/mach/mach_traps.h |
| -24 | `_kernelrpc_mach_port_construct_trap` | x2 | `uint64_t context` | osfmk/mach/mach_traps.h |
| -33 | `semaphore_signal_trap` | x0 | `mach_port_name_t signal_name` | osfmk/mach/mach_traps.h |
| -36 | `semaphore_wait_trap` | x0 | `mach_port_name_t wait_name` | osfmk/mach/mach_traps.h |
| -47 | `mach_msg2_trap` | x1 | `mach_msg_option64_t options` | osfmk/mach/mach_traps.h |
| -47 | `mach_msg2_trap` | x2 | `uint64_t msgh_bits_and_send_size` | osfmk/mach/mach_traps.h |
| -47 | `mach_msg2_trap` | x3 | `uint64_t msgh_remote_and_local_port` | osfmk/mach/mach_traps.h |
| -47 | `mach_msg2_trap` | x4 | `uint64_t msgh_voucher_and_id` | osfmk/mach/mach_traps.h |
| -47 | `mach_msg2_trap` | x5 | `uint64_t desc_count_and_rcv_name` | osfmk/mach/mach_traps.h |
| -47 | `mach_msg2_trap` | x6 | `uint64_t rcv_size_and_priority` | osfmk/mach/mach_traps.h |
| -47 | `mach_msg2_trap` | x7 | `uint64_t timeout` | osfmk/mach/mach_traps.h |
| -70 | `host_create_mach_voucher_trap` | x0 | `mach_port_name_t host` | osfmk/mach/mach_traps.h |
| -70 | `host_create_mach_voucher_trap` | x2 | `int recipes_size` | osfmk/mach/mach_traps.h |

### Pointer-typed positions kept `Scalar` — the rulings

- **[K1] `munmap` (73) x0, `mprotect` (74) x0, `mmap` (197) x0 — `caddr_ut addr`.** A VM range /
  placement hint the kernel acts on in the caller's map, never data. All three are **emulated
  above the trace** (`retrace-core/src/lib.rs`: the `SYS_MMAP` arms at `:272`/`:290`, `SYS_MUNMAP`
  `:311`, `SYS_MPROTECT` `:316`; file-backed mmap via `guest_mmap_file`) and never reach
  `forward_and_diff` — M33's rustdoc: "A row for a call serviced or emulated above the trace is
  documentation". Contrast `madvise`, the one forwarded member of this family, which is the
  finding. Forwarding any of these three would be wrong with EITHER kind (a rebased `munmap`
  unmaps the guest backing from under HVF), so the kind cannot make them safe; emulation is the
  only sound handling and it is what the tree does.
- **[K2] `sigreturn` (184) x2 — `user_addr_t token`.** A value the kernel COMPARES:
  `bsd/dev/arm/unix_signal.c` builds it as `token_uctx ^ ut->uu_sigreturn_token` (`:594`) and
  `sigreturn` checks the caller's copy against that — never dereferenced. Serviced above the trace
  (M11, the `SYS_SIGRETURN` arm `:937`). Probing it would corrupt the comparison whenever the token
  happened to equal a mapped IPA.
- **[K3] `bsdthread_create` (360) x0/x1/x2 — `user_addr_t func`, `func_arg`, `stack`.** Values
  the kernel installs in the new thread's `pc`/`x0`/`sp`; the only memory it writes is through
  x3 (`pthread`, the row's `Ptr`). Emulated at retrace-core's `bsdthread_create` arm (~`:1006`);
  NOT separately asserted against — the arm's position is the only guard (the M33 row comment and
  CLAUDE.md say "asserted against"; the generic arm's asserts are `is_signal_syscall`, the workq
  pair and `writes_via_nested_pointer` only).
- **[K4] `bsdthread_terminate` (361) x0/x3 — `user_addr_t stackaddr`, `sema_or_ulock`.**
  `stackaddr` is a VM range to `mach_vm_deallocate` (like [K1]); `sema_or_ulock` is a semaphore
  port name or a ulock wait-queue key (libpthread `_bsdthread_terminate`) — no user memory is read
  or written. Emulated (M14, the arm at `:1017`).
- **[K5] `bsdthread_register` (366) x0/x1/x4 — `user_addr_t threadstart`, `wqthread`,
  `targetconc_ptr`.** `threadstart`/`wqthread` are code addresses the kernel records and never
  follows. x4 is the master's stale name: libpthread's `_bsdthread_register(p, threadstart,
  wqthread, pthsize, pthread_init_data, pthread_init_data_size, dispatchqueue_offset, retval)`
  (`kern/kern_support.c:535–542`, fetched from `apple-oss-distributions/libpthread`) binds the
  fifth master argument as `pthread_init_data_size` — a SIZE, compared to `sizeof(data.version)`
  (`:558`), `MIN`'d into the `copyin`/`copyout` length for x3 (`:561–562`, `:660`, the row's
  `Ptr`) and compared to `data.version` (`:566`). A
  size that equalled a mapped IPA would, if probed, become a host pointer used as a length — the
  §4b defect. Emulated since M18 Stage 1 (the arm at `:963`).
- **[K6] `bsdthread_ctl` (478) x0/x1/x2 — `user_addr_t cmd`, `arg1`, `arg2`.** FORWARDED (the
  census saw it from `panicky`), so this ruling carries weight. `bsd/pthread/pthread_workqueue.c`
  `bsdthread_ctl` switches on `cmd` and casts `arg1`/`arg2` to `mach_port_name_t`,
  `pthread_priority_t`, `thread_qos_t`, `unsigned long`, `bool`, `int` or `uint64_t` in every
  arm; the one dereference in the switch is `arg3` (`copyin_atomic32` on the ulock in
  `workq_thread_add_dispatch_override`), which the row already marks `Ptr`. Kept `Scalar`;
  post-fix these three are forwarded verbatim, which is strictly more faithful than before.
- **[K7] `csrctl` (483) x2 — `user_addr_t usersize`.** A size with a pointer type:
  `bsd/kern/kern_csr.c` `:355` and `:373` test `args->usersize != sizeof(mask|config)` and never
  dereference it; the pointer is x1 (`useraddr`, the row's `Ptr`, 4 bytes in or out).
- **[K8] `ulock_wake` (516) x1 — `void *addr`.** A wait-queue KEY (`bsd/kern/sys_ulock.c`
  `ulock_wake`: `key.ulk_addr = addr` then `ull_get(&key, ULL_MUST_EXIST, …)`; the function has
  no `copyin`), as the row's own comment says. Emulated (M14, the arm
  at `:1059`). Contrast `ulock_wait` (515), where the kernel DOES read 4/8 bytes at `addr` and the
  row says `Ptr`.
- **[A] `_kernelrpc_mach_vm_deallocate_trap` (-12) x1, `_kernelrpc_mach_vm_protect_trap` (-14)
  x1 — `mach_vm_address_t address`.** Integer-typed (not a finding under the brief's rule) but
  address-valued; both traps are emulated (`MACH_VM_DEALLOCATE` `:421`, `MACH_VM_PROTECT` `:432`
  in retrace-core) and never forwarded. Listed so the next audit does not rediscover them.


## Audit measurement 2 — `scalar-writes`

Run over every kept trace named by the brief: the 54 Task 1 baseline traces, the 3 × 54 post-fix
traces (N, I, S — `yes` included, read before deletion), M36's 27 kept traces (`keep-{O,L,I}`) and
M35's 6 `dddiagnose` probes (`ddd-keep*/`). Outputs: `audit/scalar-writes-{base,N,I,S,m35m36}.log`.

| corpus | traces | hits | in which traces |
|---|---|---|---|
| baseline (pre-fix, non-colliding) | 54 | 9 | `desdp` 3, `dyld_info` 3, `flex` 3 |
| N (post-fix, non-colliding) | 54 | 9 | `desdp` 3, `dyld_info` 3, `flex` 3 |
| I (post-fix, `[0x4000,0x10000)`) | 54 | 9 | `desdp` 3, `dyld_info` 3, `flex` 3 |
| S (post-fix, `[0x10000,0x18000)`) | 54 | 9 | `desdp` 3, `dyld_info` 3, `flex` 3 |
| M36 O/L/I (pre-fix) | 27 | 27 | the same three binaries × 3 runs × 3 |
| M35 `ddd-keep*` (pre-fix, `dddiagnose`) | 6 | 0 | — |

(The `yes` traces alone carry ~3.3 million `Scalar` positions each — the `write` count register —
and 0 hits; a typical trace has a few thousand.)

**Every hit is one shape and it is not a probe.** All 63 hits are `mmap` (197) x0 with
`flags = 0x40012` (`MAP_FIXED | MAP_PRIVATE | MAP_UNIX03`), `prot` 5 / 3 / 1 in that order, sizes
`0xc000` / `0x4000` / `0x8000`, `ret == x0`, and the write region is exactly `[x0, x0 + len)` — the
three segments of one non-cache dylib the guest maps `MAP_FIXED` from a file (N, `desdp` #270–#272,
verbatim from `scalar-writes-N.log`):

```
HIT …/keep-N/desdp.bin #270 syscall 197 x0=0xa00554000 inside write [0xa00554000,0xa00560000) args=[0xa00554000, 0xc000, 0x5, 0x40012] ret=0xa00554000
HIT …/keep-N/desdp.bin #271 syscall 197 x0=0xa00560000 inside write [0xa00560000,0xa00564000) args=[0xa00560000, 0x4000, 0x3, 0x40012] ret=0xa00560000
HIT …/keep-N/desdp.bin #272 syscall 197 x0=0xa00568000 inside write [0xa00568000,0xa00570000) args=[0xa00568000, 0x8000, 0x1, 0x40012] ret=0xa00568000
```

`mmap` is **emulated above the trace** (`retrace-core/src/lib.rs` `SYS_MMAP` arms; a file-backed
`MAP_FIXED` goes through `place_fixed`, which `pread`s the file bytes into the anon backing at the
address the guest named and records them as this landmark's write — CLAUDE.md "SPTM / anon-only
memory"). The call never reaches `forward_and_diff`, so its x0 was never probed before M37 and is
never forwarded after it; the position is Task 3's **[K1]** ruling, an address the box itself acts
on, and the write is the box's own staging, not a kernel dereference. The literal rule "a write
region containing a `Scalar` value" cannot tell an emulated call's placement write from a
dereference; the number the audit owes — hits on a **forwarded** syscall, or on any position other
than [K1] — is **0** in every corpus, pre- and post-fix. The `mmap` row is not changed (`Ptr` there
would document a probe the arm never performs). Hits are identical in count and shape across the
three post-fix regimes and the pre-fix corpora because the position's handling did not change.

## Audit measurement 3 — baseline vs N (`audit/baseline-vs-N.md` has the same content)

**Labels.** `ROW` fields (result, rc, rp, reason) for all 54 rows, baseline vs N: identical except
the two expected moves — `csh` and `tcsh`, `recorder panicked: … crates/retrace-core/src/lib.rs:1140:17:
dup2 is not modelled by the M10 fd table …` (rc 101, no replay) → `record error, rc=4: RECORD ERROR:
unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 (guest task port Some(515))
send_size 64` (rc/rp 4/3). Landmarks differ on the six §4b rows by the run-to-run spread explained
above (baseline / N: `launchctl` 329/338, `automationmodetool` 338/343, `desdp` 365/366,
`dyld_info` 364/365, `flex` 366/362, `dddiagnose` 382/383).

**`errs` per syscall number**, reader `errs` over the 53 baseline/N trace pairs (`yes` excluded as
killed mid-run; outputs `audit/errs-{base,N}/<b>.txt`, diffed per binary): **identical for 49 of
53**. The four differences, adjudicated by name:

| binary | baseline → N | adjudication |
|---|---|---|
| `csh` | `ioctl`(54) 3 → 7, `fcntl`(92) 0 → 2 | The N trace is longer (261 → 331 events): the baseline panicked at the `dup2` assert before recording it. Over N's first 261 events `errs` is **identical** to the baseline's (reader `errs <trace> 261`); every extra `err` is past the old stop (`ioctl` `FIODTYPE`/`TIOCGETA` on the new aliases 17 and 18 answered `ENOTTY` — the sweep's stdout/stderr are files, #271–#274; two `fcntl(F_SETFD)` `EBADF` at #329/#330). Not a probe delta. |
| `tcsh` | `ioctl`(54) 3 → 7, `fcntl`(92) 0 → 2 | Same: identical over the baseline's 265 events; the rest is past the old `dup2` stop. |
| `date` | `madvise`(75) 4 → 0 | Task 3's expected delta (a). The four baseline failures are `madvise(0xa00538000/0xa0052c000, len 0xc000, 7)` → `EINVAL`: `0xc000` is `PT_L1_IPA`'s backing (mapped in every guest, `[0xc000,0x10000)` in the final snapshot), so the pre-fix probe rewrote the LENGTH to a host pointer. Post-fix the same four calls return 0. |
| `zsh` | `madvise`(75) 2 → 0 | Same, two calls with `len 0xc000`. |
| `ps` | `madvise`(75) 32 → 0 | Same class, `len 0x100000`, inside the guest's `[0x4c000, 0x454000)` region (final snapshot). All 32 post-fix calls return 0. |
| `ps` | `sysctl`(202) 0 → 1 | **Not the fix.** The one `err` is `sysctl({CTL_KERN, KERN_PROCARGS2, pid}, 3, buf, &len)` → `EINVAL` at N #7916, the 5th of `ps`'s 15 per-process argument fetches; the same 15 calls succeed in the baseline, in I and in S (`errs` N-vs-I and N-vs-S differ in nothing else). The target had exited between `ps`'s `KERN_PROC` listing (#269) and this call — `ps`'s own stdout (`keep-N/ps.rp.out`, byte-identical on record and replay, so the row is `PASS`) prints that row as `35890 ttys001 0:00.00 (caffeinate)`, the parenthesised form `ps` uses when `KERN_PROCARGS2` fails, where the baseline printed `59315 ttys001 0:00.00 caffeinate -i -t 300`. `sysctl`'s `Scalar` positions are `namelen` (3) and `newlen` (0), neither a mapped IPA in any regime (nothing is backed below `0x4000`), so the probe never touched them pre-fix and forwarding them verbatim post-fix hands the kernel the same values; the host's process table is a forwarded input. |

No `ret=14` (`EFAULT`) appeared on any row with a `Scalar` position in any post-fix trace; no
table row was changed by this task and no binary was re-run.

**Regime independence (spec §2b's claim, extended to the whole corpus).** `errs` N-vs-I and N-vs-S,
53 pairs each: identical for 52; the only difference is the `ps` `sysctl` above (present in N
only). No syscall's error count moves with the recorder's pid any more.

## Acceptance — self-pid `ESRCH` (spec §5)

Reader `selfpid <trace> <recpid>` over **every** kept trace of runs I and S (54 each, `yes`
included: in run I `yes` was re-recorded alone at pid 20384 after its sweep trace had been deleted —
`sweeps/yes-I-rerun.log`, `RETRACE_SWEEP_LIST` with one entry, the same binary — and read before
deletion), plus run N (53). Sum of `self-pid ESRCH` = **0** in I (54 traces), **0** in S (54),
**0** in N (53); outputs `audit/selfpid-{N,I,S}.log`. The nine rows (`calls` = landmarks carrying
the recorder's pid, `ESRCH` = of those answered 3 with `err`):

| row | N calls / ESRCH | I calls / ESRCH | S calls / ESRCH |
|---|---|---|---|
| `csh` | 7 / 0 | 7 / 0 | 7 / 0 |
| `tcsh` | 7 / 0 | 7 / 0 | 7 / 0 |
| `launchctl` | 12 / 0 | 12 / 0 | 12 / 0 |
| `automationmodetool` | 12 / 0 | 12 / 0 | 12 / 0 |
| `desdp` | 12 / 0 | 12 / 0 | 12 / 0 |
| `dyld_info` | 12 / 0 | 12 / 0 | 12 / 0 |
| `flex` | 12 / 0 | 12 / 0 | 12 / 0 |
| `dddiagnose` | 13 / 0 | 13 / 0 | 13 / 0 |
| `yes` | (deleted before the count) | 5 / 0 (pid 20384, re-recorded) | 5 / 0 |

For contrast, the same reader on M36's colliding traces: `dddiagnose` keep-I 12 / **11**, keep-O
13 / **12** (the rule's positive control above), and the six §4b rows' `brk` / identical-fault
faces of M36 are gone: all six stop at the RCV-shaped call in all three regimes.

## The `dup2` measurement (spec §2a), now from kept traces

Both C shells issue exactly four `dup2` calls, each followed by `fcntl(new, F_SETFD, 1)`, in every
regime (reader `calls <trace> 90` / `92`; N `csh` shown, the other five traces have the same four
with the same arguments and results at indices ±3):

```
#263 syscall 90 args=[0x0, 0x10, …] ret=0x10 err=false      dup2(0, 16)   then #264 fcntl(16, F_SETFD, 1) → 0
#265 syscall 90 args=[0x1, 0x11, …] ret=0x11 err=false      dup2(1, 17)   then #266 fcntl(17, F_SETFD, 1) → 0
#267 syscall 90 args=[0x2, 0x12, …] ret=0x12 err=false      dup2(2, 18)   then #268 fcntl(18, F_SETFD, 1) → 0
#269 syscall 90 args=[0x10, 0x13, …] ret=0x13 err=false     dup2(16, 19)  then #270 fcntl(19, F_SETFD, 1) → 0
```

Then ~60 landmarks later both stop at `RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34:
msgh_id 3403 dest 0x203 (guest task port Some(515)) send_size 64`. `3403` is `mach_ports_register`
(the SDK's `mach/task.defs`: `subsystem task 3400`, third routine), a complex message the router does
not know; its backtrace, measured in the M37 pre-flight with `dladdr` and recorded in spec §2a (not
re-derived here), is `_kernelrpc_mach_ports_register3+0x88` ← `mach_ports_register+0x80` ← libxpc
`xpc_atfork_prepare+0x50` ← `libSystem_atfork_prepare+0x28` ← libsystem_c `fork+0x24`. The wall
behind `dup2` is `fork`; `fork`(2) has no `arg_kinds` row and process creation is charter class C.

## Files

One `<basename>.<run>.<phase>.err` per non-clean row and run, verbatim from `keep-<run>/`.
`rec.err` is the recorder's stderr (first line `recpid=<pid>`, printed by the wrapper shell that
`exec`s into the recorder); `rp.err` is the replay's, absent when replay did not run (the `yes`
timeout ends the row before replay). 27 `rec.err` + 24 `rp.err`.

- `automationmodetool.I.rec.err` — run I (recpid 19193), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `automationmodetool.I.rp.err` — run I, replay ran out of events at the same landmark (340, `expected recorded syscall, got None`)
- `automationmodetool.N.rec.err` — run N (recpid 2593), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `automationmodetool.N.rp.err` — run N, replay ran out of events at the same landmark (343, `expected recorded syscall, got None`)
- `automationmodetool.S.rec.err` — run S (recpid 68104), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `automationmodetool.S.rp.err` — run S, replay ran out of events at the same landmark (339, `expected recorded syscall, got None`)
- `csh.I.rec.err` — run I (recpid 17385), the `mach_ports_register` (msgh_id 3403) wall after the four modelled `dup2`s; 0 self-pid ESRCH in the kept trace
- `csh.I.rp.err` — run I, replay ran out of events at the same landmark (330, `expected recorded syscall, got None`)
- `csh.N.rec.err` — run N (recpid 1005), the `mach_ports_register` (msgh_id 3403) wall after the four modelled `dup2`s; 0 self-pid ESRCH in the kept trace
- `csh.N.rp.err` — run N, replay ran out of events at the same landmark (331, `expected recorded syscall, got None`)
- `csh.S.rec.err` — run S (recpid 66385), the `mach_ports_register` (msgh_id 3403) wall after the four modelled `dup2`s; 0 self-pid ESRCH in the kept trace
- `csh.S.rp.err` — run S, replay ran out of events at the same landmark (334, `expected recorded syscall, got None`)
- `dddiagnose.I.rec.err` — run I (recpid 19315), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `dddiagnose.I.rp.err` — run I, replay ran out of events at the same landmark (380, `expected recorded syscall, got None`)
- `dddiagnose.N.rec.err` — run N (recpid 2718), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `dddiagnose.N.rp.err` — run N, replay ran out of events at the same landmark (383, `expected recorded syscall, got None`)
- `dddiagnose.S.rec.err` — run S (recpid 68226), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `dddiagnose.S.rp.err` — run S, replay ran out of events at the same landmark (384, `expected recorded syscall, got None`)
- `desdp.I.rec.err` — run I (recpid 19226), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `desdp.I.rp.err` — run I, replay ran out of events at the same landmark (366, `expected recorded syscall, got None`)
- `desdp.N.rec.err` — run N (recpid 2626), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `desdp.N.rp.err` — run N, replay ran out of events at the same landmark (366, `expected recorded syscall, got None`)
- `desdp.S.rec.err` — run S (recpid 68136), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `desdp.S.rp.err` — run S, replay ran out of events at the same landmark (365, `expected recorded syscall, got None`)
- `dyld_info.I.rec.err` — run I (recpid 19255), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `dyld_info.I.rp.err` — run I, replay ran out of events at the same landmark (367, `expected recorded syscall, got None`)
- `dyld_info.N.rec.err` — run N (recpid 2655), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `dyld_info.N.rp.err` — run N, replay ran out of events at the same landmark (365, `expected recorded syscall, got None`)
- `dyld_info.S.rec.err` — run S (recpid 68167), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `dyld_info.S.rp.err` — run S, replay ran out of events at the same landmark (366, `expected recorded syscall, got None`)
- `flex.I.rec.err` — run I (recpid 19285), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `flex.I.rp.err` — run I, replay ran out of events at the same landmark (369, `expected recorded syscall, got None`)
- `flex.N.rec.err` — run N (recpid 2688), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `flex.N.rp.err` — run N, replay ran out of events at the same landmark (362, `expected recorded syscall, got None`)
- `flex.S.rec.err` — run S (recpid 68197), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `flex.S.rp.err` — run S, replay ran out of events at the same landmark (366, `expected recorded syscall, got None`)
- `launchctl.I.rec.err` — run I (recpid 18162), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `launchctl.I.rp.err` — run I, replay ran out of events at the same landmark (330, `expected recorded syscall, got None`)
- `launchctl.N.rec.err` — run N (recpid 1601), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `launchctl.N.rp.err` — run N, replay ran out of events at the same landmark (338, `expected recorded syscall, got None`)
- `launchctl.S.rec.err` — run S (recpid 66837), the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid ESRCH in the kept trace
- `launchctl.S.rp.err` — run S, replay ran out of events at the same landmark (331, `expected recorded syscall, got None`)
- `tcsh.I.rec.err` — run I (recpid 18983), the `mach_ports_register` (msgh_id 3403) wall after the four modelled `dup2`s; 0 self-pid ESRCH in the kept trace
- `tcsh.I.rp.err` — run I, replay ran out of events at the same landmark (332, `expected recorded syscall, got None`)
- `tcsh.N.rec.err` — run N (recpid 2342), the `mach_ports_register` (msgh_id 3403) wall after the four modelled `dup2`s; 0 self-pid ESRCH in the kept trace
- `tcsh.N.rp.err` — run N, replay ran out of events at the same landmark (329, `expected recorded syscall, got None`)
- `tcsh.S.rec.err` — run S (recpid 67896), the `mach_ports_register` (msgh_id 3403) wall after the four modelled `dup2`s; 0 self-pid ESRCH in the kept trace
- `tcsh.S.rp.err` — run S, replay ran out of events at the same landmark (332, `expected recorded syscall, got None`)
- `yes.I.rec.err` — run I (recpid 19820), recorder stderr up to the 30 s SIGKILL; no `RECORD ERROR`, no replay ran
- `yes.N.rec.err` — run N (recpid 3145), recorder stderr up to the 30 s SIGKILL; no `RECORD ERROR`, no replay ran
- `yes.S.rec.err` — run S (recpid 68650), recorder stderr up to the 30 s SIGKILL; no `RECORD ERROR`, no replay ran
