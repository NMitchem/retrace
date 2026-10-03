# M48-node Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make real Homebrew `node` (v25.6.1, JIT on) record and replay bit-identically, print `1`, run a 2-second `setTimeout` on the synthetic clock, and crash in a repo-owned script whose corrupting store is reverse-continued into V8's JIT code. On the way:
- fix a pre-existing hv-sys ABI bug that writes host garbage into guest SIMD registers;
- give every guest partial `munmap`, guest kqueues, psynch condition variables, deadline-bounded waits and `MAP_JIT` write-protect.

**Architecture:**
- **t0 (Task 0).** The controller's scratch probe already walked node to exit with stubs (`walls.md`, below). Task 0 does four things:
  - copies the probe's patch, scripts, native replicas and crash demo into the ledger;
  - re-runs every walk on a release build of that patch, as committed scripts;
  - re-takes the measurements the probe left open: the psynch return words, where the 1 ns waits come from, a closed reader's `EV_EOF`, and the SIMD fixture on the base binary;
  - writes the measurements file that later tasks read through controller addenda.
- **SIMD (Task 1).** `hv_sys::Vcpu::set_simd` calls the framework through inline asm, with the 16-byte vector in `v0` as AAPCS64 requires. Every SIMD restore (a thread switch, a `sigreturn`, a checkpoint restore, a debugger register write) then installs the value it was given. This lands first because every node gate depends on it.
- **Rows (Task 2).**
  - `kevent` (363), the three psynch condition-variable calls (303–305) and `setsockopt` (105) get `arg_kinds` rows.
  - `semaphore_destroy` (msgh_id 3419) joins `FORWARD_ALLOWLIST`.
  - The generic forward arm asserts against 363 and every SDK psynch number.
- **Partial munmap (Task 3).** `guest_munmap` splits a backing that the unmapped range cuts. The head and tail stay mapped over the same host pages, and only the cut pages are released. V8 trims its aligned reservations this way.
- **One deadline queue and guest kqueues (Task 4).**
  - `BlockReason` gains `Kevent` and, in Task 5, `Cv`, each with an optional deadline.
  - `schedule_after_block` wakes due waiters, and its one idle jump goes to the earliest of all deadlines.
  - A pure `gkq.rs` (K1) holds each guest kqueue's knotes.
  - `Box_::guest_kevent` applies changes, then returns or blocks.
  - A woken thread's reply is written at the wake.
- **psynch (Task 5).** A pure `psynch.rs` (P1) ports libpthread-539.100.4 `kern/kern_synch.c`'s condition-variable paths (`cvwait`, `cvsignal`, `cvbroad`), keyed by guest address. The mutex calls and `cvclrprepost` are refused by value and never forwarded.
- **JIT write-protect (Task 6).**
  - `S3_6_C15_C1_5` is emulated per thread in the EC 0x00 path beside `try_emulate_undef_mrs`.
  - A pure `jit.rs` holds the `MAP_JIT` ranges.
  - The stamped view is the ranges minus their no-access extents. It flips on a toggle and on a thread switch.
  - `flush_guest_tlb` becomes step-safe.
  - SCTLR gains UCI for every guest.
- **Node (Tasks 7–8), the walk and the sweep (Task 9), the audit (Task 10), the close (Task 11).**
- **No trace-format change.** `TRACE_MAGIC` stays `RT\x00\x0b`, and `crates/retrace-trace` has no diff.

**Tech Stack:**
- Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, cargo tests.
- clang for the guest fixtures and the N-API addon.
- Homebrew `node` 25.6.1 and its `node_api.h`.
- lldb only where named.
- POSIX `bash` and `perl` for bounded runs.
- A release build of `retrace` is used only for t0's walks. Every gate drives the debug `CARGO_BIN_EXE_retrace`.

**Spec:** `docs/superpowers/specs/2026-10-02-retrace-m48-node-design.md`. The controller commits it before Task 0 and corrects it from this plan; see its §11, whose items this plan cites as `§11a item N` and `§11b item N`. Sections and rulings are cited as `M48 §3c`, `R3` and so on.

**Companions:**
- **The measurements file**, `docs/superpowers/specs/2026-10-02-retrace-m48-node-measurements.md`, is written by Task 0.
- **The controller's scratch probe** is `/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/4b336710-6279-49f2-be58-212ed366d476/scratchpad/m48-probe/`, cited as `walls.md §N` and `probe.patch`. Task 0 Step 1 copies it into the ledger and the evidence directory, so nothing later depends on the scratchpad surviving.

## Global Constraints

- **Toolchain.** `1.95.0`, target `aarch64-apple-darwin`.
- **The gate.** `cargo test` in chunks, every chunk `--no-fail-fast` and `--test-threads=1`, plus `cargo clippy --workspace --all-targets -- -D warnings`. The chunking is Task 11's `gate.sh`.
- **`clippy -D warnings` rejects dead code and unused imports, in test files too (`--all-targets`).** Each task adds only what its own code uses. A step marked *conditional* is applied only when its condition holds; when it does not, apply none of its code.
- **Banned calls.** `clippy.toml` bans `Instant::now`, `SystemTime::now` and `std::thread::Thread`. Task 0's scratch build is never committed and is the one place a wall clock may be read.
- **One VM per process.** Every `cargo test` runs with `--test-threads=1`.
- **The trace format does not move (H3).** `TRACE_MAGIC` stays `RT\x00\x0b`, and `crates/retrace-trace` has no diff.
- **Symmetry rule 1.** Every record arm and its replay mirror call the same `Box_` method with the same arguments, and both sit before the generic forward. The new hook `Box_::note_fd_effects` is called from the generic arm and the console-close arm on record (Task 4 Ruling K6: replay finishes the console-close landmark through the generic mirror) and from the generic mirror on replay, with the same arguments.
- **Symmetry rule 2.** None of the following is recorded (R3). Each lives in `run()`, `step()`, `schedule_after_block`, `switch_to_thread`, or in box methods both arms call:
  - the SPRR register;
  - the JIT view;
  - the deadline wakes;
  - the backing split.
- **The thread oracle's count stays at seven.** Every new mirror (`kevent`, the psynch calls) lives inside the existing `Event::Syscall` landmark chain in `ReplaySession::advance`, after its arm-top `verify_thread` call, beside the M45 `kevent_qos` mirror. This corrects spec §3j (§11a item 1). Task 10's controls prove the arm-top call covers each new mirror.
- **Delivery happens at the wake.** A thread woken from `kevent` or `psynch_cvwait` gets its return word, its carry flag and any event-list bytes when it is woken. Its blocking landmark already returned `0` through `set_x0_err_and_return`, so its saved context is a post-return context. The wake then:
  - writes the event-list bytes;
  - sets `x0` and the C bit of the thread's CPSR, in its saved context or on the live vCPU when it is the current thread.

  This is the probe's `probe_finish`, measured across four walks, and M46's `enter_manager` posture. It corrects spec §3c (§11a item 6).
- **A deadline already past at the call is not special-cased.** A relative timeout that converts to 0 ticks blocks with `deadline == now`. `wake_due_threads` then wakes it in the same `schedule_after_block`, which is a schedule point: the lowest-indexed runnable thread runs next. The two node shapes are `kevent` `{0, 1 ns}` and `cvwait` `sec 0, nsec 1`; 1 ns is 0 ticks at 24 MHz. These are the probe's measured semantics (§11b item 8).
- **Node gates run on the debug build, detached.** Every gate spawns `CARGO_BIN_EXE_retrace`, the debug build.
  - **Timings on it:** node records in about 45 s and replays in about 46 s (walls.md build note), so a node test target runs for minutes. `node_e2e` takes about 6 min and `node_crash_e2e` about 8 min; Task 8 measures both.
  - **Running a node target:** use `nohup … > log 2>&1 &` and poll the log, because a foreground tool call stops at 10 minutes.
  - **Traces:** node tests delete their trace files after their last assertion. A node trace is 526 MB and the crash demo's is 1.2 GB. A failing test keeps its trace for diagnosis.
- **Spawn the CLI through `util`.** Every test that spawns the CLI uses `crates/retrace/tests/util/mod.rs`'s helpers, which call `util::bin()`, the codesigned copy.
- **Skips announce themselves.** Every node test and the addon build check skip through `util::announce` with a line starting `SKIPPED`, naming the missing path and saying the gate did not run. The `retrace-guest` unit test cannot reach `util`, so it uses a local `announce` with the same body (CLAUDE.md allows this).
- **Existing assertions stay.** The named exceptions:
  - `sctlr_dze_tests::sctlr_enables_dc_zva_for_el0_and_nothing_else` is rewritten and renamed `sctlr_enables_dc_zva_and_ic_ivau_for_el0_and_nothing_else` in Task 6 (Ruling T6-i; its count stays one);
  - `node_e2e`'s `#[ignore]` is deleted in Task 7.
- **Refusal texts.** Tests match on these prefixes. Every refusal names the value it refuses.
  - `M48: kevent ` (the model; e.g. `M48: kevent change 0 `, `M48: kevent on fd `)
  - `M48: pipe ` (the byte-count model)
  - `M48: psynch ` (the port)
  - `M48: SPRR ` (the register)
  - `M48: MAP_JIT ` (the range set)
  - `M48: a signal is pending on thread ` (a wake that would strand one)

  A replay-side refusal wraps the same message as `<call> refused on replay, though the recording accepted it — replay diverged before this landmark: <message>`. A replay-side mismatch starts `<call> rc mismatch: replay `. A refusal below the trace (`M48: SPRR `, `M48: MAP_JIT `) panics with the same text on both sides, the M18 semaphore posture.
- **Values pending t0.** A value written `T0(<section>)` in this plan is not yet measured. Each task that uses one starts with a controller step that writes `task-N-addendum.md` pinning it from the named measurements section. Only values are deferred this way; every design decision is in this plan.
- **Worktree shell rules:**
  - no `VAR=val cmd` prefix; put `export VAR=val` on its own line first;
  - no `git -C` for the repository itself;
  - put `echo "exit=$?"` in the **same** command as the cargo invocation it checks, **before** any pipe;
  - `--no-fail-fast` goes before `--`;
  - never `git stash` anything;
  - macOS has no `timeout(1)`: bound a run with `perl -e 'alarm N; exec @ARGV' <cmd…>` (exit 142 means the alarm fired);
  - scripts that read `PIPESTATUS` run under `bash` explicitly; the session shell is zsh.
- **Controls (deliberate breakages).**
  - Run them only on the **committed** tree, after the task's commit. Restore with `git checkout <commit> -- <file>`, where `<commit>` is that task's commit, and confirm `git status --short` is empty afterwards.
  - Record each control's actual symptom (the failing assertion's text) in the task report.
  - A control that stays green is a finding: report it, never paper over it.
- **Logs.** Each command writes to `$L/t<N>-<what>.log`, where `L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node`. Shell state does not persist between tool calls, so every command that uses `$L` starts with its own `export L=…` line. Scratch traces and binaries go to `/private/tmp/claude-501/m48-*`.
- **Grep gate logs with `grep -a`**, since they carry ANSI and UTF-8. Before any `awk`, sanitize with `LC_ALL=C tr -cd '\11\12\15\40-\176' < log | sed 's/\x1b\[[0-9;]*m//g'`.
- **Evidence commits exclude trace files**, using the directory-anchored pathspec `':(exclude)<dir>/*.bin'` (M44 P1: the bare form stages nothing).
- **Style.**
  - Match the surrounding code's comment density and idiom.
  - Comments cite the spec as `M48 §3x`, rulings as `R<n>`, t0 as `(t0 M3)`, the probe as `walls.md §N`, and kernel sources by file and function (`kern_synch.c:_psynch_cvwait`, `kern_event.c:kevent_internal`).
  - Test names are sentences.
- **Halts.** The spec's H1–H6 (§7). A measurement that contradicts the spec is a ledgered Ruling and a re-scope, never a silent narrowing.
- **The close pushes.** Task 11 merges `--no-ff` into `main` from the main checkout, compares tree hashes, and runs `git push origin main` (the operator's grant for this milestone). Before `git worktree remove`, the ledger is copied into the main checkout's `.superpowers/sdd/`.

## Plan-time facts

These are read-only measurements taken while writing this plan, on the probe host (Apple M4 Pro, macOS 26.5.2, kernel `xnu-12377.121.10`). Task 0 re-takes each one and commits its output as evidence. None of them substitutes for a t0 measurement the spec names.

| # | Fact | Source |
|---|---|---|
| F1 | The guest's libpthread is `539.100.4` (`LC_ID_DYLIB cur-vers`). That is also the newest published `apple-oss-distributions/libpthread` tag (commit `1f4f5265b319`). Task 5 ports from that tag's `kern/kern_synch.c` and `kern/synch_internal.h`. | `xcrun dyld_info -arch arm64e -load_commands /usr/lib/system/libsystem_pthread.dylib \| grep -A4 ID_DYLIB` |
| F2 | `__pthread_cond_wait` takes the psynch path unless the condvar's signature is `_PTHREAD_COND_SIG_ulock`. That happens only when the mutex carries the ulock option, bit `0x4000` of its options word. `__pthread_mutex_global_init` sets that option only for `PTHREAD_MUTEX_USE_ULOCK=1` in the environment or bit 8 of the kernel's registration-data policy word. retrace's emulated `bsdthread_register` writes no registration data back, and the host's `kern.pthread_mutex_default_policy` is 0. So guest mutexes are **firstfit psynch** and guest condvars are **psynch**. | `xcrun dyld_info -arch arm64e -disassemble /usr/lib/system/libsystem_pthread.dylib` (`__pthread_cond_wait`, `__pthread_mutex_global_init`); `sysctl kern.pthread_mutex_default_policy` |
| F3 | When a firstfit mutex has a kernel waiter, `pthread_cond_wait` passes that mutex to `psynch_cvwait` (`_PTHREAD_MTX_OPT_NOTIFY`), and the mutex's unlock calls `psynch_mutexdrop`. So a `cvwait` whose mutex argument is nonzero needs the mutex pair, which is out of scope (Task 5 refuses it). | `src/pthread_cond.c:_pthread_psynch_cond_wait`, `src/pthread_mutex.c:_pthread_mutex_firstfit_unlock_updatebits` at the F1 tag |
| F4 | `sys_icache_invalidate` is `dsb ish`, then `ic ivau` per 64-byte line, then `dsb ish; isb`. One path reads the commpage's CPU family at `+0x80`. It reads no `CTR_EL0` and issues no `dc cvau`. | `xcrun dyld_info -arch arm64e -disassemble /usr/lib/system/libsystem_platform.dylib` |
| F5 | Every psynch and `kevent` stub is `mov x16, #N; svc #0x80; b.lo <ok>`. The raw kernel return is in `x0`, with the carry flag, at stub + 8. | `xcrun dyld_info -arch arm64e -disassemble /usr/lib/system/libsystem_kernel.dylib` |
| F6 | `hw.tbfrequency` is 24 000 000: a nanosecond count converts to ticks as `ns * 3 / 125`, truncating, so 1 ns is 0 ticks. | `sysctl hw.tbfrequency` |
| F7 | `kevent` scans for events only when `nevents > 0` and no change produced an output (`kern_event.c:kevent_internal`, xnu-12377.121.6). A call with `nevents == 0` applies its changes and returns 0 without blocking, whatever its timeout. This is libuv's `uv_async_send` shape. | `bsd/kern/kern_event.c` at that tag |
| F8 | An `EVFILT_USER` event returns the flags the knote was added with, `fflags = kn_sfflags` and `data = kn_sdata`. On delivery, `EV_CLEAR` deactivates the knote, `EV_ONESHOT` drops it, and a knote with neither is re-activated (level). Touch applies the `NOTE_FF*` operation to `kn_sfflags` and sets `kn_sdata`; `NOTE_TRIGGER` activates (`kern_event.c:filt_usertouch`, `:filt_userprocess`, `:knote_process`). Input flags lose `EV_SYSFLAGS` (`0xF000`) at copyin. | the F7 file |
| F9 | A pipe's `EVFILT_READ` is ready when its byte count is at least 1, with `data` the count. `EVFILT_WRITE` is ready when `MAX(PIPE_SIZE, buffer size) − count ≥ PIPE_BUF` (512), with `data` that difference. `PIPE_SIZE` is 16384 in the SDK. A closed peer reports `EV_EOF`. Each end has its own buffer, so `EVFILT_READ` on a write end is ready only at `EV_EOF` (`sys_pipe.c:filt_piperead_common`, `:filt_pipewrite_common`). | `bsd/kern/sys_pipe.c` at the F7 tag |
| F10 | libpthread hands a relative timespec to the kernel unchanged unless it is `{0, 0}`, which returns `ETIMEDOUT` in user space. An absolute deadline already past also returns there (`pthread_cond.c:_pthread_cond_wait`). So `{0, 1}` reaches the kernel. | `src/pthread_cond.c` at the F1 tag |
| F11 | libpthread reads a failed `cvwait`'s errno as `err & 0xff` and passes the whole value to `_pthread_cond_updateval`, which tests `ECVCLEARED` (0x100) and `ECVPREPOST` (0x200) in it. So the kernel's timeout errno carries those bits (`kern_synch.c:psynch_cvcontinue`), and a lone waiter's timeout is `ETIMEDOUT \| ECVCLEARED` (0x13c, inferred; t0 M4 pins it). | the F10 file; `kern/kern_synch.c` at the F1 tag |

## Probe facts

These come from the controller's scratch probe (`walls.md`, 2026-10-02), measured on a release build of `probe.patch` at `e6caa65`. Task 0 re-runs them as committed scripts.

| # | Fact | Where |
|---|---|---|
| P1 | **The wall list for `-e 'console.log(1)'`, in order:**<br>1. `kevent` (363), no row;<br>2. `mach_msg2` msgh_id 3419 `semaphore_destroy`;<br>3. a data abort after a *partial* `munmap` (V8's aligned-reservation trim; `guest_munmap` drops the whole backing);<br>4. EC 0x00 at the `msr S3_6_C15_C1_5` in `pthread_jit_write_protect_np`;<br>5. EC 0x18 at `ic ivau` in `sys_icache_invalidate`;<br>6. `setsockopt` (105), no row.<br>Then exit 0, printing `1`. `getsockname` (32) fires only when stdin is a socket; with a null stdin, as `Command::output()` gives a test, it never does. Walks 2–4 (`setTimeout` 10 ms and 2000 ms, an `--allow-natives-syntax` script, the crash demo) met no further wall. One XPC message-queue send is refused by M23's existing `RefuseMqSend`, and node carries on. | walls.md §1, §2 |
| P2 | **The hv-sys SIMD bug.** `hv_vcpu_set_simd_fp_reg` takes `hv_simd_fp_uchar16_t`, a 16-byte vector passed in `v0`. bindgen maps it to `u128`, passed in a GPR pair, so every `set_simd` installs the host's `v0`.<br>**Where it bites:** `load_ctx` (every thread switch), `from_checkpoint`, the `sigreturn` restore and `vcpu_set_q`. `get_simd` takes a pointer and is correct.<br>**Symptom:** a recording of walk 1 replayed clean under some host environments and diverged at the final memory compare under others.<br>**Fix:** inline asm that puts the value in `v0`. | walls.md §4 item 1 |
| P3 | **kevent census.**<br>**Volume:** 10–14 calls per walk on five kqueues (4, 7, 8, 10, 13). kq 10 is the main loop; 7 and 13 are throwaways, closed after one use; 8 is libuv's thread-pool loop on tid 1.<br>**Filters:** only `EVFILT_USER`, plus one `EVFILT_READ` `EV_ADD\|EV_ENABLE` on fd 1, the inherited stdout pipe. The latter comes from `uv__stream_try_select`, with timeout `{0, 1 ns}` on kq 13, and native answers it with 0 events.<br>**Flags:** 0x21 and 0x5, fflags `NOTE_TRIGGER`.<br>**Shapes:** nevents 0, 1 or 1024. Timeouts zero, NULL, `{0, 1 ns}`, and `{1, 986000000}` in the 2 s walk.<br>**Outcomes:** every multi-event return held one event. tid 1 blocks in `kevent(kq 8, 1024, NULL)` until main's `NOTE_TRIGGER` at shutdown.<br>**Not on node's path:** no fd filter on a guest pipe, no `EV_DELETE`, `EV_ONESHOT`, `EV_RECEIPT` or `EV_DISABLE`. | walls.md §3 kevent |
| P4 | **psynch census.**<br>**Calls:** 26–29 per walk: `cvwait`, `cvsignal`, and `cvbroad` once at shutdown.<br>**Shape:** every `cvwait` has flags 0xa0 and mutex 0. Every timed one (6 per walk) is `sec 0, nsec 1`.<br>**Never seen:** `mutexwait`, `mutexdrop`, `cvclrprepost`, any rwlock call, a targeted signal, or a prepost.<br>**Return words:** the probe's stub returned 0 to a woken waiter and 0x100 per woken waiter to a signaller. These are a stub's values, not measurements; t0 M4 pins the kernel's. | walls.md §3 psynch |
| P5 | **JIT census.**<br>**The mapping:** exactly one `MAP_JIT` mmap per walk: 256 MiB, `PROT_NONE`, flags `0x41842`, not FIXED.<br>**What follows:** one `mprotect(+0x40000, 0xffc0000, RWX)` inside it; `madvise` REUSE and REUSABLE over it (M47's model, unchanged); one whole-range `munmap` at exit.<br>**Toggles:** 268 SPRR writes in walks 1–2 (main 16, tid 2 252), 278 in walk 3 and 20 in walk 4. View flips number the writes plus 2. No flip was caused by a thread switch.<br>**Cost:** the whole record takes 4–5 s on release and 45 s on debug, so **R9 does not fire**. | walls.md §3 JIT |
| P6 | **Thread census.**<br>**Count:** 7 threads (6 `bsdthread_create`s).<br>**Start-up:** main `cvwait`s for each of four V8 workers' start and makes one `semaphore_wait_trap`/`semaphore_signal_trap` pair (uv_sem is a mach semaphore, M18's `Sem`).<br>**At exit:** tid 1 is woken by `NOTE_TRIGGER` at shutdown; tid 6 is Runnable at exit, never scheduled. At most 6 are runnable at once; no deadlock. | walls.md §3 threads |
| P7 | **The crash demo** (`crash/` in the probe) meets every assertion of spec §1 part 3.<br>**Native:** the marker `CRASHJS cell=… target=0x4000dead0000 rows=2 opt=101001` (kIsFunction \| kOptimized \| kTurboFanned).<br>**Before the crash:** node's own SIGSEGV handler runs (`SignalDelivery` sig 11, then the re-raise).<br>**The crash:** the terminal `Event::Crash` at the addon's `deref`, with FAR 0x4000dead0000 and ESR 0x92000005 (DFSC 0x05, level-1 translation), on thread 0.<br>**The reverse-continue:** `watch <cell> 8; reverse-continue` lands on a pc inside the `MAP_JIT` range, where the cell holds `2` (the warm-up value) and one `stepi` later holds the target.<br>**The lever:** `%PrepareFunctionForOptimization` plus `%OptimizeFunctionOnNextCall`, compiling synchronously on main. | walls.md §2 |
| P8 | **A 10 ms timer never blocks.** `SYNTH_TSC_STRIDE` carries the clock past it before the loop's first poll, so there is no idle jump. A 2000 ms timer blocks in `kevent(kq 10, {1, 986000000})` and is reached by the idle jump. | walls.md §3 timing |
| P9 | **`flush_guest_tlb` panics inside `step()`.**<br>**How it happens:** a view flip during a single-step runs the EL1 TLBI stub with `MDSCR_EL1.SS` armed, which gives `tlbi stub faulted at EL1: EC=SoftStep`. The `reverse-continue` into JIT code crosses toggles, so the demo hits it.<br>**The probe's fix:** clear `SS` and `MDE` around the stub. | walls.md §1, §4 item 2 |
| P10 | **Sizes.** retrace backs a `PROT_NONE` mmap in full. V8's reservations (256 MiB, 256 MiB, 128 MiB, 0x80000) are therefore in every snapshot and checkpoint. That gives a 526 MB trace for `console.log(1)`, 1.2 GB for the crash demo, and 4.2 GB peak RSS for one `reverse-continue` session. | walls.md §2, §3 |

## Review Focus

These are the five inputs or failure modes the spec implies but no fixture is sure to reach, most likely first. Each is pinned by a test in the task that owns the code.

1. **An event list that aliases the change list.** libuv's `uv__kqueue_runtime_detection` passes the same address for both. The model must read every change before it writes any event, as the kernel's copyin loop does. Native leaves the second change slot untouched.
   - Pinned in Task 4: `an_event_list_aliasing_the_change_list_is_read_before_it_is_written` (`crates/retrace-box/tests/gkq.rs`: the pure `gkq.rs` module never sees guest memory), and `kq_e2e`'s `the_runtime_detection_probe_returns_natives_one_event`.
2. **A deadline wake whose thread is the one on the vCPU.** This is node's *common* case: every 1 ns wait blocks and is woken in the same `schedule_after_block` while it is still the current thread. When `pick_next` returns that thread again, `switch_to_thread` returns early, so a delivery written only to the saved context is lost.
   - Pinned in Task 4: `a_wake_of_the_current_thread_writes_the_vcpu` (`crates/retrace-box/tests/gkq.rs`) and `kq_e2e`'s `a_timeout_on_the_only_thread_answers_on_the_vcpu`.
   - Pinned in Task 5: `condvar_e2e`'s `a_timed_wait_on_the_only_waiter_answers_on_the_vcpu`.
3. **psynch sequence words across the 2³² wrap.** `is_seqlower`, `is_seqhigher` and `diff_genseq` work in a half-window, so L near `0xffff_ff00` with S near `0x100` is legal.
   - Pinned in Task 5: `the_sequence_window_wraps_as_synch_internal_h_computes` and `a_signal_across_the_sequence_wrap_wakes_the_waiter` (`psynch.rs` unit tests).
4. **An `unprotect` inside a `MAP_JIT` range.** V8 maps its code range `PROT_NONE` and then `mprotect`s a sub-range RWX. `guest_mprotect` routes that through `unprotect`, which stamps `ATTR_DATA` unconditionally. Unless the JIT view restamps afterwards, the code range is writable and non-executable under the protected view, and the first call into it faults.
   - Pinned in Task 6: `the_stamped_extents_are_the_ranges_minus_their_noaccess_extents` (`jit.rs` unit), `an_unprotect_inside_a_jit_range_is_restamped_by_the_view_not_left_data` (`crates/retrace-box/tests/jit.rs`), and `jitwp_e2e`'s `the_v8_shape_none_mapped_then_mprotected_rwx_runs_its_code`.
5. **A shape the recording accepted but replay refuses.** Replay reaches it only after an earlier silent divergence, so it must be a `Divergence` naming the call and the field, never a panic.
   - Pinned in Task 4: `kq_e2e`'s `a_kevent_refused_on_replay_is_a_divergence_naming_it_not_a_panic`.
   - Pinned in Task 5: `condvar_e2e`'s `a_cvwait_refused_on_replay_is_a_divergence_naming_it_not_a_panic`.

---

## File Structure

| File | Change | Task |
|---|---|---|
| `docs/superpowers/specs/2026-10-02-retrace-m48-node-measurements.md` | create: t0's M1–M9 and the Decisions list | 0 |
| `docs/sweep-evidence/2026-10-02-m48-t0/` | create: t0 evidence (the probe's copied artefacts, the re-walk logs and censuses, native outputs, README) | 0 |
| `crates/hv-sys/src/lib.rs` | `Vcpu::set_simd` through inline asm | 1 |
| `crates/hv-sys/tests/simd.rs` | +1 test | 1 |
| `crates/retrace-box/tests/simdctx.rs` | create: 3 tests | 1 |
| `crates/retrace-guest/c/simd_dyn.c` | create: the SIMD fixture | 1 |
| `crates/retrace/tests/simd_e2e.rs` | create: 3 tests | 1 |
| `crates/retrace/tests/util/mod.rs` | `replay_env` (Task 1); `record_dynamic_args_env`, `assert_rung_records_and_replays_env`, `map_jit_ranges` (Task 7) | 1, 7 |
| `crates/retrace-arch/src/lib.rs` | the M48 section: `SYS_KEVENT`, the `SYS_PSYNCH_*` numbers, `is_psynch`, the kevent constants, `Kevent` (the 32-byte `struct kevent`), the psynch constants, rows 363, 303–305 and 105, and `ETIMEDOUT`/`EINTR` (Task 2); `MAP_JIT`, `SprrAccess`, `decode_sprr_access` (Task 6) | 2, 6 |
| `crates/retrace-arch/tests/census.rs` | 105, 303, 304, 305 and 363; the M48 doc paragraph | 2, 10 (conditional) |
| `crates/retrace-arch/tests/legacy_equivalence.rs` | the 363 and two 105 entries | 2 |
| `crates/retrace-arch/tests/nodeshapes.rs` | create: 6 tests (4 in Task 2, 2 in Task 6) | 2, 6 |
| `crates/retrace-core/src/machmsg.rs` | 3419 in `FORWARD_ALLOWLIST`; +1 unit test | 2 |
| `crates/retrace-box/src/lib.rs` | as listed below; Task 7 adds one `RETRACE_SPRR`-gated `[M48 SPRR]` line in `sprr_write` | 1, 3–7 |
| `crates/retrace-box/src/backings.rs` | `Backings::insert` and `SpanIndex::insert_at`, for the split | 3 |
| `crates/retrace-box/src/thread.rs` | `BlockReason::{Kevent, Cv}` and `deadline()`; `ThreadTable::{wake, due_waiters, earliest_deadline}`; `Thread.sprr` and its accessors; 3 unit tests | 4–6 |
| `crates/retrace-box/src/gkq.rs` | create: `GuestKqueues`, `Kqueue`, `Knote`, `FdKind`, `Pipes`; 11 unit tests | 4 |
| `crates/retrace-box/src/psynch.rs` | create: `Psynch`, `Kwq`, `Kwe`, the ported paths; 11 unit tests | 5 |
| `crates/retrace-box/src/jit.rs` | create: `JitSet`, `View`, `admit_mmap`, `admit_mprotect`; 8 unit tests | 6 |
| `crates/retrace-box/tests/trim.rs` | create: 6 tests | 3 |
| `crates/retrace-box/tests/gkq.rs` | create: 10 tests | 4 |
| `crates/retrace-box/tests/psynch.rs` | create: 4 tests | 5 |
| `crates/retrace-box/tests/jit.rs` | create: 5 tests | 6 |
| `crates/retrace-box/tests/checkpointparity.rs` | +1: the M48 state survives a checkpoint | 6 |
| `crates/retrace-core/src/lib.rs` | the generic-arm asserts (Task 2); the `note_fd_effects` calls (generic arm, console-close arm, generic mirror) and the `kevent` arm and mirror (Task 4); the psynch arm and mirror, and `ReplaySession::dbg_psynch_mut` (Task 5); the `MAP_JIT` exemption from the anon-exec warning (Task 6) | 2, 4–6 |
| `crates/retrace-guest/c/trim_dyn.c`, `kq_dyn.c`, `condvar_dyn.c`, `jitwp_dyn.c` | create: the fixtures | 3–6 |
| `crates/retrace-guest/asm/sprrprobe.s` | create: the static SPRR/cache-maintenance probe | 6 |
| `crates/retrace-guest/node/crash_addon.c`, `crash.js`, `crash.json` | create: the crash demo | 8 |
| `crates/retrace-guest/build.rs`, `src/lib.rs` | build them; `SIMD_DYN`, `TRIM_DYN`, `KQ_DYN`, `CONDVAR_DYN`, `JITWP_DYN`, `SPRRPROBE`, `NODE_CRASH_ADDON`, `CRASH_JS`, `CRASH_JS_JSON`; 7 parse/wiring tests | 1, 3–6, 8 |
| `crates/retrace/tests/trim_e2e.rs` | create: 3 tests | 3 |
| `crates/retrace/tests/kq_e2e.rs` | create: 12 tests | 4 |
| `crates/retrace/tests/condvar_e2e.rs` | create: 8 tests | 5 |
| `crates/retrace/tests/jitwp_e2e.rs` | create: 8 tests | 6 |
| `crates/retrace/tests/node_e2e.rs` | un-ignore, add the JIT assertion, add `node_timer_replays` | 7 |
| `crates/retrace/tests/node_crash_e2e.rs` | create: 1 test | 8 |
| `crates/retrace/tests/thread_oracle.rs` | 2 tests: a retagged `kevent` and a retagged `psynch_cvwait` landmark | 10 |
| `docs/sweep-evidence/2026-10-02-m48/` | create: the walk and the sweep (Task 9); `t10-audit.sh` and `t10-audit.txt` (Task 10) | 9, 10 |
| `tools/bench.py` | a node workload | 9 |
| `crates/retrace/tests/apple_walls_e2e.rs` | an `#[ignore]` reason rewrite only, if the sweep moves a parked row's wall; no new `#[ignore]` (H2) | 9 (conditional) |
| `docs/status-log.md`, `docs/current-state.md`, `README.md`, `CLAUDE.md` | docs | 11 |
| `.superpowers/sdd/2026-10-02-retrace-m48-node/{predict,gate,tally}.sh` | the close | 11 |

The changes to `crates/retrace-box/src/lib.rs`, by task:
- **Task 1:** none. Its call sites are fixed by the hv-sys change.
- **Task 3:** `guest_munmap`'s rounding and `unmap_range`, plus the `Backing` and `free_pages` contracts.
- **Task 4:** the `gkq` field through every path; the deadline queue in `schedule_after_block`; `guest_kevent`, `note_fd_effects` and `deliver_wake`.
- **Task 5:** the `psynch` field through every path; `guest_psynch`, `cv_timed_out`, the `Cv` arm in `wake_due_threads`, `dbg_psynch` and `dbg_psynch_mut`.
- **Task 6:**
  - the `jit` field through every path;
  - the SPRR arm in `try_emulate_undef_mrs`, plus `sprr_write`, `sync_jit_view` and `restamp_jit`;
  - the `MAP_JIT` hooks in `map_mmap_region` (`guest_mmap`'s body), `guest_munmap`, `guest_mprotect`, `place_fixed` and `guest_vm_remap`;
  - `sprr_admitted`, `unmap_jit`, `assert_jit_stamped`, `switch_to_thread`'s view sync, `ipa_is_el0_writable`, `dbg_jit`, and ` jit={:?}` in `dbg_internal_state`;
  - `flush_guest_tlb`'s `MDSCR` guard;
  - SCTLR UCI and the rewritten unit test.

Each evidence directory is named for the day it is written. If a task runs on another day, use that day's date and carry the name forward.

**Test-count prediction (made here, reconciled at the close):**

| Task | Tests added |
|---|---|
| 1 | `hv-sys/tests/simd.rs` +1; `retrace-box/tests/simdctx.rs` 3 (**new binary**); `retrace-guest` unit `simd_dyn_guest_parses` 1; `simd_e2e.rs` 3 (**new binary**) |
| 2 | `nodeshapes.rs` 4 (**new binary**); `machmsg.rs` unit 1 |
| 3 | `retrace-box/tests/trim.rs` 6 (**new binary**); `retrace-guest` unit 1; `trim_e2e.rs` 3 (**new binary**) |
| 4 | `thread.rs` unit 2; `gkq.rs` unit 11; `retrace-box/tests/gkq.rs` 10 (**new binary**); `retrace-guest` unit 1; `kq_e2e.rs` 12 (**new binary**) |
| 5 | `psynch.rs` unit 11; `retrace-box/tests/psynch.rs` 4 (**new binary**); `retrace-guest` unit 1; `condvar_e2e.rs` 8 (**new binary**) |
| 6 | `nodeshapes.rs` +2; `jit.rs` unit 8; `thread.rs` unit 1; `retrace-box/tests/jit.rs` 5 (**new binary**); `checkpointparity.rs` +1; `retrace-guest` unit 2; `jitwp_e2e.rs` 8 (**new binary**) |
| 7 | `node_e2e.rs` +1 (`node_timer_replays`); `node_prints_one_and_replays` moves from ignored to passed |
| 8 | `retrace-guest` unit 1; `node_crash_e2e.rs` 1 (**new binary**) |
| 10 | `thread_oracle.rs` +2 |

The baseline is M47's close: 950 passed / 0 failed / 10 ignored over 160 binaries. That is 958 `#[test]` lines, plus the 2 `census.rs` tests that `legacy_equivalence.rs` compiles a second time. The 160 binaries are:
- the 145 files `crates/*/tests/*.rs`;
- seven library unit targets and the `retrace` bin target;
- seven doc-test targets.

t0 M8 re-derives all three numbers.

**Prediction:** +115 `#[test]` lines (958 → 1073). By task that is 8, 5, 10, 36, 24, 27, 1, 2 and 2. That makes passed + ignored = **1075 over 172** binaries: 160 plus 12 new test files (`simdctx`, `simd_e2e`, `nodeshapes`, `trim`, `trim_e2e`, `gkq`, `kq_e2e`, `psynch`, `condvar_e2e`, `jit`, `jitwp_e2e`, `node_crash_e2e`). The tally is **1066 / 0 / 9**: `node_e2e` is un-ignored and nothing new is ignored (H2). A wall t0's re-walk finds beyond walls.md adds its own tests, and Task 0's addendum names them. Task 11 re-derives the whole figure from source.

---

### Task 0 (t0): Measurements first

**Files:**
- Create: `docs/superpowers/specs/2026-10-02-retrace-m48-node-measurements.md`
- Create: `docs/sweep-evidence/2026-10-02-m48-t0/` (README, the probe's copied artefacts, the kept logs, probes and native outputs)

**Interfaces:**
- Consumes:
  - the controller's scratch probe (walls.md, probe-final.patch, its scripts, logs, native replicas and crash demo);
  - the committed brainstorming evidence in `docs/sweep-evidence/2026-10-02-m48-static/`;
  - M47's `docs/sweep-evidence/2026-09-30-m47/node.*`.
- Produces: the measurements file, which later tasks read by section through a controller addendum:
  - **M1:** the SPRR trap class on the base binary (both directions), `sys_icache_invalidate`'s instructions, and the native idempotence re-run. These confirm Task 6's arm placement (EC 0x00) and its one SCTLR bit (UCI).
  - **M2:** the node **wall list**, re-walked for `-e`, `setTimeout` 10 ms and 2000 ms, the natives script and `crash.js`. Each entry is classed row / small model / new subsystem and compared with walls.md §1. It confirms the task each wall belongs to, and names any new one.
  - **M3:** the `kevent` census and the native replica's answers: libuv's two shapes, the pipe's readiness and capacity, and a closed reader's `EV_EOF`. These fix Task 4's constants.
  - **M4:** the psynch census and the kernel's return words for node's shapes, the `{0, 1 ns}` origin, and the condvar's sequence-word offset. These fix Task 5's `T0(M4)` values.
  - **M5:** the JIT census, the commpage's two SPRR values, and the toggle cost on both builds. These fix Task 6 and confirm R9.
  - **M6:** the thread census (creates, block reasons, max runnable, any deadlock).
  - **M7:** the crash demo (addon build and load, the native marker, the store's pc, the fault, the debug session's time and peak RSS).
  - **M8:** the base `#[test]` count, test-file count and binary count.
  - **M9:** the SIMD fixture on the base binary and on the walk binary.

**Everything experimental in this task is throwaway.**
- The only committed files are the measurements file and the evidence directory.
- The probe's patch is applied only in a scratch worktree of this task's own, which Step 6 creates and removes. The m48 worktree's `crates/` is never patched.
- No scratch file is ever created under the m48 worktree's `crates/`.
- Run nothing else that builds while this task runs: a concurrent cargo build makes the walk's timings meaningless and the gate's runs flaky.
- **The probe's own worktree, `m48-probe/wt`, is gone:** the controller removed it. Its full patch survives as `m48-probe/probe-final.patch`, and Step 1 copies it.

- [ ] **Step 1: Carry the probe in.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
export PR=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/4b336710-6279-49f2-be58-212ed366d476/scratchpad/m48-probe
mkdir -p $L/t0/probe /private/tmp/claude-501
cp $PR/walls.md $PR/probe-final.patch $PR/run.sh $PR/census.sh $PR/savepatch.sh $L/t0/probe/
cp -R $PR/native $PR/logs $PR/crash $L/t0/probe/
# Built binaries are not evidence; Steps 6 and 7 rebuild them from their sources.
rm -f $L/t0/probe/native/kqdetect $L/t0/probe/native/kqpipe $L/t0/probe/crash/crash_addon.node
shasum -a 256 $L/t0/probe/probe-final.patch $L/t0/probe/walls.md
du -sh $L/t0/probe; ls -R $L/t0/probe | head -80
```

`probe-final.patch` is the probe's `git diff` at `e6caa65`, and it includes the untracked `crates/retrace-box/src/probe.rs`, which was added with `git add -N`. It contains the SIMD fix, so the walk binary built from it replays clean. The controller reports that `probe.patch` beside it has the same content; this plan uses `probe-final.patch`.

- [ ] **Step 2: Build the base binaries.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
git rev-parse HEAD
cargo build -p retrace > $L/t0-base-build.log 2>&1; echo "exit=$?"
cp target/aarch64-apple-darwin/debug/retrace /private/tmp/claude-501/m48-base-retrace
codesign -s - -f --entitlements retrace.entitlements /private/tmp/claude-501/m48-base-retrace; echo "sign=$?"
shasum -a 256 /private/tmp/claude-501/m48-base-retrace
```

The base binary is the branch base on the debug profile. Task 9 attributes sweep rows against it.

- [ ] **Step 3: M1(a), the SPRR register and EL0 cache maintenance on the base binary.**

Write `$L/t0/sprrprobe.s`. It is the same text Task 6 Step 9 commits as `crates/retrace-guest/asm/sprrprobe.s`, and it lives in the ledger, never under `crates/`. It is freestanding, with no commpage. On the base binary, the `mrs` reads 0 (today's `try_emulate_undef_mrs`) and the `ic ivau` stops the run. A second variant with the `ic` line deleted reaches the `msr`.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cat > $L/t0/sprrprobe.s <<'ASM'
// M48: the SPRR register and EL0 cache maintenance on a static guest (no commpage).
// The mrs must read 0 (R1), the ic ivau must run at EL0 (SCTLR.UCI), and the msr must be refused
// by value, because a static guest has no commpage to admit a value from. exit(2) means the mrs
// read nonzero.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mrs  x0, S3_6_C15_C1_5
    cbnz x0, 1f
    adr  x3, _start
    ic   ivau, x3
    dsb  ish
    isb
    mov  x1, #1
    msr  S3_6_C15_C1_5, x1
    mov  x0, #0
    b    2f
1:  mov  x0, #2
2:  mov  x16, #1
    svc  #0x80
ASM
sed '/ic  *ivau/d' $L/t0/sprrprobe.s > $L/t0/sprrprobe-msr.s
for v in sprrprobe sprrprobe-msr; do
  clang -arch arm64 -nostdlib -static -Wl,-e,_start -o $L/t0/$v $L/t0/$v.s; echo "$v build=$?"
  perl -e 'alarm 60; exec @ARGV' /private/tmp/claude-501/m48-base-retrace record $L/t0/$v -o /private/tmp/claude-501/m48-$v.bin > $L/t0/m1-$v.log 2>&1; echo "$v exit=$?"
  grep -a -E 'non-syscall exit|EC=|ISS=|elr=' $L/t0/m1-$v.log | head -5
done
```

Record each stop's EC and ISS in §M1(a). These values are expected from walls.md §1 rows 4–5, and a different value is a Ruling before Task 6:
- **`sprrprobe`** stops at `ic ivau` with EC `0x18`. Its ISS should decode to op0 1, op1 3, CRn 7, CRm 5, op2 1, Rt 3.
- **`sprrprobe-msr`** stops at the `msr` with EC `0x00`.

- [ ] **Step 4: M1(b)/(c), `sys_icache_invalidate` and native idempotence.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
nice -n 19 xcrun dyld_info -arch arm64e -disassemble /usr/lib/system/libsystem_platform.dylib 2>&1 \
  | sed -n '/_sys_icache_invalidate:/,/^_sys_dcache_flush:/p' > $L/t0/m1-icache.txt; cat $L/t0/m1-icache.txt
```

List its instructions in §M1(b) and confirm plan F4: no `CTR_EL0` and no `DC CVAU`, so UCI alone is the measured need. UCT stays clear (§11b item 3).

For §M1(c):
1. Copy `docs/sweep-evidence/2026-10-02-m48-static/sprr.c` to `$L/t0/sprr.c`.
2. After its protect call, add one line that calls `pthread_jit_write_protect_np(1)` a second time and reads the register again.
3. Build it, run it natively, and keep the output:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd $L/t0 && cc -O1 -o sprr sprr.c && ./sprr > m1-sprr-native.out 2>&1; echo "rc=$?"; cat m1-sprr-native.out; cd - >/dev/null
```

Confirm all of:
- `+0x10c` = 3;
- `+0x110` = `0x2010002030300000` and `+0x118` = `0x2010002030100000` (walls.md §1 row 4), which differ only in bit 21;
- every thread starts at `+0x118`;
- the second protect call leaves the register unchanged.

Task 6 admits exactly these two values, read from the guest's commpage at run time.

- [ ] **Step 5: M9, the SIMD fixture on the base binary.**

1. Write `$L/t0/simd_dyn.c`, the exact text of Task 1 Step 5's fixture. Copy it from this plan; Task 1 commits the identical file.
2. Build it with the fixture recipe.
3. Run it natively, then record each mode on the base binary and on the walk binary. The walk binary comes in Step 6, so re-run this step's last loop after Step 6.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
clang -arch arm64 -o $L/t0/simd_dyn $L/t0/simd_dyn.c; echo "build=$?"
for m in thread signal; do $L/t0/simd_dyn $m; echo "native $m rc=$?"; done > $L/t0/m9-native.out 2>&1; cat $L/t0/m9-native.out
for m in thread signal; do
  perl -e 'alarm 300; exec @ARGV' /private/tmp/claude-501/m48-base-retrace record-dyn $L/t0/simd_dyn -o /private/tmp/claude-501/m48-simd-$m.bin -- $m > $L/t0/m9-base-$m.out 2> $L/t0/m9-base-$m.err; echo "base $m rc=$?"; cat $L/t0/m9-base-$m.out
done
```

After Step 6 has built the walk binary, run the same loop again with `/private/tmp/claude-501/m48-walk-retrace` in place of the base binary, writing `m9-walk-$m.out` and `m9-walk-$m.err`.

Expected results:
- **Native:** `simd thread intact` and `simd signal intact`, rc 0.
- **The base binary:** a `simd <mode> MISMATCH d<n> got … want …` line and rc 1 for each mode, because the switch back to main and the `sigreturn` each install the host's `v0`.
- **The walk binary (Step 6):** `intact` for both modes.

Record all three in §M9. If the base prints `intact` for either mode, the fixture cannot see the bug through that path. That is a Ruling before Task 1: strengthen the fixture, and do not start Task 1 on a guard that cannot fail.

- [ ] **Step 6: M2, the walk binary.**

Create a scratch worktree of this task's own at the m48 worktree's `HEAD`. Apply the probe's patch there, build the release binary, save it, then remove the scratch worktree:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
export W=/private/tmp/claude-501/m48-t0-walk
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
git worktree add --detach $W HEAD; echo "add=$?"
cd $W
git apply --check $L/t0/probe/probe-final.patch; echo "check=$?"
git apply $L/t0/probe/probe-final.patch; echo "apply=$?"
cargo build --release -p retrace > $L/t0-walk-build.log 2>&1; echo "build=$?"
cp target/aarch64-apple-darwin/release/retrace /private/tmp/claude-501/m48-walk-retrace
codesign -s - -f --entitlements retrace.entitlements /private/tmp/claude-501/m48-walk-retrace; echo "sign=$?"
shasum -a 256 /private/tmp/claude-501/m48-walk-retrace
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
git worktree remove --force $W; echo "remove=$?"
git worktree list
git status --short
```

Two checks close this step:
- `git worktree list` must no longer show `$W`. `--force` is needed because the scratch tree holds the applied patch and its own `target/`, and removing it deletes both.
- `git status --short` in the m48 worktree must show no `crates/` change.

The scratch build compiles from scratch into its own `target/`, which takes a few minutes, and nothing else may build meanwhile.

If `git apply --check` fails, the branch base has moved past `e6caa65` in `crates/`. Then remove the scratch worktree as above, stop, and report it: the probe's measurements were taken at `e6caa65`.

Write the walk script, which records a node command and then replays it twice:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cat > $L/t0/walk.sh <<'SH'
#!/bin/bash
# t0 walk: record node under the walk binary, then replay twice. Usage: walk.sh <tag> -- <node args…>
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
B=/private/tmp/claude-501/m48-walk-retrace
NODE=$(realpath /opt/homebrew/bin/node)
tag=$1; shift 2
T=/private/tmp/claude-501/m48-$tag.bin
export RETRACE_TRACE=1
export RETRACE_PROBE=1
perl -e 'alarm 900; exec @ARGV' $B record-dyn "$NODE" -o $T -- "$@" < /dev/null 2> $L/t0/m2-$tag.err | cat > $L/t0/m2-$tag.out
rc=${PIPESTATUS[0]}
unset RETRACE_TRACE RETRACE_PROBE
echo "$tag record rc=$rc trace=$(stat -f %z $T 2>/dev/null) traps=$(grep -ac '^\[trap\]' $L/t0/m2-$tag.err)" | tee $L/t0/m2-$tag.status
for i in 1 2; do
  perl -e 'alarm 900; exec @ARGV' $B replay $T < /dev/null > $L/t0/m2-$tag.rp$i.out 2> $L/t0/m2-$tag.rp$i.err
  r=$?
  echo "$tag replay$i rc=$r same_stdout=$(cmp -s $L/t0/m2-$tag.out $L/t0/m2-$tag.rp$i.out && echo yes || echo no)" | tee -a $L/t0/m2-$tag.status
done
SH
sed -e "s#^P=.*#P=$L/t0#" -e 's#\$P/logs/\$1\.err#$P/m2-$1.err#' -e 's#\$P/logs/\$1\.status#$P/m2-$1.status#' $L/t0/probe/census.sh > $L/t0/census.sh
grep -n '^P=\|m2-' $L/t0/census.sh
```

Build the crash addon, then run the walks one at a time:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cc -bundle -undefined dynamic_lookup -I /opt/homebrew/include/node -o $L/t0/crash_addon.node $L/t0/probe/crash/crash_addon.c; echo "addon=$?"
bash $L/t0/walk.sh e -- -e 'console.log(1)'
bash $L/t0/walk.sh t10 -- -e 'setTimeout(() => console.log(2), 10)'
bash $L/t0/walk.sh t2000 -- -e 'setTimeout(() => console.log(2), 2000)'
bash $L/t0/walk.sh natives -- --allow-natives-syntax -e 'function f(a,v){a[0]=v} const a=new BigUint64Array(1); %PrepareFunctionForOptimization(f); f(a,1n); f(a,2n); %OptimizeFunctionOnNextCall(f); f(a,3n); console.log(String(a[0]))'
bash $L/t0/walk.sh crash -- --allow-natives-syntax $L/t0/probe/crash/crash.js $L/t0/crash_addon.node $L/t0/probe/crash/crash.json
for t in e t10 t2000 natives crash; do bash $L/t0/census.sh $t > $L/t0/m2-$t.census 2>&1; cat $L/t0/m2-$t.status; done
```

**What each walk must show.** The status lines must read as below, and each census must agree with walls.md §3 (P3–P6):

| Walk | Record rc | Prints | Each replay |
|---|---|---|---|
| `e` | 0 | `1` | rc 0, `same_stdout=yes` |
| `t10` | 0 | `2` | rc 0, `same_stdout=yes` |
| `t2000` | 0 | `2` | rc 0, `same_stdout=yes` |
| `natives` | 0 | `3` | rc 0, `same_stdout=yes` |
| `crash` | 139 | the `CRASHJS` marker, not `UNREACHED` | rc 139, `same_stdout=yes` |

Any difference is a row in §M2. A stop the probe did not meet is a **new wall**:
- **If it is a row or a small model,** the controller writes a Ruling and assigns it to the task that owns its subsystem, as a new step with its own test.
- **If it needs a new subsystem,** it is H5: halt.

For each wall in walls.md §1, write the landmark, the number, the args, the caller, the class, and the task that takes it: rows 3419 and 105 go to Task 2, the partial `munmap` to Task 3, `kevent` to Task 4, the SPRR `msr` and the `ic ivau` to Task 6. Then:
- `getsockname` (32) is noted, not added, with walls.md §1 row 0's reason.
- In the `e` and `crash` censuses, tabulate every partial `munmap`: head, tail or interior, the length, whether the end is unaligned, and whether the range spans more than one backing. These are Task 3's measured shapes.
- Delete the five traces after Step 9 has used `crash`'s.

- [ ] **Step 7: M3, the kevent census and the native replica.**

1. From the `e`, `t2000` and `crash` censuses, tabulate in §M3: every kq fd and its thread; each change (`ident`, `filter`, `flags`, `fflags`); `nchanges`, `nevents` and the timeout form; and each outcome (immediate, blocked then woken, blocked then timed out).
2. Note every fd filter, on which fd kind.
3. Re-run the probe's two replicas (`native/kqdetect.c`, `native/kqpipe.c`).
4. Add the replica below, which takes libuv's two shapes byte for byte plus the answers the pipe model computes, each on a fresh kqueue. Run it with stdout piped, as a test harness runs node:

```c
// $L/t0/kqprobe.c: libuv's two kevent shapes, byte for byte, and the pipe answers the model
// computes (walls.md §1 row 1 and "not walls"; plan F7–F9). Each probe uses a fresh kqueue.
#include <sys/event.h>
#include <sys/types.h>
#include <stdio.h>
#include <unistd.h>

static void show(const char *what, int n, const struct kevent *e) {
    printf("%s n=%d", what, n);
    for (int i = 0; i < n; i++)
        printf(" [ident=%#lx filter=%d flags=%#x fflags=%#x data=%ld udata=%p]",
               (unsigned long)e[i].ident, e[i].filter, e[i].flags, e[i].fflags, (long)e[i].data, e[i].udata);
    printf("\n");
}

static void one(const char *what, int fd, short filter, unsigned short flags, const struct timespec *t) {
    int kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, fd, filter, flags, 0, 0, 0);
    int n = kevent(kq, &ev, 1, &ev, 1, t);
    show(what, n, &ev);
    close(kq);
}

int main(void) {
    struct timespec zero = {0, 0}, onens = {0, 1};
    // uv__kqueue_runtime_detection: add and trigger in one call, the event list aliasing the changes.
    int kq = kqueue();
    struct kevent ch[2];
    EV_SET(&ch[0], 0x1e7e7711, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    EV_SET(&ch[1], 0x1e7e7711, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    int n = kevent(kq, ch, 2, ch, 1, &zero);
    show("detect", n, ch);
    show("detect slot 1 afterwards", 1, &ch[1]);
    close(kq);
    // uv__stream_try_select: EVFILT_READ, EV_ADD|EV_ENABLE, 1 ns, on a pipe's write end and on fd 1.
    int p[2];
    pipe(p);
    one("read on a pipe write end, 1 ns", p[1], EVFILT_READ, EV_ADD | EV_ENABLE, &onens);
    one("read on fd 1, 1 ns", 1, EVFILT_READ, EV_ADD | EV_ENABLE, &onens);
    // The pipe model's readiness answers, its capacity and its EOF.
    one("write-ready on an empty pipe", p[1], EVFILT_WRITE, EV_ADD, &zero);
    write(p[1], "abc", 3);
    one("read-ready after 3 bytes", p[0], EVFILT_READ, EV_ADD, &zero);
    one("write-ready after 3 bytes", p[1], EVFILT_WRITE, EV_ADD, &zero);
    close(p[0]);
    one("read on the write end, reader closed", p[1], EVFILT_READ, EV_ADD | EV_ENABLE, &onens);
    one("write on the write end, reader closed", p[1], EVFILT_WRITE, EV_ADD, &zero);
    return 0;
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd $L/t0
for c in kqprobe probe/native/kqdetect probe/native/kqpipe; do cc -O1 -o $(basename $c) $c.c; echo "$c build=$?"; done
./kqprobe | cat > m3-kqprobe.out; ./kqdetect | cat > m3-kqdetect.out 2>&1; ./kqpipe 2>&1 | cat > m3-kqpipe.out
cat m3-kqprobe.out m3-kqdetect.out m3-kqpipe.out
diff m3-kqdetect.out probe/native/kqdetect.out; diff m3-kqpipe.out probe/native/kqpipe.out
cd - >/dev/null
```

Record in §M3 the values below. They are Task 4's `T0(M3)` constants, and a different answer is a Ruling before Task 4:

| Probe | Expected |
|---|---|
| `detect` | `n=1`, `ident=0x1e7e7711`, `filter=-10`, `flags=0x21`, `fflags=0`, `data=0` |
| `detect slot 1 afterwards` | untouched (`flags=0`, `fflags=0x1000000`) |
| `read on a pipe write end, 1 ns` | `n=0` |
| `read on fd 1, 1 ns` | `n=0` |
| `write-ready on an empty pipe` | `n=1`, `data` the capacity (expected 16384) |
| `read-ready after 3 bytes` | `n=1`, `data=3`, and the returned `flags` (expected `0x1`) |
| `write-ready after 3 bytes` | `data` (expected 16381) |
| `read on the write end, reader closed` | expected `n=1`, flags with `EV_EOF` (`0x8000`) |
| `write on the write end, reader closed` | expected `n=1`, flags with `EV_EOF` (`0x8000`) |

- [ ] **Step 8: M4, the psynch census and the kernel's return words.**

From the walk censuses, tabulate every psynch call in §M4: number, args, flags, thread, and what the caller does with the return. Confirm P4:
- only 303, 304 and 305 occur;
- every `cvwait` has flags 0xa0 and mutex 0;
- every timed one is `sec 0, nsec 1`;
- no thread port is nonzero.

Pin the kernel version: libpthread `539.100.4` (plan F1), `kern/kern_synch.c` and `kern/synch_internal.h` at that tag, with the commit hash and each file's sha256.

Write the native probe below. Each mode uses a fresh pair of threads and signals **outside** the mutex, so no mutex reaches the kernel, matching node's mutex-0 shape:

```c
// $L/t0/cvprobe.c: the psynch shapes node issues (walls.md §3), natively, so lldb can read the
// kernel's return words at each stub + 8. Modes: waitsignal, broad3, timeout, onens, layout.
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t c = PTHREAD_COND_INITIALIZER;
static int ready, go;

static void *waiter(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    ready++;
    while (!go) pthread_cond_wait(&c, &m);
    pthread_mutex_unlock(&m);
    return NULL;
}

static void wait_ready(int n) {
    for (;;) {
        pthread_mutex_lock(&m); int r = ready; pthread_mutex_unlock(&m);
        if (r >= n) break;
        usleep(1000);
    }
    usleep(20000); // let the last waiter reach the kernel
}

static void dump(const char *what) {
    const unsigned char *b = (const unsigned char *)&c;
    printf("%s:", what);
    for (size_t i = 0; i < sizeof c; i++) printf("%s%02x", i % 4 ? "" : " ", b[i]);
    printf("\n");
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    pthread_t t[3];
    if (!strcmp(mode, "waitsignal")) {
        pthread_create(&t[0], NULL, waiter, NULL); wait_ready(1);
        pthread_mutex_lock(&m); go = 1; pthread_mutex_unlock(&m);
        pthread_cond_signal(&c);
        pthread_join(t[0], NULL);
    } else if (!strcmp(mode, "broad3")) {
        for (int i = 0; i < 3; i++) pthread_create(&t[i], NULL, waiter, NULL);
        wait_ready(3);
        pthread_mutex_lock(&m); go = 1; pthread_mutex_unlock(&m);
        pthread_cond_broadcast(&c);
        for (int i = 0; i < 3; i++) pthread_join(t[i], NULL);
    } else if (!strcmp(mode, "timeout") || !strcmp(mode, "onens")) {
        struct timespec rel = { 0, !strcmp(mode, "onens") ? 1 : 50 * 1000 * 1000 };
        pthread_mutex_lock(&m);
        int rc = pthread_cond_timedwait_relative_np(&c, &m, &rel);
        pthread_mutex_unlock(&m);
        printf("%s rc=%d\n", mode, rc);
        dump("after");
    } else if (!strcmp(mode, "layout")) {
        dump("fresh");
        pthread_create(&t[0], NULL, waiter, NULL); wait_ready(1);
        dump("one waiter");
        pthread_mutex_lock(&m); go = 1; pthread_mutex_unlock(&m);
        pthread_cond_signal(&c);
        pthread_join(t[0], NULL);
        dump("after the signal");
    } else { fprintf(stderr, "mode?\n"); return 2; }
    printf("%s done\n", mode);
    return 0;
}
```

Drive it under lldb. The C names of the stubs are `__psynch_*`; their Mach-O symbols carry one more underscore, which is what `dyld_info` prints.
1. Verify the stub layout, so the `+8` offset is the instruction after the `svc`.
2. Confirm each breakpoint resolved: a breakpoint with no location records nothing, silently.
3. Then run each mode:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd $L/t0
nice -n 19 xcrun dyld_info -arch arm64e -disassemble /usr/lib/system/libsystem_kernel.dylib 2>&1 \
  | grep -a -A3 -E '^___psynch_(cvwait|cvsignal|cvbroad|mutexwait|mutexdrop):' > m4-stubs.txt; cat m4-stubs.txt
cc -O1 -o cvprobe cvprobe.c
cat > m4.lldb <<'LLDB'
break set -n __psynch_cvwait -C "register read x0 x1 x2 x3 x4 x5 x6 x7" -G true
break set -n __psynch_cvwait -R 8 -C "register read x0 cpsr" -G true
break set -n __psynch_cvsignal -C "register read x0 x1 x2 x3 x4 x5 x6 x7" -G true
break set -n __psynch_cvsignal -R 8 -C "register read x0 cpsr" -G true
break set -n __psynch_cvbroad -C "register read x0 x1 x2 x3 x4 x5 x6" -G true
break set -n __psynch_cvbroad -R 8 -C "register read x0 cpsr" -G true
break set -n __psynch_mutexwait -C "register read x0" -G true
break set -n __psynch_mutexdrop -C "register read x0" -G true
break list
LLDB
for m in waitsignal broad3 timeout onens layout; do perl -e 'alarm 120; exec @ARGV' xcrun lldb -b -s m4.lldb -o "run $m" -o "quit" ./cvprobe > m4-$m.out 2>&1; echo "$m rc=$?"; done
grep -a -c 'locations = 1\|where = libsystem_kernel' m4-waitsignal.out
grep -a -h -E 'x0 = |cpsr = |rc=|after|fresh|waiter|signal:' m4-*.out | head -120
cd - >/dev/null
```

Then measure where node's `{0, 1 ns}` waits come from, natively:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd $L/t0
perl -e 'alarm 300; exec @ARGV' xcrun lldb -b -o "break set -n __psynch_cvwait -c '\$x6 == 0 && \$x7 == 1' -C 'bt 8' -G true" -o "run -e 'console.log(1)'" -o quit -- $(realpath /opt/homebrew/bin/node) > m4-node-onens.out 2>&1; echo "rc=$?"
grep -a -c 'frame #0' m4-node-onens.out; grep -a 'frame #[1-5]' m4-node-onens.out | sort | uniq -c | sort -rn | head -12
cd - >/dev/null
```

Record in §M4 the values below. They are Task 5's `T0(M4)` values, each to be cross-checked against the ported arithmetic (`PTHRW_INC`, `CBIT`/`PBIT`/`MBIT`, `ECVCLEARED 0x100`, `ECVPREPOST 0x200`):
- **The word and carry each call returns:**
  - a woken waiter's `cvwait`;
  - the signaller's `cvsignal` with one waiter (expected `0x101` if the signal balances L and S, since `ksyn_cvupdate_fixup` sets `CBIT`; the probe's stub returned `0x100`);
  - the `cvbroad` to three;
  - the `timeout` and `onens` errnos (expected `0x13c`, `ETIMEDOUT | ECVCLEARED`, with carry; plan F11).
- **The `cvwait` flags word**, natively (expected 0xa0).
- **The offset of the three `c_seq` words** inside `pthread_cond_t`: the `layout` dumps show which bytes move by `0x100`, expected bytes 24–35 of the 48. Task 5's fixture prints those 12 bytes and compares them with native.
- **Whether native node issues `{0, 1 ns}` waits at all,** and from which frames. If it does, they are V8's or libuv's own shape. If it does not, they are an artefact of the synthetic clock. Either way the design is unchanged (Global Constraints, the passed-deadline rule); the answer goes in the measurements file and the status log.
- **Whether any `mutexwait`/`mutexdrop` fired** in any mode (expected none).

- [ ] **Step 9: M5–M7, the JIT, thread and crash-demo measurements.**

From the `e`, `natives` and `crash` censuses, tabulate in §M5:
- every `MAP_JIT` mmap (`addr`, `len`, `prot`, `flags`, FIXED or not);
- every `mprotect`/`munmap`/`madvise` over one;
- the SPRR writes per thread, and the values written (must be the two from Step 4);
- the view flips, and how many a switch caused;
- the record time on the release walk binary. The debug figure is walls.md's build note (45 s record, 46 s replay). The base binary cannot walk node, since it stops at `kevent`, so re-measure on debug only if the release figure differs from walls.md's by more than a factor of 2.

R9 does not fire unless `console.log(1)`'s debug record exceeds 10 minutes.

In §M6, tabulate thread creates, each thread's block reasons over time, the maximum runnable count, and the threads' states at exit (P6).

For §M7:
1. Run the crash demo natively; keep the marker line and the exit code.
2. Discover the cell from the recorded stdout.
3. Run the debug session on the walk binary's crash trace with `/usr/bin/time -l`, keeping its time and peak RSS.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd $L/t0
$(realpath /opt/homebrew/bin/node) --allow-natives-syntax probe/crash/crash.js $PWD/crash_addon.node probe/crash/crash.json > m7-native.out 2>&1; echo "native rc=$?"; cat m7-native.out
CELL=$(grep -a -o 'cell=0x[0-9a-f]*' m2-crash.out | head -1 | cut -d= -f2); echo "cell=$CELL"
/usr/bin/time -l perl -e 'alarm 900; exec @ARGV' /private/tmp/claude-501/m48-walk-retrace debug /private/tmp/claude-501/m48-crash.bin --script "continue; watch $CELL; reverse-continue; x $CELL 8; stepi; x $CELL 8" > m7-debug.out 2> m7-debug.err; echo "debug rc=$?"
cat m7-debug.out; grep -a -E 'real|maximum resident' m7-debug.err
cd - >/dev/null
```

Record in §M7:
- the native marker, including `opt=` (expected `opt=101001`);
- that the hit lands at a pc inside the `MAP_JIT` range;
- the cell's two values (expected `2`, then the target);
- the session's time and peak RSS (P10).

If the hit's pc is outside every `MAP_JIT` range, write a Ruling naming the lever before Task 8 (`--no-concurrent-recompilation` first).

- [ ] **Step 10: M8, the base counts.**

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
ls crates/*/tests/*.rs | wc -l
grep -a -c 'test result:' /Users/noahmitchem/Documents/GitHub/retrace/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite/regate-95bf845/gate-*.log | awk -F: '{s+=$2} END {print s}'
```

Expect 958, 145 and 160. M47's 160 binaries are:
- the 145 test files;
- the seven library unit targets and the `retrace` bin target;
- the seven doc-test targets.

M47 closed at 950 + 10 = 960: the 958 lines plus the two `census.rs` tests compiled twice. A different number is reconciled file by file against M47's `predict.txt`, in the main checkout's `.superpowers/sdd/2026-09-30-retrace-m47-gitwrite/`, before Task 1 starts.

- [ ] **Step 11: Write the measurements file and the evidence; commit.**

Create `docs/sweep-evidence/2026-10-02-m48-t0/` containing:
- `$L/t0/probe/` (the copied probe);
- every `$L/t0/*.log`, `*.out`, `*.err`, `*.status`, `*.census` and `*.txt`;
- the scripts (`walk.sh`, `census.sh`, `m4.lldb`);
- the probe sources (`sprrprobe.s`, `sprrprobe-msr.s`, `simd_dyn.c`, `kqprobe.c`, `cvprobe.c`, `sprr.c`);
- a `README.md` saying, for each file, which command produced it, on which binary (with its sha256) and on which date.

Truncate each `m2-*.err` to its last 400 lines, since each is a long run of `[trap]` lines (the probe's were about 190 KB). Commit no `.bin`, no recorded trace and no built binary.

Write the measurements file with one section per measurement, `## M1` (with (a)–(c)) through `## M9`. Each section gives:
- the command;
- the result, quoted from the evidence;
- its agreement or disagreement with walls.md;
- the decision this plan's rule makes from it;
- any halt considered.

End with a **Decisions** list giving:
- the SPRR `msr` trap class and the two admitted values;
- the SCTLR bit to set (UCI);
- the kevent returned flags, the pipe capacity and the `EV_EOF` answers;
- the psynch return words, the `cvwait` flags word, the `c_seq` offset, and the `{0, 1 ns}` origin;
- the measured partial-`munmap` shapes;
- the `MAP_JIT` layout, the toggle cost, and R9's verdict;
- the crash demo's marker, store location and debug-session cost;
- the SIMD fixture's base and fixed outcomes;
- the base `#[test]`, file and binary counts;
- any new wall, with its Ruling.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
git add docs/superpowers/specs/2026-10-02-retrace-m48-node-measurements.md docs/sweep-evidence/2026-10-02-m48-t0 ':(exclude)docs/sweep-evidence/2026-10-02-m48-t0/*.bin'
git status --short
git commit -m "M48 t0: measurements M1-M9 — the probe re-walked as committed scripts, SPRR and cache maintenance, the kevent/psynch/JIT/thread censuses, the crash demo, the SIMD fixture, the base count"
```

---

### Task 1: The SIMD register ABI (`hv-sys`, box tests, fixture, e2e)

This bug predates M48. walls.md §4 item 1 root-caused it, and the controller ruled it a task, not an H4 halt (§11b item 1). It lands first because every node gate depends on it: node is the first gated guest whose threads keep live SIMD state across a switch.

**Files:**
- Modify: `crates/hv-sys/src/lib.rs` (`Vcpu::set_simd`)
- Modify: `crates/hv-sys/tests/simd.rs` (+1)
- Create: `crates/retrace-box/tests/simdctx.rs`
- Create: `crates/retrace-guest/c/simd_dyn.c`; modify `crates/retrace-guest/build.rs`, `src/lib.rs`
- Modify: `crates/retrace/tests/util/mod.rs` (`replay_env`)
- Create: `crates/retrace/tests/simd_e2e.rs`

**Interfaces:**
- Consumes: t0 M9 (the base binary's `MISMATCH` lines, which are what this task's control must reproduce).
- Produces: a `set_simd` that installs its argument. Its four callers in `retrace-box` need no change: `load_ctx`, `from_checkpoint`, the `sigreturn` restore and `vcpu_set_q`.

- [ ] **Step 1: Controller addendum.** Write `$L/task-1-addendum.md` pinning, from measurements §M9:
  - the base binary's output and exit code for each `simd_dyn` mode;
  - the walk binary's, the fixed one.

  If t0 ruled the fixture too weak, the addendum carries the strengthened text instead of Step 5's. Do not start until it exists.

- [ ] **Step 2: The hv-sys test, red first.** Append to `crates/hv-sys/tests/simd.rs`:

```rust
// M48 Task 1: the value must come from the argument, not from whatever the host left in v0. The
// test above passes on the pre-M48 binding by accident: its constant happens to be in v0 at the
// call. This one poisons v0 before every call, so only a binding that passes the vector itself can
// read back what it set (walls.md §4 item 1).
#[test]
fn set_simd_installs_the_passed_value_not_what_the_host_left_in_v0() {
    let vm = Vm::create().unwrap();
    let vcpu = Vcpu::create(&vm).unwrap();
    for n in 0..32u32 {
        let v: u128 = std::hint::black_box(
            0x5eed_0000_0000_0000_0000_0000_0000_0000 ^ ((n as u128 + 1) * 0x0101_0101_0101_0101_0101_0101_0101_0101));
        // SAFETY: writes only v0, which the block declares clobbered.
        unsafe { core::arch::asm!("movi v0.2d, #0xffffffffffffffff", out("v0") _); }
        vcpu.set_simd(simd::q(n), v).unwrap();
        assert_eq!(vcpu.get_simd(simd::q(n)).unwrap(), v, "Q{n}: set_simd installed something other than its argument");
    }
}
```

No value in that loop equals the all-ones poison: its top byte is `0x5e ^ (n + 1)`.

- [ ] **Step 3: The box-level tests, red first.** Create `crates/retrace-box/tests/simdctx.rs`:

```rust
//! M48 Task 1, box level: every SIMD restore installs the value it was handed. Before the fix each
//! of these read back whatever the host had in v0 (walls.md §4 item 1). Static box: the restores
//! under test touch only the vCPU and the thread table.
use retrace_box::thread::ThreadCtx;
use retrace_box::Box_;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

/// 32 distinct values that no register holds by accident.
fn values(seed: u128) -> [u128; 32] {
    std::array::from_fn(|i| std::hint::black_box(seed ^ ((i as u128 + 1) * 0x0101_0101_0101_0101_0101_0101_0101_0101)))
}

#[test]
fn the_debuggers_register_write_installs_its_value_in_every_q_register() {
    let mut b = tb();
    let v = values(0x5eed_0000_0000_0000_0000_0000_0000_0000);
    for (n, &x) in v.iter().enumerate() { b.vcpu_set_q(n as u32, x); }
    for (n, &x) in v.iter().enumerate() { assert_eq!(b.vcpu_get_q(n as u32), x, "q{n}"); }
}

#[test]
fn a_thread_switch_installs_the_incoming_threads_simd_registers() {
    let mut b = tb();
    let mut ctx = ThreadCtx::zeroed();
    ctx.fp = values(0x7417_0000_0000_0000_0000_0000_0000_0000);
    let want = ctx.fp;
    let t = b.threads_mut().spawn(ctx, (0, 0));
    b.switch_to_thread(t);
    for (n, &x) in want.iter().enumerate() { assert_eq!(b.vcpu_get_q(n as u32), x, "thread {t} q{n} after the switch"); }
    let main_saved = b.threads().ctx_of(0).fp;
    b.switch_to_thread(0);
    for (n, &x) in main_saved.iter().enumerate() { assert_eq!(b.vcpu_get_q(n as u32), x, "main q{n} after the switch back"); }
}

#[test]
fn a_checkpoint_restore_installs_the_captured_simd_registers() {
    let b = tb();
    let mut st = b.checkpoint();
    st.fp = values(0xc4ec_0000_0000_0000_0000_0000_0000_0000);
    let want = st.fp;
    drop(b); // one VM per process: the restored box builds its own
    let r = Box_::from_checkpoint(&st);
    for (n, &x) in want.iter().enumerate() { assert_eq!(r.vcpu_get_q(n as u32), x, "q{n} after from_checkpoint"); }
}
```

`ThreadTable::spawn`'s stack pair is bookkeeping for a thread's own stack, and a thread that never runs may carry `(0, 0)`. Check `spawn`'s body. If it validates the pair, pass the static box's stack window instead (`b.stack_top() - 0x4000, b.stack_top()`).

- [ ] **Step 4: Run red.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p hv-sys --test simd -- --test-threads=1 > $L/t1-red-hv.log 2>&1; echo "exit=$?"; grep -a -E 'test .* (ok|FAILED)|did not|other than' $L/t1-red-hv.log | head
cargo test -p retrace-box --test simdctx -- --test-threads=1 > $L/t1-red-box.log 2>&1; echo "exit=$?"; grep -a -E 'test .* (ok|FAILED)' $L/t1-red-box.log
```

Expect `set_simd_installs_the_passed_value_not_what_the_host_left_in_v0` and all three `simdctx` tests to fail. The existing `fp_and_simd_regs_roundtrip` may pass; that is the accident Step 2's comment names.

- [ ] **Step 5: The fixture and the e2e, red first.**

Create `crates/retrace-guest/c/simd_dyn.c`. This is the text t0 Step 5 ran, or the addendum's replacement for it:

```c
// M48 Task 1: callee-saved SIMD state across a thread switch and across a signal handler's
// sigreturn. AAPCS64 makes d8-d15 callee-saved, so natively they survive any call. A mismatch is
// therefore retrace's: before the fix, every SIMD restore installed the host's v0 (walls.md §4).
// Modes: thread, signal. Prints "simd <mode> intact", or the first mismatch with exit 1.
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

// simd_across(fn, arg, in, out): load d8-d15 from in[0..8], call fn(arg), store d8-d15 to out[0..8].
// It saves and restores the caller's own d8-d15, as the ABI requires of a callee that uses them.
void simd_across(void (*fn)(void *), void *arg, const uint64_t *in, uint64_t *out);
__asm__(
    ".text\n.p2align 2\n.globl _simd_across\n_simd_across:\n"
    "  stp x29, x30, [sp, #-96]!\n"
    "  mov x29, sp\n"
    "  stp x19, x20, [sp, #16]\n"
    "  stp d8, d9, [sp, #32]\n  stp d10, d11, [sp, #48]\n"
    "  stp d12, d13, [sp, #64]\n  stp d14, d15, [sp, #80]\n"
    "  mov x19, x3\n  mov x20, x0\n"
    "  ldp d8, d9, [x2]\n  ldp d10, d11, [x2, #16]\n"
    "  ldp d12, d13, [x2, #32]\n  ldp d14, d15, [x2, #48]\n"
    "  mov x0, x1\n  blr x20\n"
    "  stp d8, d9, [x19]\n  stp d10, d11, [x19, #16]\n"
    "  stp d12, d13, [x19, #32]\n  stp d14, d15, [x19, #48]\n"
    "  ldp d8, d9, [sp, #32]\n  ldp d10, d11, [sp, #48]\n"
    "  ldp d12, d13, [sp, #64]\n  ldp d14, d15, [sp, #80]\n"
    "  ldp x19, x20, [sp, #16]\n"
    "  ldp x29, x30, [sp], #96\n  ret\n");

// The child dirties vector registers of its own, so a switch that leaked its state into main's would show.
static void *child(void *p) {
    __asm__ volatile("movi v8.16b, #0xa5\n movi v9.16b, #0xa5\n movi v16.16b, #0xa5\n movi v31.16b, #0xa5"
                     ::: "v8", "v9", "v16", "v31");
    return p;
}

static void join_a_child(void *unused) {
    (void)unused;
    pthread_t t;
    if (pthread_create(&t, NULL, child, NULL) != 0) { puts("pthread_create failed"); return; }
    pthread_join(t, NULL); // main blocks here, so the child runs, exits and wakes it
}

// Leaves d8-d15 zeroed on the way out. There is deliberately no clobber list, so the compiler
// restores nothing, and only sigreturn's restore of the interrupted context brings main's values back.
static void on_usr1(int sig) {
    (void)sig;
    __asm__ volatile("movi v8.2d, #0\n movi v9.2d, #0\n movi v10.2d, #0\n movi v11.2d, #0\n"
                     "movi v12.2d, #0\n movi v13.2d, #0\n movi v14.2d, #0\n movi v15.2d, #0\n");
}

static void raise_usr1(void *unused) { (void)unused; raise(SIGUSR1); }

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "thread";
    uint64_t in[8], out[8];
    for (int i = 0; i < 8; i++) in[i] = 0x0123456789abcdefULL * (uint64_t)(i + 1) ^ (0x5d00ULL << i);
    memset(out, 0, sizeof out);
    if (!strcmp(mode, "thread")) {
        simd_across(join_a_child, NULL, in, out);
    } else if (!strcmp(mode, "signal")) {
        signal(SIGUSR1, on_usr1);
        simd_across(raise_usr1, NULL, in, out);
    } else {
        printf("unknown mode %s\n", mode);
        return 2;
    }
    for (int i = 0; i < 8; i++) {
        if (out[i] != in[i]) {
            printf("simd %s MISMATCH d%d got %#llx want %#llx\n", mode, i + 8,
                   (unsigned long long)out[i], (unsigned long long)in[i]);
            return 1;
        }
    }
    printf("simd %s intact\n", mode);
    return 0;
}
```

Wire it into `build.rs` with the `madv_dyn` recipe (`clang -arch arm64 -o $OUT/simd_dyn c/simd_dyn.c`). Add `pub const SIMD_DYN: &str = concat!(env!("OUT_DIR"), "/simd_dyn");` to `src/lib.rs`, and a `simd_dyn_guest_parses` unit test in the `madv_guest_parses` pattern.

Add to `crates/retrace/tests/util/mod.rs`, beside `replay`:

```rust
/// M48: `replay` with extra environment set on the REPLAYER (see `run_env`). The SIMD bug made a
/// replay's outcome depend on the host environment (walls.md §4 item 1), so `simd_e2e` replays one
/// trace under several environments and asserts they agree.
pub fn replay_env(trace: &std::path::Path, env: &[(&str, &str)]) -> RunOut {
    run_env(&["replay", trace.to_str().unwrap()], env)
}
```

Create `crates/retrace/tests/simd_e2e.rs`:

```rust
//! M48 Task 1: callee-saved SIMD registers survive a thread switch and a sigreturn, and a threaded
//! replay does not depend on the host environment. Before the fix, every SIMD restore installed the
//! host's v0 (walls.md §4 item 1). `simd_dyn` then printed MISMATCH (t0 M9), and node's walk 1
//! replayed clean under one login environment and diverged under another.
mod util;
use retrace_guest::SIMD_DYN;

fn native(mode: &str) -> Vec<u8> {
    let out = std::process::Command::new(SIMD_DYN).arg(mode).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "native simd_dyn {mode} must pass: {}", String::from_utf8_lossy(&out.stdout));
    out.stdout
}

#[test]
fn a_thread_switch_preserves_the_callee_saved_simd_registers() {
    let want = native("thread");
    assert_eq!(want, b"simd thread intact\n");
    util::assert_rung_records_and_replays(SIMD_DYN, &["thread"], &want);
}

#[test]
fn a_sigreturn_restores_the_interrupted_simd_registers() {
    let want = native("signal");
    assert_eq!(want, b"simd signal intact\n");
    util::assert_rung_records_and_replays(SIMD_DYN, &["signal"], &want);
}

// The property walls.md §4 lost: one recording replays identically whatever the replayer's
// environment. A larger environment shifts the host's stack and so what the host leaves in v0.
#[test]
fn a_threaded_replay_does_not_depend_on_the_host_environment() {
    let rec = util::assert_rung_records_and_replays(SIMD_DYN, &["thread"], b"simd thread intact\n");
    let pad = "x".repeat(4096);
    let envs: [Vec<(&str, &str)>; 3] =
        [vec![], vec![("M48_SIMD_PAD", pad.as_str())], vec![("M48_SIMD_PAD", "1"), ("PWD", "/")]];
    for env in &envs {
        let rep = util::replay_env(&rec.trace, env);
        assert_eq!(rep.code, 0, "replay under {env:?} must exit 0:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay under {env:?} diverged from the recording");
    }
}
```

Run `simd_e2e` red: the first two tests fail with t0 M9's `MISMATCH` lines. The third may pass or fail, because what the host leaves in `v0` is the variable; the first two are the guard that cannot pass by accident.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test simd_e2e -- --test-threads=1 > $L/t1-red-e2e.log 2>&1; echo "exit=$?"; grep -a -E 'test .* (ok|FAILED)|MISMATCH' $L/t1-red-e2e.log | head
```

- [ ] **Step 6: The fix.** Replace `Vcpu::set_simd`'s body in `crates/hv-sys/src/lib.rs`:

```rust
    /// Install `v` in SIMD/FP register `r`.
    ///
    /// M48 Task 1. `hv_vcpu_set_simd_fp_reg` takes `hv_simd_fp_uchar16_t`, a 16-byte
    /// `ext_vector_type`, BY VALUE, and AAPCS64 passes a short vector in `v0`. bindgen maps the
    /// type to `u128`, which Rust passes in a general-register pair, so the framework installed
    /// whatever the host process last left in `v0`. That gave host-environment-dependent guest
    /// state: a node replay that diverged under one login environment and not another (walls.md
    /// §4 item 1). This calls the function by hand, with the value where the ABI puts it. Stable
    /// Rust has no vector FFI (`simd_ffi` is unstable), hence the asm. `get_simd` takes a pointer
    /// and was always right.
    pub fn set_simd(&self, r: SimdReg, v: u128) -> Result<(), HvError> {
        let (lo, hi) = (v as u64, (v >> 64) as u64);
        let f = hv_vcpu_set_simd_fp_reg as *const () as usize;
        let ret: u64;
        // SAFETY: a call to the framework function under its C signature: the vCPU in x0, the
        // register in w1, the vector in v0, the result in w0. `clobber_abi("C")` declares every
        // caller-saved register the callee may use (x0-x17, x30, v0-v7, v16-v31, and the upper
        // halves of v8-v15) clobbered. The block is not `nostack`, so the compiler keeps nothing
        // below sp and sp is call-aligned on entry.
        unsafe {
            core::arch::asm!(
                "fmov d0, {lo}",
                "mov v0.d[1], {hi}",
                "blr {f}",
                lo = in(reg) lo, hi = in(reg) hi, f = in(reg) f,
                inlateout("x0") self.id => ret, in("x1") r.0 as u64,
                clobber_abi("C"),
            );
        }
        check(ret as u32 as i32 as hv_return_t)
    }
```

Then audit the bindings for any other function that takes a vector type by value. List every `pub fn hv_` in `$OUT_DIR/bindings.rs` whose parameters name `u128`, `hv_simd_fp_uchar16_t` or an `_uchar64_t` type without a `*`, and record the list in the task report. At plan time only `hv_vcpu_set_simd_fp_reg` did.

- [ ] **Step 7: Green, regression, clippy, commit.** Every SIMD restore changed, so the regression set is every gate that switches threads, delivers a signal, seeks through a checkpoint or writes a register from a debugger:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 > $L/t1-ws.log 2>&1; echo "ws exit=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t1-box.log 2>&1; echo "box exit=$?"
for t in simd_e2e thread_rust_e2e thread_watch_e2e thread_oracle sigthread_e2e killother_e2e blockedctx dispatch_e2e gcdtimer_e2e kqinit_e2e checkpoint_seek seek segv_rust_e2e panic_e2e sigdeliver_e2e sigcatch_dyn_e2e sigblocked_e2e sigraise_e2e sigign_e2e hitorder_e2e gdbserver_e2e cpython_crash_e2e lldb_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t1-$t.log 2>&1; echo "$t exit=$?"; done
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/t1-bins.log 2>&1; echo "bins exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t1-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git commit -m "M48 t1: hv-sys set_simd passes its vector in v0 — every SIMD restore installed the host's v0 (simdctx, simd_dyn, simd_e2e)"
```

A gate that changes outcome here is a finding to diagnose before proceeding. Its pre-fix pass rested on both sides installing the same garbage.

- [ ] **Step 8: Control (on the committed tree).**
  1. Restore the pre-fix binding: `git show <t0 commit>:crates/hv-sys/src/lib.rs > crates/hv-sys/src/lib.rs`.
  2. Run `cargo test -p hv-sys --test simd`, `-p retrace-box --test simdctx` and `-p retrace --test simd_e2e`.
  3. Confirm `set_simd_installs_the_passed_value_not_what_the_host_left_in_v0`, the three `simdctx` tests and `simd_e2e`'s first two go red, the e2e with the addendum's `MISMATCH` lines.
  4. Restore with `git checkout <t1 commit> -- crates/hv-sys/src/lib.rs`, and record the symptoms.

---

### Task 2: The rows, the forward and the generic-arm asserts (`retrace-arch`, `retrace-core`)

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (the M48 section)
- Modify: `crates/retrace-arch/tests/census.rs`, `crates/retrace-arch/tests/legacy_equivalence.rs`
- Create: `crates/retrace-arch/tests/nodeshapes.rs`
- Modify: `crates/retrace-core/src/machmsg.rs` (3419), `crates/retrace-core/src/lib.rs` (the generic-arm asserts)

**Interfaces:**
- Consumes: t0 M2's wall list and t0 M4's psynch census.
- Produces:
  - `arg_kinds` rows for 363, 303, 304, 305 and 105;
  - `retrace_arch::is_psynch`, the never-forward set;
  - the kevent constants and the `Kevent` struct Task 4 decodes with;
  - the psynch constants Task 5 needs;
  - `semaphore_destroy` forwarded;
  - the generic-arm asserts Tasks 4 and 5 rely on.

- [ ] **Step 1: Controller addendum.** Write `$L/task-2-addendum.md` pinning, from measurements §M2 and §M4:
  - the psynch numbers the walks reached (expected exactly {303, 304, 305}; any other is a Ruling, since Task 5 refuses every other psynch number by value);
  - that 105 and 3419 were reached;
  - that 32 was not, under a null stdin.

  Do not start until it exists.

- [ ] **Step 2: A failing test first.** Create `crates/retrace-arch/tests/nodeshapes.rs`. Copy M47 `gitshapes.rs`'s `sdk_header` and `define` helpers, which re-read the SDK's `sys/syscall.h` at test time so a typed number cannot satisfy the test. Write:
  - `the_kevent_setsockopt_and_psynch_numbers_are_the_sdks`: `SYS_KEVENT`, `SYS_SETSOCKOPT`, `SYS_PSYNCH_CVBROAD`, `SYS_PSYNCH_CVSIGNAL` and `SYS_PSYNCH_CVWAIT` equal `SYS_kevent`, `SYS_setsockopt`, `SYS_psynch_cvbroad`, `SYS_psynch_cvsignal` and `SYS_psynch_cvwait`.
  - `is_psynch_is_exactly_the_sdks_psynch_set`: collect every `#define SYS_psynch_<name> <n>` in the header (14 at plan time: 297–309 and 312). Assert that set equals `(0..1024).filter(|&n| is_psynch(n))`.

  Run it red; the constants do not exist yet:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-arch --test nodeshapes -- --test-threads=1 > $L/t2-red.log 2>&1; echo "exit=$?"; grep -a -E 'error|cannot find' $L/t2-red.log | head
```

- [ ] **Step 3: The constants and the `Kevent` struct.** In `crates/retrace-arch/src/lib.rs`'s M48 section, add the following.
  - **The syscall numbers.** `SYS_KEVENT: u64 = 363` (unless it exists), `SYS_SETSOCKOPT: u64 = 105`, `SYS_PSYNCH_CVBROAD = 303`, `SYS_PSYNCH_CVSIGNAL = 304` and `SYS_PSYNCH_CVWAIT = 305`, each with a doc line citing the SDK.
  - **`pub fn is_psynch(num: u64) -> bool { matches!(num, 297..=309 | 312) }`.** Its doc says this is every `SYS_psynch_*` in the SDK, the set that must never be forwarded: forwarded, a psynch call acts on the HOST's psynch state keyed by retrace's own addresses. Task 5 models 303–305 and refuses the rest by value. The record arm, the replay mirror and the generic-arm assert share it, as `is_fcntl_dupfd` is shared.
  - **The `struct kevent` layout as a decoder,** 32 bytes little-endian (SDK `sys/event.h`): `ident u64 @0`, `filter i16 @8`, `flags u16 @10`, `fflags u32 @12`, `data i64 @16`, `udata u64 @24`. Model it as `pub struct Kevent { ident, filter, flags, fflags, data, udata }` with `from_bytes(&[u8; 32])` and `to_bytes()`, beside the existing `KeventQos`.
  - **The flag constants `sys/event.h` gives and the crate lacks:** `EVFILT_READ -1`, `EVFILT_WRITE -2`, `EV_RECEIPT 0x40`, `EV_ERROR 0x4000`, `EV_EOF 0x8000`, `EV_SYSFLAGS 0xF000`, `NOTE_FFAND 0x40000000`, `NOTE_FFOR 0x80000000`, `NOTE_FFCOPY 0xc0000000`, `NOTE_FFCTRLMASK 0xc0000000` and `NOTE_FFLAGSMASK 0x00ffffff`. Reuse the existing `EVFILT_USER`, `EV_ADD`, `EV_ENABLE`, `EV_CLEAR`, `EV_ONESHOT`, `EV_DELETE`, `EVFILT_TIMER` and `NOTE_TRIGGER`.
  - **The psynch constants Task 5 needs,** from `synch_internal.h` at the F1 tag, each with a source comment: `PTHRW_INC 0x100`, `PTHRW_COUNT_SHIFT 8`, `PTHRW_COUNT_MASK 0xffffff00`, `PTHRW_MAX_READERS 0xffffff00`, `PTH_RWL_MTX_WAIT 0x20`, `PTH_RWS_CV_CBIT 1`, `PTH_RWS_CV_PBIT 2`, `PTH_RWS_CV_MBIT 0x40` and `PTH_RWS_CV_BITSALL 3`. From `kern_internal.h`: `ECVCLEARED 0x100` and `ECVPREPOST 0x200`. Neither `ETIMEDOUT` nor `EINTR` exists in `retrace-arch`, so define both there, beside `EINVAL` and `ENOTSUP` and with their type: `pub const ETIMEDOUT: u64 = 60;` and `pub const EINTR: u64 = 4;`, each with a doc line citing `sys/errno.h`. The psynch sequence-word constants (`PTHRW_*`, `PTH_RWL_*`, `PTH_RWS_*`) are `u32`, the kernel's `uint32_t`; `ECVCLEARED` and `ECVPREPOST` are `u64`, because they are ORed into a returned errno word.

- [ ] **Step 4: The rows.** Add to `arg_kinds`:
  - **kevent:** `SYS_KEVENT => row!(P, [Fd, Ptr, Scalar, Ptr, Scalar, Ptr])`, for `kevent(kq, changelist, nchanges, eventlist, nevents, timeout)`. Its comment: `M48 §3b: emulated, never forwarded. The model reads and writes nchanges/nevents × 32 bytes through the stage-1 walk, the M45 kevent_qos row's precedent; the generic-arm assert keeps it off the forward path`.
  - **The psynch calls,** from their `kern_synch.c` prototypes, all `Ret::Plain`:

    | Number | Call | Row | Prototype |
    |---|---|---|---|
    | 303 | `cvbroad` | `[Ptr, Scalar, Scalar, Scalar, Ptr, Scalar, Scalar]` | `cv, cvlsgen, cvudgen, flags, mutex, mugen, tid` |
    | 304 | `cvsignal` | `[Ptr, Scalar, Scalar, Scalar, Ptr, Scalar, Scalar, Scalar]` | `cv, cvlsgen, cvugen, thread_port, mutex, mugen, tid, flags` |
    | 305 | `cvwait` | `[Ptr, Scalar, Scalar, Ptr, Scalar, Scalar, Scalar, Scalar]` | `cv, cvlsgen, cvugen, mutex, mugen, flags, sec, nsec` |

    Each is marked emulated, never forwarded, and cites the tag. The other psynch numbers stay rowless: Task 5 refuses them by value before any row is consulted.
  - **setsockopt:** `105 => row!(P, [Fd, Scalar, Scalar, Source, Scalar])` (`setsockopt(s, level, name, val, len)`: `val` is read for `len` bytes, the `write` row's convention). Its comment cites walls.md §1 row 6: libuv's Apple-only `SO_OOBINLINE` on the stdout pipe, answered `ENOTSOCK` by the host, which libuv ignores.

  Add to `nodeshapes.rs`, each reading the row's `(args, ret)` through `arg_kinds` as `gitshapes.rs` does:
  - `the_kevent_row_is_its_prototype`;
  - `the_psynch_and_setsockopt_rows_are_their_prototypes`.

- [ ] **Step 5: The census and `legacy_equivalence`.**
  - **`census.rs`:** add 105, 303, 304, 305 and 363 to `CENSUS` (sorted), with an M48 doc paragraph giving the measurement date, the walk evidence path (`docs/sweep-evidence/2026-10-02-m48-t0/`), and that these are node's kqueues, condvars and libuv's stdout `setsockopt`. Note that `getsockname` (32) is reached only when stdin is a socket, so it is not in the census.
  - **`legacy_equivalence.rs`'s `EXPECTED_DIFFS`:** add:
    - `(363, View::FdOperands, "kevent(kq, …): the kqueue fd is operand 0 — emulated above the forward (M48), so the view is moot; exercised (node, kq_dyn)")`
    - `(105, View::FdOperands, "setsockopt(s, …): a descriptor the legacy table never had — M48; exercised (node: libuv SO_OOBINLINE on the stdout pipe)")`
    - `(105, View::ReadsGuestBuffer, "setsockopt reads optval for optlen bytes — M48; exercised (node)")`

    The psynch rows carry only `Scalar`/`Ptr` kinds, so by the file's own rule they need no entry; add a one-line comment saying so. If the sweep finds any other difference, add it with its reason. It must not be taught to lie.

- [ ] **Step 6: The forward.** In `crates/retrace-core/src/machmsg.rs`, add `(3419, "semaphore_destroy")` to `FORWARD_ALLOWLIST` beside `(3418, "semaphore_create")`. Its comment cites walls.md §1 row 2: libuv's `uv_sem_destroy` after the loop thread's `uv_sem_post`, a complex message moving the semaphore's send right. It is forwarded and recorded like its create, and replay applies the recorded reply. Add the unit test:

```rust
    #[test]
    fn semaphore_destroy_is_forwarded_as_semaphore_create_is() {
        // M48 (walls.md §1 row 2): libuv's uv_sem_destroy, the mirror of 3418.
        assert!(matches!(route(&msg(3419, 0x203, KOBJ), Some(0x203)), Route::Forward("semaphore_destroy")));
        assert!(check_forward_body(3419, &[]).is_ok(), "3419 has no body-level check");
    }
```

- [ ] **Step 7: The generic-arm asserts.** In `record_box`'s generic `Stop::Syscall` arm, after the `kevent_qos` assert, add two asserts, each naming why forwarding is fatal:

```rust
// M48 §3b: kevent (363) on a guest kqueue is emulated above. Forwarded, it acts on a kqueue
// retrace never created, and a filter that blocks would block the RECORDER. This assert makes
// "never forwarded" a checked fact (the bsdthread_create gap M37 measured).
assert!(num != retrace_arch::SYS_KEVENT,
    "kevent (363) reached the generic forward arm — it must be emulated above (M48 §3c).");
// M48 §3e: every psynch call is handled above, modelled or refused by value. Forwarded, it acts
// on the HOST's psynch state keyed by retrace's own addresses, blocking or waking the recorder.
assert!(!retrace_arch::is_psynch(num),
    "psynch syscall {num} reached the generic forward arm — it must be emulated above (M48 §3e).");
```

These fire with no handler yet. No gate guest reaches 363 or a psynch number before Tasks 4–5: a rowless number already fails loud through M33, and M47's gate is green.

- [ ] **Step 8: Green, clippy, commit.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t2-arch.log 2>&1; echo "arch exit=$?"
cargo test -p retrace-core --lib --no-fail-fast -- --test-threads=1 > $L/t2-core.log 2>&1; echo "core exit=$?"
cargo clippy -p retrace-arch -p retrace-core --all-targets -- -D warnings > $L/t2-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git commit -m "M48 t2: kevent, psynch and setsockopt rows, is_psynch, the Kevent decoder and psynch constants, semaphore_destroy forwarded, the generic-arm asserts"
```

- [ ] **Step 9: Control (on the committed tree).**
  1. Delete the `SYS_KEVENT` row, run `cargo test -p retrace-arch --test census`, and confirm `every_census_number_has_a_row` goes red for 363.
  2. Then narrow `is_psynch` to `303..=305` and confirm `is_psynch_is_exactly_the_sdks_psynch_set` goes red, naming 297.
  3. Restore with `git checkout <t2 commit> -- crates/retrace-arch`, and record both symptoms.

---

### Task 3: Partial `munmap` (`retrace-box`, fixture, e2e)

walls.md §1 row 3 is a small model the spec did not name, absorbed by Ruling (§11b item 5).
- **The trim:** V8 over-allocates a reservation, then trims its head and tail to the alignment it wants.
- **Why it breaks today:** `guest_munmap` drops the whole backing that contains `addr` (its comment: "whole-backing unmap for M2's page-granular guests"), so the first trim releases the middle V8 keeps.
- **The measured volume:** 56 partial munmaps per walk, including unaligned lengths such as `0x10b20`.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`guest_munmap`, the new `unmap_range`, the `Backing` and `free_pages` contracts)
- Modify: `crates/retrace-box/src/backings.rs` (`Backings::insert` and `SpanIndex::insert_at`, which `unmap_range`'s split needs: `Backings` has only `push`, `extend` and `remove`)
- Create: `crates/retrace-box/tests/trim.rs`
- Create: `crates/retrace-guest/c/trim_dyn.c`; modify `build.rs`, `src/lib.rs`
- Create: `crates/retrace/tests/trim_e2e.rs`

**Interfaces:**
- Consumes: t0 M2's partial-`munmap` shapes (`T0(M2)`: heads, tails, interior punches, unaligned lengths, any span over two backings). The design below handles every one; the addendum records which occur.
- Produces: `guest_munmap(ipa, len)`, which releases exactly `[ipa & !(G-1), round_up(ipa + len))` from every backing it overlaps. Task 6 adds one line to it, for `MAP_JIT` ranges.

- [ ] **Step 1: Controller addendum.** Write `$L/task-3-addendum.md` from §M2: the measured shapes and their counts. Do not start until it exists.

- [ ] **Step 2: The box-level tests, red first.** Create `crates/retrace-box/tests/trim.rs`:

```rust
//! M48 Task 3, box level: `guest_munmap` releases exactly the pages it is asked to, splitting a
//! backing the range cuts (walls.md §1 row 3: V8's aligned-reservation trim). Static box: the MMU
//! is on with an identity map (`load_with_pac` sets `sctlr_mmu_on`), so VA == IPA, and these tests
//! see stage 2, which is where a release happens.
use retrace_box::Box_;

const ANON: u64 = 0x1002; // MAP_ANON | MAP_PRIVATE
const RW: u64 = 3;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

#[test]
fn a_head_trim_keeps_the_rest_mapped_with_its_bytes() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    b.poke_guest(a + 0x20000, b"middle");
    b.guest_munmap(a, 0x10000);
    assert!(!b.is_mapped(a) && !b.is_mapped(a + 0xc000), "the head is released");
    assert!(b.is_mapped(a + 0x10000) && b.is_mapped(a + 0x7bfff), "the rest stays mapped");
    assert_eq!(b.read_bytes_for_test(a + 0x20000, 6), b"middle");
}

#[test]
fn a_tail_trim_with_an_unaligned_length_rounds_its_end_up() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    b.guest_munmap(a + 0x50000, 0x2bb20); // ends at a + 0x7bb20, which rounds up to a + 0x7c000
    assert!(b.is_mapped(a + 0x4ffff));
    assert!(!b.is_mapped(a + 0x50000) && !b.is_mapped(a + 0x7bfff), "the whole rounded tail is released");
}

#[test]
fn an_interior_punch_splits_one_backing_into_two() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x40000, RW, ANON).unwrap();
    b.poke_guest(a, b"head");
    b.poke_guest(a + 0x3c000, b"tail");
    let before = b.mapped_len();
    b.guest_munmap(a + 0x10000, 0x8000);
    assert_eq!(b.mapped_len(), before - 0x8000, "exactly the punched pages leave the map");
    assert!(!b.is_mapped(a + 0x10000) && !b.is_mapped(a + 0x17fff));
    assert_eq!(b.read_bytes_for_test(a, 4), b"head");
    assert_eq!(b.read_bytes_for_test(a + 0x3c000, 4), b"tail");
}

#[test]
fn a_munmap_spanning_two_backings_drops_one_and_trims_the_other() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x10000, RW, ANON).unwrap();
    let c = b.guest_mmap(0, 0x10000, RW, ANON).unwrap();
    assert_eq!(c, a + 0x10000, "non-FIXED anon mmaps pack");
    b.guest_munmap(a, 0x18000);
    assert!(!b.is_mapped(a) && !b.is_mapped(c + 0x7fff));
    assert!(b.is_mapped(c + 0x8000), "the second backing keeps its tail");
}

#[test]
fn a_split_then_full_teardown_returns_every_byte() {
    let mut b = tb();
    let base = retrace_box::live_backing_bytes();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    b.guest_munmap(a, 0x10000);
    b.guest_munmap(a + 0x50000, 0x2bb20);
    b.guest_munmap(a + 0x20000, 0x8000);
    b.guest_munmap(a + 0x10000, 0x40000);
    assert_eq!(retrace_box::live_backing_bytes(), base, "every host page is released exactly once");
}

#[test]
fn a_split_backing_survives_a_checkpoint() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x7c000, RW, ANON).unwrap();
    // A backing placed after `a`'s: its Vec position shifts up when `a` splits.
    let c = b.guest_mmap(0, 0x4000, RW, ANON).unwrap();
    b.poke_guest(a + 0x20000, b"kept");
    b.poke_guest(c, b"after");
    b.guest_munmap(a, 0x10000);
    b.guest_munmap(a + 0x50000, 0x2bb20);
    assert_eq!(b.read_bytes_for_test(c, 5), b"after", "the index followed the shift");
    let st = b.checkpoint();
    drop(b); // one VM per process
    let r = Box_::from_checkpoint(&st);
    assert!(!r.is_mapped(a) && !r.is_mapped(a + 0x50000));
    assert!(r.is_mapped(a + 0x10000) && r.is_mapped(a + 0x4ffff));
    assert_eq!(r.read_bytes_for_test(a + 0x20000, 4), b"kept");
    assert_eq!(r.read_bytes_for_test(c, 5), b"after");
}
```

Run red. Every test but `a_tail_trim_…` fails on the whole-backing unmap, and that one fails on its first assert.

- [ ] **Step 3: The split.** First give `Backings` an insert. `unmap_range` puts a cut backing's head and tail back at the cut backing's own Vec position, so `snapshot` and `checkpoint` walk them where they walked it (M44 R2: the Vec's order is what they iterate). `Backings` has no insert today, only `push`, `extend` and `remove`, and its index is keyed by Vec position, so an insert must shift every later position up by one, as `remove` shifts them down. In `crates/retrace-box/src/backings.rs`, add to `impl SpanIndex`, after `remove`:

```rust
    /// Index a span inserted at Vec position `pos`: every later position shifts up by one, as
    /// `Vec::insert` does, then the span is indexed as `insert` does. Panics if it overlaps.
    pub(crate) fn insert_at(&mut self, start: u64, len: usize, pos: usize) {
        for x in &mut self.e { if x.2 >= pos { x.2 += 1; } }
        self.insert(start, len, pos);
    }
```

and to `impl Backings`, after `remove`:

```rust
    // Vec first, index second, as `push`: if the overlap assert fires, the Backing is already in the
    // Vec and drops with it after `vm` (the field order).
    pub(crate) fn insert(&mut self, pos: usize, b: Backing) {
        let (ipa, len) = (b.ipa, b.len);
        self.v.insert(pos, b);
        self.idx.insert_at(ipa, len, pos);
    }
```

No unit test is added to `backings.rs`, which keeps the plan's test count: `a_split_backing_survives_a_checkpoint` (Step 2) maps a second region after the split one, so the shift of a later backing's position is read back through the index before and after a restore. Then, in `crates/retrace-box/src/lib.rs`, replace `guest_munmap`'s backing block with a call to a new `unmap_range`:

```rust
    pub fn guest_munmap(&mut self, ipa: u64, len: u64) {
        let g = GRANULE as u64;
        // The kernel's rounding (`mach_vm_deallocate`: start down, end up). V8 passes unaligned
        // lengths (walls.md §1 row 3: 0x10b20).
        let start = ipa & !(g - 1);
        let end = (ipa.saturating_add(len) + g - 1) & !(g - 1);
        self.subtract_reservations(ipa, len);
        // M13: the pages are gone, so the protection goes with them — otherwise the next mapping at
        // this address inherits a no-access extent its guest never asked for. Runs BEFORE the
        // stage-2 unmap below so the pages are still backed while their leaves are reset.
        self.drop_protection(ipa, len);
        if end > start { self.unmap_range(start, end); }
    }

    /// M48 Task 3: release `[start, end)` (page-aligned) from every backing it overlaps. A backing
    /// wholly inside the range is dropped, as before M48. A backing the range cuts keeps the part
    /// outside it: its stage-2 mapping is removed, the head and tail are re-mapped as Backings of
    /// their own over the same host pages, and only the cut pages go back to the host. That is
    /// V8's aligned-reservation trim (walls.md §1 row 3), which the whole-backing unmap broke by
    /// releasing the middle V8 keeps. The stage-1 identity block stays, as it always has.
    fn unmap_range(&mut self, start: u64, end: u64) {
        let mut i = 0;
        while i < self.backings.len() {
            let (bs, be) = (self.backings[i].ipa, self.backings[i].ipa + self.backings[i].len as u64);
            if be <= start || end <= bs { i += 1; continue; }
            let bk = self.backings.remove(i);
            let _ = self.vm.unmap(bs, bk.len);
            if start <= bs && be <= end { drop(bk); continue; } // wholly inside: the pre-M48 path
            // Cut. The host pages pass from `bk` to its pieces, so forget `bk`, whose Drop would
            // release them all.
            let host = bk.host;
            std::mem::forget(bk);
            let (cs, ce) = (start.max(bs), end.min(be));
            if cs > bs {
                let l = (cs - bs) as usize;
                self.vm.map(host, bs, l, MemFlags::RWX).expect("hv_vm_map (munmap head)");
                self.backings.insert(i, Backing { host, ipa: bs, len: l });
                i += 1;
            }
            if ce < be {
                // SAFETY: `ce - bs` lies inside this allocation, which is `be - bs` bytes long.
                let h = unsafe { host.add((ce - bs) as usize) };
                let l = (be - ce) as usize;
                self.vm.map(h, ce, l, MemFlags::RWX).expect("hv_vm_map (munmap tail)");
                self.backings.insert(i, Backing { host: h, ipa: ce, len: l });
                i += 1;
            }
            // SAFETY: the cut pages `[cs, ce)` are page-aligned, inside this allocation, unmapped
            // at stage 2 just above, and owned by no Backing, which is `free_pages`'s contract.
            unsafe { free_pages(host.add((cs - bs) as usize), (ce - cs) as usize); }
        }
    }
```

Rewrite the two contracts that the split widens. Neither may be left saying something false.
- **`Backing`'s doc comment.** "Each Backing owns exactly the host pages `[host, host+len)`, a whole number of host pages. `alloc_pages` returns one such range. `unmap_range` (M48) splits one into disjoint pieces. No two Backings ever cover one host page." This replaces "every Backing must carry exactly the length `alloc_pages` returned". Add `unmap_range` to the M40 audit's list of construction and removal sites.
- **`free_pages`' `# Safety`.** "`host`/`len` must be a page-aligned range of live `alloc_pages` memory that no Backing owns, no longer mapped into the guest, and never touched again." Its two existing callers already meet this.

macOS's host page is 16 KiB, which is `GRANULE`, so a `libc::munmap` of any granule-aligned piece is exact.

`unmap_overlapping` (`place_fixed`'s helper) is unchanged: it is reached only in the fully-covering case.

This task does not model `munmap`'s own `EINVAL` for an unaligned start or a zero length. That is pre-existing, and V8 was measured never to pass either. `guest_munmap` rounds the start down, as `mach_vm_deallocate`, which shares it, requires.

- [ ] **Step 4: The fixture.** Create `crates/retrace-guest/c/trim_dyn.c`:

```c
// M48 Task 3: V8's aligned-reservation trim (walls.md §1 row 3). Map 0x7c000 bytes, unmap a
// 0x10000 head and a tail whose length is not page-aligned (the kernel rounds its end up), then use
// the middle. Modes: trim (the middle keeps its bytes and takes writes), and head and tail (touching
// a released part faults, as it does natively). The TRIM marker names the two addresses the
// fault modes touch.
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "trim";
    uint8_t *p = mmap(NULL, 0x7c000, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (p == MAP_FAILED) { puts("mmap failed"); return 2; }
    for (size_t i = 0; i < 0x7c000; i++) p[i] = (uint8_t)(i * 7);
    printf("TRIM head=%p tail=%p\n", (void *)p, (void *)(p + 0x7c000 - 8));
    fflush(stdout);
    if (munmap(p, 0x10000) != 0 || munmap(p + 0x50000, 0x2bb20) != 0) { puts("munmap failed"); return 2; }
    if (!strcmp(mode, "head")) { volatile uint64_t *q = (volatile uint64_t *)p; return (int)*q; }
    if (!strcmp(mode, "tail")) { volatile uint64_t *q = (volatile uint64_t *)(p + 0x7c000 - 8); return (int)*q; }
    uint64_t sum = 0;
    for (size_t i = 0x10000; i < 0x50000; i++) sum = sum * 31 + p[i];
    memset(p + 0x10000, 0xab, 0x40000);
    for (size_t i = 0x10000; i < 0x50000; i++) if (p[i] != 0xab) { puts("the middle lost a write"); return 1; }
    printf("trim ok sum=%#llx\n", (unsigned long long)sum);
    return 0;
}
```

Wire it with the `madv_dyn` recipe. Add `pub const TRIM_DYN` and a `trim_dyn_guest_parses` unit test.

- [ ] **Step 5: `trim_e2e`.** Create `crates/retrace/tests/trim_e2e.rs` with three tests:
  - **`a_trimmed_reservation_keeps_its_middle_and_replays`.**
    1. Record `trim`; it must exit 0.
    2. Its second stdout line must equal native's second line. The `trim ok sum=…` line is address-independent; the `TRIM` line is not.
    3. Two replays must match the recorded stdout byte for byte.
  - **`the_trimmed_head_and_the_rounded_tail_are_gone`.** For each of `head` and `tail`:
    1. Record; it must exit 139, and native must exit 139 too.
    2. Parse the marker's address for that mode.
    3. Read the trace's terminal `Event::Crash` with `retrace_trace::Reader`, as `cpython_crash_e2e::crash_and_marker_thread` does, and assert its `far` equals that address.
    4. Two replays must exit 139.

    The `tail` mode is what proves the unaligned length's end was rounded up.
  - **`a_seek_across_the_trim_matches_a_cold_seek`.** This is the `gcdtimer_e2e` seek test's pattern, on the `trim` trace.
    1. Find the two `munmap` (73) landmarks.
    2. Checkpoint at the landmark between them, restore with `from_checkpoint`, and advance past the second.
    3. Compare `current_thread`, `dbg_regs`, `dbg_fp_regs`, `dbg_internal_state` and `diff_memory` with a cold seek to the same landmark.

    This is the restore-parity guard for split backings.

- [ ] **Step 6: Green, regression, clippy, commit.** Every guest unmaps memory, so run the box chunk and the gates whose guests unmap the most:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t3-box.log 2>&1; echo "box exit=$?"
for t in trim_e2e hello_dyn_e2e hello_rust_e2e jq_e2e jq_file_e2e cpython_e2e cpython_crash_e2e protnone_rust_e2e segv_rust_e2e thread_rust_e2e dispatch_e2e vmremap_e2e git_e2e apple_walls_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t3-$t.log 2>&1; echo "$t exit=$?"; done
cargo clippy -p retrace-box -p retrace-guest -p retrace --all-targets -- -D warnings > $L/t3-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git commit -m "M48 t3: partial munmap splits the backing it cuts (V8's aligned-reservation trim), trim_dyn and trim_e2e"
```

If a target name above does not exist in `crates/retrace/tests/`, drop it and say so in the report; do not invent one.

- [ ] **Step 7: Control (on the committed tree).**
  1. Make `unmap_range` drop every overlapping backing whole (`drop(bk); continue;` for every overlap).
  2. Confirm `a_head_trim_keeps_the_rest_mapped_with_its_bytes` goes red, and that `trim_e2e`'s first test records exit 139 at a FAR in the middle.
  3. Restore with `git checkout <t3 commit> -- crates/retrace-box/src/lib.rs`, and record the symptom.

---

### Task 4: One deadline queue and guest kqueues (K1) (`retrace-box`, `retrace-core`, fixture, e2e)

M48 §3c and §3d, with §11a item 6 and §11b item 8 and the header's "Delivery happens at the wake" and "A deadline already past at the call is not special-cased" constraints. The queue's table half lands first (Steps 2–3), because Task 5's condition variables block on it too; the queue itself joins `schedule_after_block` in Step 7.
- **The queue.** A blocked thread may carry a deadline on the guest clock. `schedule_after_block` wakes every due waiter, and its one idle jump goes to the earliest of all deadlines, M46's timers included (R7).
- **The kqueues.** A pure `gkq.rs` holds each guest kqueue's knotes and the guest pipes' byte counts (R4). `Box_::guest_kevent` applies a call's changes, then answers, or blocks the caller. A wake writes its reply where the woken thread will read it: the live vCPU when it is the current thread, else its saved context (`deliver_wake`).
- **Never forwarded, nothing recorded.** The `kevent` arm and its mirror call the same method with the same arguments, and the events carry no writes (R3). The fd-lifecycle hook `note_fd_effects` runs after the generic forward on record and after the generic mirror on replay.

**Files:**
- Modify: `crates/retrace-box/src/thread.rs` (`BlockReason::Kevent`, `BlockReason::deadline`, `ThreadTable::{wake, due_waiters, earliest_deadline}`, 2 unit tests)
- Create: `crates/retrace-box/src/gkq.rs` (11 unit tests)
- Modify: `crates/retrace-box/src/lib.rs` (`pub mod gkq;`; the `gkq` field through every path; `wake_due_threads` and the deadline queue in `schedule_after_block`; `guest_kevent`, `kevent_timed_out`, `note_fd_effects`, `deliver_wake`; `dbg_gkq`)
- Modify: `crates/retrace-core/src/lib.rs` (the `kevent` arm in `record_box`, its mirror in `ReplaySession::advance`, the `note_fd_effects` call in record's generic arm and console-close arm and in replay's generic mirror)
- Create: `crates/retrace-box/tests/gkq.rs` (10 tests)
- Create: `crates/retrace-guest/c/kq_dyn.c`; modify `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` (`KQ_DYN`, `kq_dyn_guest_parses`)
- Create: `crates/retrace/tests/kq_e2e.rs` (12 tests)

**Interfaces:**
- Consumes:
  - Task 2: `retrace_arch::{SYS_KEVENT, Kevent, EVFILT_READ, EVFILT_WRITE, EVFILT_USER, EVFILT_TIMER, EV_ADD, EV_ENABLE, EV_DELETE, EV_CLEAR, EV_ONESHOT, EV_RECEIPT, EV_EOF, EV_SYSFLAGS, NOTE_TRIGGER, NOTE_FFAND, NOTE_FFOR, NOTE_FFCOPY, NOTE_FFCTRLMASK, NOTE_FFLAGSMASK}`, the 363 row, and the generic-arm assert against 363. `Kevent` is `{ pub ident: u64, pub filter: i16, pub flags: u16, pub fflags: u32, pub data: i64, pub udata: u64 }` with `from_bytes(&[u8; 32]) -> Kevent` and `to_bytes(&self) -> [u8; 32]`, deriving `Debug, Clone, Copy, PartialEq, Eq` as `KeventQos` does. Task 2's list stops short of `EV_DISABLE` (0x0008, `sys/event.h`), so `gkq.rs` defines it (`gkq::EV_DISABLE`) rather than reopening `retrace-arch`.
  - t0 M3 (`T0(M3)`): the native answers listed in Step 1.
  - M46: `kq::tsc_for_deadline`, `Box_::{now_guest, timebase_offset, fire_due_timers, write_va_committing, read_va_prefix, fmt_args}`. M38/M37: `retrace_arch::{returns_fd_pair, is_close_syscall, is_write_syscall, is_fcntl_dupfd, SYS_DUP, SYS_DUP2, SYS_READ, SYS_READ_NOCANCEL}`, `FdTable::is_open`.
- Produces (Tasks 5, 6 and 10 consume these names exactly):
  - **`thread::BlockReason::Kevent { kq: u64, deadline: Option<u64> }`.** `kq` is the guest kqueue fd. `deadline` is a guest-clock value (the guest's `mach_absolute_time`, `Box_::now_guest`'s domain, the same domain as M46's timer deadlines), and `None` for a NULL timeout. `BlockReason` stays `Copy + Eq`.
  - **`impl BlockReason { pub fn deadline(&self) -> Option<u64> }`.** `Kevent { deadline, .. } => deadline`; every other variant is `None`. Task 5 adds `BlockReason::Cv { .. }`'s arm to this match.
  - **`ThreadTable::wake(&mut self, tid: usize) -> Result<(), String>`.** Refuses, with nothing changed, when `tid`'s pending-signal set is nonzero: `Err("M48: a signal is pending on thread {tid} (set {pending:#x}) …")`. Otherwise asserts `tid` is `Blocked(BlockReason::Kevent { .. })` (a scheduling bug, so a panic) and makes it `Runnable`. Task 5 widens the assert's pattern to `Kevent { .. } | Cv { .. }`.
  - **`ThreadTable::due_waiters(&self, now: u64) -> Vec<usize>`.** Every `Blocked(r)` thread with `r.deadline() <= Some(now)` (a `Some`), ordered by `(deadline, tid)` (R7: ties by thread index). Pure; wakes nobody.
  - **`ThreadTable::earliest_deadline(&self) -> Option<u64>`.** The least `deadline()` over `Blocked` threads.
  - **`Box_::deliver_wake(&mut self, tid: usize, ret: u64, err: bool, events: &[(u64, Vec<u8>)]) -> Result<(), String>`** (pub, so the box tests drive it). In order:
    1. `self.threads.wake(tid)?`, so a refusal comes before any write;
    2. each `(va, bytes)` is written through the guest's stage-1 walk with `write_va_committing` (a VA that no longer translates panics `M48: kevent event list …`; `guest_kevent` checked it at the call);
    3. `x0 = ret`, and PSTATE.C set iff `err`. When `tid == self.threads.current()` these go onto the live vCPU (`reg::x(0)`, the C bit of `reg::CPSR`); otherwise into `self.threads.ctx_mut(tid).regs.x[0]` and the C bit of `….regs.cpsr`.

    It never touches PC, ELR or SPSR: the blocking landmark's `set_x0_err_and_return(0, false)` already made the context a post-return one. Task 5 calls it with `events = &[]`.
  - **`Box_::wake_due_threads(&mut self)`** (private). Reads the clock only when `self.threads.earliest_deadline().is_some()`, so a static box and every pre-M48 guest never reach it. For each tid of `due_waiters(now_guest())`, in order, it first skips a thread that is no longer `Blocked`, because an earlier wake in the same pass may have woken it (Task 5's psynch timeout can). Then it dispatches on the thread's reason:
    ```rust
    match self.threads.state_of(tid) {
        thread::ThreadState::Blocked(thread::BlockReason::Kevent { kq, .. }) => self.kevent_timed_out(tid, kq),
        s => unreachable!("M48: due_waiters returned thread {tid} in {s:?}, which has no deadline"),
    }
    ```
    Each arm returns `()` and panics with a refusal's text itself (below the trace, R5): `fn kevent_timed_out(&mut self, tid: usize, kq: u64)`. Task 5 adds `thread::ThreadState::Blocked(thread::BlockReason::Cv { addr, .. }) => self.cv_timed_out(tid, addr),` as the second arm, directly before the `s => unreachable!` arm. Deadlines are guest-clock values (`now_guest()`), never raw `synthetic_tsc`.
  - **`schedule_after_block`, in order:**
    1. `fire_due_timers()` (M46);
    2. `wake_due_threads()`;
    3. `pick_next`.

    With nothing runnable, the one idle jump goes to `min(self.kq.earliest_deadline(), self.threads.earliest_deadline())` (either may be `None`) through `kq::tsc_for_deadline`, then 1–2 run again and the pick is retried once. Otherwise the deadlock panic lists every thread's state, which names each reason and its deadline, plus `kq` and `gkq`.
  - **`Box_::guest_kevent(&mut self, args: [u64; 8]) -> Result<(u64, bool), String>`.** `(ret, err)`: the immediate count, or `0` when the caller blocks, or `(EFAULT, true)` for a pointer argument that does not translate. `Err` is the refusal text, starting `M48: kevent ` or `M48: pipe `.
  - **`Box_::note_fd_effects(&mut self, num: u64, args: [u64; 8], ret: u64, ret1: u64, err: bool) -> Result<(), String>`.** The fd-lifecycle hook (Step 7 lists what it does per call). Called after the call's return is set, from three sites: on record by the generic forward arm (with the forward's `(ret, ret1, err)`) **and by the console-close arm** (`(0, 0, false)`), and on replay by the generic mirror (with the recorded values). The console-close arm is the one record arm whose landmark replay finishes through the generic mirror (its own comment says so), so without its call the two sides would see different closes (Ruling K6). `Err` starts `M48: kevent ` or `M48: pipe `.
  - **`Box_::gkq: gkq::GuestKqueues`**, declared after `excl`, and `#[doc(hidden)] pub fn dbg_gkq(&self) -> &gkq::GuestKqueues`.
  - **The field-through-every-path pattern** (Tasks 5 and 6 copy it for `psynch` and `jit`). A new `Box_` field `f` appears at exactly six sites:
    1. the `Box_` struct, after `excl`, so the `vcpu`-before-`vm` drop order is untouched;
    2. `BoxState`, as `pub f`, after `excl`, with a comment saying why a mid-run capture cannot re-derive it;
    3. `checkpoint()`: `f: self.f.clone()`;
    4. `from_checkpoint`: `f: state.f.clone()`;
    5. the three literal constructors, `load_with_pac` (lib.rs:1518), `load_dynamic` (:2120) and `restore` (:3400), each with `f: <Type>::default()`. A snapshot is taken at process start, so empty is right there;
    6. `dbg_internal_state`'s format string, which gains ` f={:?}`. That is what `checkpointparity.rs` and every e2e seek test compare, so a field dropped from 2–4 fails a seek test.

    `BoxState` derives only `Clone`, so the field's type needs `Clone + Debug + Default + PartialEq` and no serde.
  - **`retrace_box::gkq`**, pub: `GuestKqueues`, `Kqueue`, `Knote`, `FdKind`, `Pipes` (Step 5 gives each signature).
  - **`retrace_guest::KQ_DYN`**, with modes `probe`, `wake`, `timeout`, `tryselect`, `pipe`, `oneshot` and `bad` (`bad filter`, `bad notkq`).
  - **Refusal texts** (the header's prefixes): `M48: kevent change {i}: …`, `M48: kevent on fd {fd}…`, `M48: kevent on kq {kq}: …`, `M48: kevent nchanges …`, `M48: kevent timeout …`, `M48: kevent change list …`, `M48: kevent event list …`, `M48: pipe {fd}: …`, and `M48: a signal is pending on thread {tid} …`. Replay wraps a refusal as `kevent refused on replay, though the recording accepted it — replay diverged before this landmark: <message>`, or `syscall {num} refused on replay, …` from the hook in the generic mirror. A replay-side mismatch starts `kevent rc mismatch: replay `.

**Rulings made in this task** (numbered K so they do not collide with the spec's R1–R9):
- **K1. The pipe model stays** (the brief's open question). `gkq.rs` keeps `Pipes`, `note_fd_effects` keeps the byte counts, the `M48: pipe ` prefix stays, and `kq_dyn` keeps its `pipe` mode. The reasons:
  - spec §3c, R4 and §3h name it;
  - t0 M3 measures its constants (Task 0 Step 7);
  - `note_fd_effects` must exist anyway for the kqueue lifecycle (`kqueue`, `close`, `dup`), so the counts are a few lines more.

  Node never reaches it (walls.md §3: "no fd filter on a guest pipe is ever registered"), so `kq_dyn pipe` is its only guard. A `M48: pipe ` refusal fires only when a kqueue watches a pipe in a state the model cannot answer: a byte count it lost, or a count past the measured buffer. A guest with no kqueue, which is every gate guest before M48, can never meet one. The header needs no amendment for this.
- **K2. One waiter per kqueue.** A call with `nevents > 0` on a kqueue another thread is blocked on is refused (`M48: kevent on kq {kq}: …`). Natively both threads wake and race for the event. Node has one waiter per kqueue, and its cross-thread triggers carry `nevents == 0` (P3), which stays admitted.
- **K3. The one external-descriptor answer** (§11b item 7). `EVFILT_READ` with flags exactly `EV_ADD|EV_ENABLE`, on an open descriptor that is not a guest pipe end, registers a knote that never activates. That is native's answer for a piped stdout (walls.md §1; t0 M3 `read on fd 1, 1 ns`: `n=0`). For a stdout redirected to a regular file, native would report it readable. libuv's `uv__stream_try_select` keeps kqueue for the descriptor on 0 events or on any event without `EV_ERROR`, so the deviation does not change node's path. Task 11 names it in Known limits. Every other filter or flag set on such a descriptor is refused.
- **K4. No change ever produces an output.** Every change the kernel answers with an `EV_ERROR` event is refused by value: `ENOENT` on an unregistered knote, `EBADF` on a closed descriptor, `EV_RECEIPT`, and the rest. So F7's scan condition ("`nevents > 0` and no change produced an output") reduces to `nevents > 0`.
- **K5. Which bad pointers are `EFAULT`.** The kernel copies the timeout in before it looks the kqueue up, and each change in before applying it. So an unmapped timeout, or an unmapped first change, answers `(EFAULT, carry set)` with nothing applied, as native does. Two cases are refused by value instead, because the kernel applies changes before its `EFAULT` and the model does not reproduce a partial application:
  - a change list that maps only in part;
  - an event list that does not translate in full.
- **K6. The console-close arm calls the hook.** Record's console-close arm is the one arm whose landmark replay finishes through the generic mirror. Without the call there, replay alone would see that close. It matters only if a kqueue watches fd 0–2, but symmetry rule 1 wants the same `Box_` method with the same arguments on both sides by construction, not by an argument that the difference is unobservable. So `.note_fd_effects(` has three call sites in `retrace-core`: the console-close arm and the generic arm on record, and the generic mirror on replay. The header (line 59 and the `retrace-core` row) and Task 10's audit check 2 expect exactly those three.
- **K7. A deadline wake scans once more.** `kevent_timed_out` takes whatever the kqueue holds, up to the waiter's `nevents`. That is ordinarily nothing, because every activation path wakes the waiter at once. If a path ever missed a wake, the event is still delivered rather than silently left behind.
- **K8. A wake with a signal pending is refused** (`M48: a signal is pending on thread `). The kernel interrupts a `kevent` wait with `EINTR`, and that answer is unmeasured. A reply written over the blocked context would make the signal vanish where `assert_no_stranded_signals` cannot see it. This is M46's `unpark` posture.
- **K9. A touch.** A change on a registered knote:
  - replaces its `udata` (`native/kqdetect.out`'s `retrigger` delivered the trigger's `udata` 0x55);
  - keeps its creating flags (`detect` delivered `0x21` after a trigger whose flags were 0);
  - applies `EV_ENABLE`/`EV_DISABLE`;
  - for `EVFILT_USER`, applies `filt_usertouch` (F8).

  A touch that changes `EV_CLEAR` or `EV_ONESHOT` is refused, because it is unmeasured.
- **K10. Box tests and the clock.** A static box has no commpage, so it has no guest clock (`timebase_offset` panics; `kqmanager.rs`'s module doc). The box tests therefore use NULL or zero timeouts on a static box. The two deadline tests use a dynamic box (`load_dynamic` of `hello_dyn`, the `stackgrow.rs` pattern), whose stage 1 is identity-mapped, so `guest_mmap`'s IPA is also its VA.

- [ ] **Step 1: Controller addendum.** Write `$L/task-4-addendum.md` pinning, from measurements §M3:
  - `gkq::PIPE_CAPACITY`: the `data` of `write-ready on an empty pipe` (expected 16384).
  - `gkq::ADD_FLAGS_RETURNED`: the bits of a knote's creating flags that a delivery returns. Read it off `read on the write end, reader closed`, whose knote was added with `EV_ADD|EV_ENABLE`:
    - flags `0x8005` gives `0x0fff` (`!EV_SYSFLAGS`: every input bit is kept);
    - flags `0x8001` gives `0x0ff3` (`!(EV_SYSFLAGS | EV_ENABLE | EV_DISABLE)`).
  - That `detect`, `detect slot 1 afterwards`, both 1 ns reads, `read-ready after 3 bytes` (`data=3`, flags `0x1`), `write-ready after 3 bytes` (capacity − 3) and `write on the write end, reader closed` (`EV_EOF`, `data` 0) are as Task 0 Step 7's table expects. Any other answer is a Ruling written here before Task 4 starts.
  - From the §M3 census, three facts K1–K3 rest on:
    - no kqueue fd is ever dup'd;
    - no two threads wait on one kqueue;
    - the largest `nchanges` and `nevents` seen (expected 2 and 1024).

  Do not start until it exists.

- [ ] **Step 2: The deadline queue's table half, red first.** Append to the `tests` module of `crates/retrace-box/src/thread.rs`:

```rust
    // M48 §3d, R7: the queue's order is the deadline, then the thread index, and a deadline equal
    // to now is due. A wait with no timeout carries no deadline and is never due.
    #[test]
    fn due_waiters_are_in_deadline_order_with_ties_by_thread_index() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        for _ in 0..3 { t.spawn(ThreadCtx::zeroed(), (0, 0)); }
        // `block` acts on the current thread, so switch to each one first.
        for (tid, deadline) in [(0, Some(300)), (1, Some(100)), (2, None), (3, Some(100))] {
            t.switch_to(tid);
            t.block(BlockReason::Kevent { kq: 3 + tid as u64, deadline });
        }
        assert_eq!(t.earliest_deadline(), Some(100));
        assert!(t.due_waiters(99).is_empty());
        assert_eq!(t.due_waiters(100), vec![1, 3], "a deadline equal to now is due; ties by thread index");
        assert_eq!(t.due_waiters(u64::MAX), vec![1, 3, 0], "deadline order; the NULL timeout is never due");
        t.wake(1).unwrap();
        assert_eq!(t.state_of(1), ThreadState::Runnable);
        assert_eq!(t.earliest_deadline(), Some(100), "thread 3's, now that 1 is awake");
    }

    // M48 K8: a wake refuses a thread with a signal pending, masked or not, and changes nothing.
    #[test]
    fn a_wake_refuses_a_thread_with_a_signal_pending_and_changes_nothing() {
        let mut t = ThreadTable::new(ThreadCtx::zeroed());
        t.block(BlockReason::Kevent { kq: 3, deadline: None });
        t.pend(0, 30);
        let e = t.wake(0).unwrap_err();
        assert!(e.starts_with("M48: a signal is pending on thread 0 (set 0x20000000"), "{e}");
        assert_eq!(t.state_of(0), ThreadState::Blocked(BlockReason::Kevent { kq: 3, deadline: None }));
        assert_eq!(BlockReason::Kevent { kq: 3, deadline: Some(7) }.deadline(), Some(7));
        assert_eq!(BlockReason::Wait { addr: 8 }.deadline(), None, "only a timed wait carries a deadline");
    }
```

Run it red; the variant and the methods do not exist yet:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --lib thread::tests --no-fail-fast -- --test-threads=1 > $L/t4-thread-red.log 2>&1; echo "exit=$?"; grep -a -E 'error\[|no variant|no method' $L/t4-thread-red.log | head
```

Expected: exit 101, with `no variant or associated item named `Kevent`` and `no method named `due_waiters``.

- [ ] **Step 3: `BlockReason::Kevent`, `deadline()`, `wake`, `due_waiters`, `earliest_deadline`.** In `crates/retrace-box/src/thread.rs`, add the variant after `Parked`:

```rust
    /// M48 §3c: blocked in `kevent` on the guest kqueue `kq` (its guest fd).
    ///
    /// `deadline` is on the guest clock (`Box_::now_guest`'s domain, the same as M46's timer
    /// deadlines), or `None` for a NULL timeout. A 1 ns timeout converts to 0 ticks, so its deadline
    /// is the call's own "now": it blocks and is woken in the same `schedule_after_block` (§11b item
    /// 8). Woken by a change that activates one of the kqueue's knotes, by a pipe write it watches,
    /// or by its deadline (`Box_::wake_due_threads`). The waiter's event list lives in `gkq`, not
    /// here, so the variant stays `Copy`.
    Kevent { kq: u64, deadline: Option<u64> },
```

Directly after the `BlockReason` enum's closing brace, add:

```rust
impl BlockReason {
    /// M48 §3d: the guest-clock deadline a timed wait ends at. `None` for a wait with no timeout and
    /// for every reason that is not a timed wait. The match names every variant, so a new blocking
    /// primitive must say here whether it carries one.
    pub fn deadline(&self) -> Option<u64> {
        match *self {
            BlockReason::Kevent { deadline, .. } => deadline,
            BlockReason::Join { .. } | BlockReason::Wait { .. } | BlockReason::Sem { .. }
            | BlockReason::Parked => None,
        }
    }
}
```

In `impl ThreadTable`, after `unblock_sem_waiters_on`, add:

```rust
    /// M48 §3c–§3d: make `tid`, blocked in a timed-wait primitive, runnable. Its caller writes the
    /// reply (`Box_::deliver_wake`).
    ///
    /// **Refuses, with nothing changed, when a signal is pending on it, masked or not** (K8). The
    /// kernel interrupts these waits with `EINTR`, which is unmeasured. A reply written over the
    /// blocked context would make the signal vanish where `assert_no_stranded_signals` cannot see
    /// it, with record and replay agreeing: `unpark`'s posture, as a `Result` because a wake can
    /// be reached above the trace (a trigger) as well as below it (a deadline).
    pub fn wake(&mut self, tid: usize) -> Result<(), String> {
        let pending = self.threads[tid].pending;
        if pending != 0 {
            return Err(format!(
                "M48: a signal is pending on thread {tid} (set {pending:#x}; bit n is signal n+1, \
                 masked or not), and this wake would write its reply over the blocked context. \
                 Measure the kernel's EINTR answer for a thread blocked in kevent or psynch_cvwait, \
                 as blockedctx.rs does for __ulock_wait, before modelling it."));
        }
        assert!(matches!(self.threads[tid].state, ThreadState::Blocked(BlockReason::Kevent { .. })),
            "M48: wake of thread {tid}, which is {:?}, not blocked in a timed-wait primitive",
            self.threads[tid].state);
        self.threads[tid].state = ThreadState::Runnable;
        Ok(())
    }

    /// M48 §3d: every blocked thread whose deadline `now` has reached, in deadline order with ties
    /// by thread index (R7). Pure: it wakes nobody.
    pub fn due_waiters(&self, now: u64) -> Vec<usize> {
        let mut due: Vec<(u64, usize)> = self.threads.iter().enumerate()
            .filter_map(|(tid, t)| match t.state {
                ThreadState::Blocked(r) => r.deadline().filter(|&d| d <= now).map(|d| (d, tid)),
                _ => None,
            })
            .collect();
        due.sort_unstable();
        due.into_iter().map(|(_, tid)| tid).collect()
    }

    /// M48 §3d: the earliest deadline over blocked threads, which the idle jump may land on.
    pub fn earliest_deadline(&self) -> Option<u64> {
        self.threads.iter()
            .filter_map(|t| match t.state { ThreadState::Blocked(r) => r.deadline(), _ => None })
            .min()
    }
```

Green:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --lib thread::tests --no-fail-fast -- --test-threads=1 > $L/t4-thread.log 2>&1; echo "exit=$?"; grep -a 'test result' $L/t4-thread.log
```

Expected: exit 0, with 12 thread tests passing (10 at plan time, plus these 2).

- [ ] **Step 4: `gkq.rs`'s unit tests, red first.** Add `pub mod gkq;` to `crates/retrace-box/src/lib.rs` after `pub mod kq;`. Create `crates/retrace-box/src/gkq.rs` holding only this test module for now; Step 5 writes the module above it:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use retrace_arch::{EVFILT_TIMER, EV_RECEIPT};

    const KQ: u64 = 3;
    const ID: u64 = 0x1e7e_7711;

    fn ch(ident: u64, filter: i16, flags: u16, fflags: u32, data: i64, udata: u64) -> Kevent {
        Kevent { ident, filter, flags, fflags, data, udata }
    }
    fn user(flags: u16, fflags: u32) -> Kevent { ch(ID, EVFILT_USER, flags, fflags, 0, 0) }
    fn kq() -> GuestKqueues {
        let mut g = GuestKqueues::default();
        g.create(KQ).unwrap();
        g
    }

    /// `kern_event.c:knote_fdclose`: a close drops the descriptor's READ and WRITE knotes in every
    /// kqueue, and a kqueue's own close drops its table. An `EVFILT_USER` knote names no descriptor
    /// and stays. Closing a kqueue a thread is blocked on is refused, because the kernel's answer
    /// to that thread is unmeasured.
    #[test]
    fn a_close_drops_the_descriptors_knotes_and_a_waited_kqueue_is_refused() {
        let mut g = kq();
        g.create(4).unwrap();
        g.apply(KQ, &[ch(9, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0), user(EV_ADD, 0)]).unwrap();
        g.apply(4, &[ch(9, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0)]).unwrap();
        g.close(9).unwrap();
        assert!(g.knote(KQ, 9, EVFILT_READ).is_none() && g.knote(4, 9, EVFILT_READ).is_none(),
            "every kqueue loses fd 9's knote");
        assert!(g.knote(KQ, ID, EVFILT_USER).is_some(), "an EVFILT_USER knote names no descriptor and stays");
        g.set_waiter(4, Waiter { tid: 1, events: 0x1000, nevents: 1 });
        let e = g.close(4).unwrap_err();
        assert!(e.starts_with("M48: kevent on kq 4: closed while thread 1 is blocked on it"), "{e}");
        assert!(g.is_kqueue(4), "the refused close changed nothing");
        g.close(KQ).unwrap();
        assert!(!g.is_kqueue(KQ) && g.knote(KQ, ID, EVFILT_USER).is_none(), "a kqueue's close drops its table");
    }

    /// K4: the kernel answers these with an ENOENT `EV_ERROR` event, which the model never makes.
    /// A refused call applies nothing, even its admitted earlier changes.
    #[test]
    fn a_change_to_an_unregistered_knote_is_refused_and_applies_nothing() {
        let mut g = kq();
        let e = g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap_err();
        assert!(e.starts_with("M48: kevent change 0: ") && e.contains("not registered and the change has no EV_ADD"), "{e}");
        let e = g.apply(KQ, &[user(EV_DELETE, 0)]).unwrap_err();
        assert!(e.contains("EV_DELETE of a knote that is not registered"), "{e}");
        let e = g.apply(KQ, &[user(EV_ADD, 0), ch(9, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)]).unwrap_err();
        assert!(e.starts_with("M48: kevent change 1: "), "{e}");
        assert!(g.knote(KQ, ID, EVFILT_USER).is_none(), "the refused call's first change was not applied");
    }

    /// F8: a trigger activates; two before a scan are one event; `EV_CLEAR` deactivates at delivery.
    #[test]
    fn two_triggers_before_a_scan_are_one_event_and_ev_clear_resets_it() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_CLEAR, 0)]).unwrap();
        assert!(!g.has_events(KQ), "an add without a trigger is not active");
        g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap();
        g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap();
        assert_eq!(g.take_events(KQ, 16).unwrap().len(), 1, "two triggers before a scan are one event");
        assert!(!g.has_events(KQ));
        g.apply(KQ, &[user(0, NOTE_TRIGGER)]).unwrap();
        assert!(g.has_events(KQ), "a trigger after the reset raises a new event");
    }

    /// F8: a knote with neither `EV_CLEAR` nor `EV_ONESHOT` is re-activated after its delivery.
    #[test]
    fn a_knote_with_neither_clear_nor_oneshot_stays_active_after_delivery() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD, 0), user(0, NOTE_TRIGGER)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(ID, EVFILT_USER, EV_ADD, 0, 0, 0)]);
        assert_eq!(g.take_events(KQ, 1).unwrap().len(), 1, "level: delivered again on the next scan");
    }

    /// F8: `EV_ONESHOT` drops the knote at its delivery.
    #[test]
    fn ev_oneshot_drops_the_knote_at_delivery() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_ONESHOT, 0), user(0, NOTE_TRIGGER)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(ID, EVFILT_USER, EV_ADD | EV_ONESHOT, 0, 0, 0)]);
        assert!(g.knote(KQ, ID, EVFILT_USER).is_none(), "dropped");
        assert!(g.take_events(KQ, 1).unwrap().is_empty());
    }

    /// F8 (`filt_usertouch`) and K9: the `NOTE_FF*` operations act on `kn_sfflags`, a touch sets
    /// `kn_sdata` and replaces `udata`, and a delivery returns the creating flags.
    #[test]
    fn the_note_ff_operations_apply_to_the_saved_fflags_and_a_touch_sets_data_and_udata() {
        let mut g = kq();
        g.apply(KQ, &[ch(ID, EVFILT_USER, EV_ADD | EV_CLEAR, 0b1100, 0, 0x1234)]).unwrap();
        g.apply(KQ, &[ch(ID, EVFILT_USER, 0, NOTE_FFOR | 0b0011, 0, 0x1234)]).unwrap();
        assert_eq!(g.knote(KQ, ID, EVFILT_USER).unwrap().sfflags, 0b1111, "NOTE_FFOR");
        g.apply(KQ, &[ch(ID, EVFILT_USER, 0, NOTE_FFAND | 0b0110, 0, 0x1234)]).unwrap();
        assert_eq!(g.knote(KQ, ID, EVFILT_USER).unwrap().sfflags, 0b0110, "NOTE_FFAND");
        g.apply(KQ, &[ch(ID, EVFILT_USER, 0, NOTE_TRIGGER | NOTE_FFCOPY | 5, 9, 0x5678)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(ID, EVFILT_USER, EV_ADD | EV_CLEAR, 5, 9, 0x5678)],
            "fflags = kn_sfflags after NOTE_FFCOPY, data = kn_sdata, udata = the trigger's");
    }

    /// `EV_DISABLE` holds an active knote back, `EV_ENABLE` releases it, `EV_DELETE` removes it.
    #[test]
    fn a_disabled_knote_is_held_until_enabled_and_ev_delete_removes_it() {
        let mut g = kq();
        g.apply(KQ, &[user(EV_ADD | EV_CLEAR | EV_DISABLE, 0), user(0, NOTE_TRIGGER)]).unwrap();
        assert!(!g.has_events(KQ), "active but disabled");
        g.apply(KQ, &[user(EV_ENABLE, 0)]).unwrap();
        assert!(g.has_events(KQ), "enabled, and still active");
        g.apply(KQ, &[user(EV_DELETE, 0)]).unwrap();
        assert!(g.knote(KQ, ID, EVFILT_USER).is_none() && !g.has_events(KQ));
    }

    /// Delivery is in activation order (`kern_event.c:knote_enqueue` appends to the active queue's
    /// tail), not in the table's ident order, and a knote past `max` stays active for the next scan.
    #[test]
    fn events_are_delivered_in_activation_order_up_to_max() {
        let mut g = kq();
        g.apply(KQ, &[ch(1, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0), ch(2, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0)]).unwrap();
        g.apply(KQ, &[ch(2, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)]).unwrap();
        g.apply(KQ, &[ch(1, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)]).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap()[0].ident, 2, "ident 2 was activated first");
        let rest = g.take_events(KQ, 16).unwrap();
        assert_eq!(rest.iter().map(|e| e.ident).collect::<Vec<_>>(), vec![1], "the knote past max was kept");
    }

    /// F9 and R4: a read end is ready from one byte with `data` the count; a write end while at
    /// least `PIPE_BUF` of the buffer is free, with `data` the room. Both are level-triggered, so
    /// a delivered one re-queues at the tail of the order.
    #[test]
    fn a_pipe_read_end_is_ready_from_one_byte_and_the_write_end_while_room_is_left() {
        let mut g = kq();
        g.pipe(5, 6).unwrap();
        g.apply(KQ, &[ch(5, EVFILT_READ, EV_ADD, 0, 0, 0xabc), ch(6, EVFILT_WRITE, EV_ADD, 0, 0, 0)]).unwrap();
        let cap = PIPE_CAPACITY as i64;
        assert_eq!(g.take_events(KQ, 16).unwrap(), vec![ch(6, EVFILT_WRITE, EV_ADD, 0, cap, 0)],
            "an empty pipe: writable with the whole buffer, not readable");
        g.note_write(6, 5).unwrap();
        assert_eq!(g.take_events(KQ, 16).unwrap(),
            vec![ch(6, EVFILT_WRITE, EV_ADD, 0, cap - 5, 0), ch(5, EVFILT_READ, EV_ADD, 0, 5, 0xabc)],
            "the write knote re-queued at its delivery; the read knote joined behind it at the write");
        g.note_read(5, 5).unwrap();
        assert_eq!(g.pipe_count(5), Some(0));
        let filters = |g: &mut GuestKqueues| g.take_events(KQ, 16).unwrap().iter().map(|e| e.filter).collect::<Vec<_>>();
        assert_eq!(filters(&mut g), vec![EVFILT_WRITE], "drained: not readable");
        g.note_write(6, PIPE_CAPACITY - (PIPE_BUF - 1)).unwrap();
        assert_eq!(filters(&mut g), vec![EVFILT_READ], "PIPE_BUF - 1 bytes of room: not writable");
    }

    /// F9: the last writer's close is `EV_EOF` to the reader (a `dup`'d descriptor keeps the end
    /// open), and the reader's close is `EV_EOF` to a read filter on the write end (t0 M3).
    #[test]
    fn a_closed_writer_reports_eof_to_the_reader_and_a_closed_reader_to_the_writer() {
        let mut g = kq();
        g.pipe(5, 6).unwrap();
        g.dup(6, 7).unwrap();
        g.apply(KQ, &[ch(5, EVFILT_READ, EV_ADD, 0, 0, 0)]).unwrap();
        g.close(6).unwrap();
        assert!(!g.has_events(KQ), "fd 7 still holds the write end open");
        g.close(7).unwrap();
        assert_eq!(g.take_events(KQ, 1).unwrap(), vec![ch(5, EVFILT_READ, EV_ADD | EV_EOF, 0, 0, 0)],
            "the last writer gone: EV_EOF, data the count");
        let mut g = kq();
        g.pipe(5, 6).unwrap();
        g.apply(KQ, &[ch(6, EVFILT_READ, EV_ADD, 0, 0, 0)]).unwrap();
        assert!(!g.has_events(KQ), "a write end's read filter waits for EOF (t0 M3: `read on a pipe write end, 1 ns`)");
        g.close(5).unwrap();
        let e = g.take_events(KQ, 1).unwrap();
        assert_eq!((e[0].ident, e[0].flags & EV_EOF, e[0].data), (6, EV_EOF, 0), "t0 M3: `read on the write end, reader closed`");
    }

    /// R5 and K3: every unmodelled filter, flag, descriptor or kqueue `dup` is refused by value,
    /// naming it. libuv's `uv__stream_try_select` probe on a descriptor that is not a guest pipe is
    /// the one external shape admitted, and it is never ready.
    #[test]
    fn an_unmodelled_filter_flag_descriptor_or_kqueue_dup_is_refused_by_value() {
        let mut g = kq();
        for (c, why) in [
            (ch(1, EVFILT_TIMER, EV_ADD, 0, 0, 0), "filter -7 is not modelled"),
            (ch(1, EVFILT_USER, EV_ADD | EV_RECEIPT, 0, 0, 0), "flags 0x40 are not modelled"),
            (ch(1, EVFILT_USER, EV_ADD, NOTE_TRIGGER, 0, 0), "an EV_ADD whose fflags carry NOTE_TRIGGER"),
            (ch(1, EVFILT_READ, EV_ADD, 0, 0, 0), "not a guest pipe end"),
            (ch(KQ, EVFILT_READ, EV_ADD, 0, 0, 0), "filter -1 on Kqueue is not modelled"),
        ] {
            let e = g.apply(KQ, &[c]).unwrap_err();
            assert!(e.starts_with("M48: kevent ") && e.contains(why), "{why}: {e}");
        }
        g.apply(KQ, &[ch(1, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0)]).unwrap();
        assert!(!g.has_events(KQ), "§11b item 7: registered, never ready");
        let e = g.dup(KQ, 9).unwrap_err();
        assert!(e.starts_with("M48: kevent on fd 3: a dup of a guest kqueue"), "{e}");
    }
}
```

Run it red; nothing above the module exists yet:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --lib gkq::tests --no-fail-fast -- --test-threads=1 > $L/t4-gkq-red.log 2>&1; echo "exit=$?"; grep -a -E 'error\[' $L/t4-gkq-red.log | sort | uniq -c | head
```

Expected: exit 101, with `cannot find type `GuestKqueues`` and `cannot find type `Kevent`` among the errors.

- [ ] **Step 5: `gkq.rs`.** Write the module above the test module in `crates/retrace-box/src/gkq.rs`. `PIPE_CAPACITY` and `ADD_FLAGS_RETURNED` carry their expected values. If the Step 1 addendum pinned other values, write those instead.

```rust
//! M48 §3c (K1): the guest's own kqueues, and the byte counts of the guest's pipes that their read
//! and write filters answer from (R4). Pure data with no `Box_` access, the `kq.rs` pattern:
//! `Box_` owns one (`Box_::gkq`) and carries it through every rebuild path in `BoxState`, because
//! a mid-run capture cannot re-derive it.
//!
//! Natively, a change the kernel rejects becomes an `EV_ERROR` event. The model never makes one: it
//! refuses every such change by value, naming its index and fields (R5, K4). So no change produces
//! an output, and a call scans exactly when it has an event list (F7).

use std::collections::BTreeMap;

use retrace_arch::{Kevent, EVFILT_READ, EVFILT_USER, EVFILT_WRITE, EV_ADD, EV_CLEAR, EV_DELETE,
                   EV_ENABLE, EV_EOF, EV_ONESHOT, EV_SYSFLAGS, NOTE_FFAND, NOTE_FFCOPY,
                   NOTE_FFCTRLMASK, NOTE_FFLAGSMASK, NOTE_FFOR, NOTE_TRIGGER};

/// `EV_DISABLE` (SDK `sys/event.h`). Task 2's constants stop short of it.
pub const EV_DISABLE: u16 = 0x0008;
/// `sizeof(struct kevent)` (SDK `sys/event.h`).
pub const KEVENT_BYTES: usize = 32;
/// A guest pipe's buffer: `EVFILT_WRITE`'s `data` on an empty pipe (t0 M3, `write-ready on an
/// empty pipe`; `PIPE_SIZE` in the SDK, F9). The kernel grows a buffer under a large write; the
/// model does not, and refuses a watched pipe that passes this (K1).
pub const PIPE_CAPACITY: u64 = 16384;
/// `PIPE_BUF` (SDK `sys/syslimits.h`): a write end is ready while this much room is left (F9).
pub const PIPE_BUF: u64 = 512;
/// The bits of a knote's creating flags a delivery hands back (t0 M3, `read on the write end,
/// reader closed`, whose knote was added `EV_ADD|EV_ENABLE`). Every measured delivery returns the
/// add's flags verbatim (`detect`: `0x21`).
pub const ADD_FLAGS_RETURNED: u16 = !EV_SYSFLAGS;

/// What a guest descriptor is to the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FdKind {
    /// A kqueue the guest's `kqueue()` returned.
    Kqueue,
    /// The read end of guest pipe `id`.
    PipeRead(u64),
    /// The write end of guest pipe `id`.
    PipeWrite(u64),
    /// Anything else: a file, a socket, a tty, or a descriptor the guest inherited (its 0, 1, 2).
    Other,
}

/// One knote, keyed in its kqueue by `(ident, filter)`, the kernel's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Knote {
    /// The creating `EV_ADD`'s flags, less `EV_SYSFLAGS`. A touch never changes them (K9).
    pub flags: u16,
    /// `kn_sfflags`: what an `EVFILT_USER` delivery returns as `fflags` (F8).
    pub sfflags: u32,
    /// `kn_sdata`: what an `EVFILT_USER` delivery returns as `data` (F8).
    pub sdata: i64,
    pub udata: u64,
    pub enabled: bool,
    /// `Some(n)` while active: `n` is its place in the activation order, which is delivery order.
    pub active: Option<u64>,
    /// For `EVFILT_READ`/`EVFILT_WRITE`: what the descriptor was at `EV_ADD`.
    pub watch: Option<FdKind>,
}

/// The thread blocked in `kevent` on a kqueue, and where its events go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waiter {
    pub tid: usize,
    /// The guest VA of its event list.
    pub events: u64,
    pub nevents: usize,
}

/// One guest kqueue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kqueue {
    knotes: BTreeMap<(u64, i16), Knote>,
    /// At most one (K2).
    waiter: Option<Waiter>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pipe {
    /// Unread bytes. `None` once a read returned more than the count, so the model lost it.
    count: Option<u64>,
    /// Open guest descriptors on each end. A `dup` adds one, and the end closes at zero.
    readers: u32,
    writers: u32,
}

/// The guest's pipes, by descriptor (R4). The counts move only by the calls' own returns:
/// forwarded on record, recorded on replay (`Box_::note_fd_effects`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pipes {
    /// Guest fd -> (pipe id, is the write end).
    ends: BTreeMap<u64, (u64, bool)>,
    pipes: BTreeMap<u64, Pipe>,
    next: u64,
}

impl Pipes {
    fn kind_of(&self, fd: u64) -> Option<FdKind> {
        self.ends.get(&fd).map(|&(id, w)| if w { FdKind::PipeWrite(id) } else { FdKind::PipeRead(id) })
    }

    fn bind(&mut self, fd: u64, id: u64, write: bool) {
        self.ends.insert(fd, (id, write));
        let p = self.pipes.get_mut(&id).expect("a bound end's pipe exists");
        if write { p.writers += 1 } else { p.readers += 1 }
    }

    /// Forget `fd`. Returns whether it was a pipe end. A pipe with no descriptor left goes.
    fn unbind(&mut self, fd: u64) -> bool {
        let Some((id, write)) = self.ends.remove(&fd) else { return false };
        let p = self.pipes.get_mut(&id).expect("a bound end's pipe exists");
        if write { p.writers -= 1 } else { p.readers -= 1 }
        if p.readers == 0 && p.writers == 0 { self.pipes.remove(&id); }
        true
    }

    /// Is `filter` on descriptor `fd`, which `watch` describes, ready, and with what `EV_EOF` bit
    /// and `data`? `sys_pipe.c:filt_piperead_common` and `:filt_pipewrite_common` (F9).
    pub fn ready(&self, fd: u64, watch: FdKind, filter: i16) -> Result<Option<(u16, i64)>, String> {
        let lost = || format!(
            "M48: pipe {fd}: the byte count is unknown: a read returned more bytes than the model \
             counted, so a write reached the pipe by a path note_fd_effects does not see");
        match (watch, filter) {
            (FdKind::PipeRead(id), EVFILT_READ) => {
                let p = self.pipes[&id];
                let eof = p.writers == 0;
                let count = p.count.ok_or_else(lost)?;
                Ok((count >= 1 || eof).then_some((if eof { EV_EOF } else { 0 }, count as i64)))
            }
            // Each end has its own buffer and nothing writes into the write end's, so its read
            // filter fires only at EOF (F9).
            (FdKind::PipeWrite(id), EVFILT_READ) => Ok((self.pipes[&id].readers == 0).then_some((EV_EOF, 0))),
            (FdKind::PipeWrite(id), EVFILT_WRITE) => {
                let p = self.pipes[&id];
                if p.readers == 0 { return Ok(Some((EV_EOF, 0))); }
                let count = p.count.ok_or_else(lost)?;
                if count > PIPE_CAPACITY {
                    return Err(format!(
                        "M48: pipe {fd}: {count} unread bytes exceed the measured {PIPE_CAPACITY}-byte \
                         buffer: the kernel grows a pipe's buffer, which the model does not (t0 M3)"));
                }
                let room = PIPE_CAPACITY - count;
                Ok((room >= PIPE_BUF).then_some((0, room as i64)))
            }
            // K3: libuv's uv__stream_try_select probe, never ready.
            (FdKind::Other, _) => Ok(None),
            (w, f) => unreachable!("M48: apply admits no filter {f} on {w:?}"),
        }
    }
}

/// Every guest kqueue, by its guest fd, and the guest's pipes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestKqueues {
    kqs: BTreeMap<u64, Kqueue>,
    pipes: Pipes,
    /// The activation counter, one for every kqueue, so the order is total.
    seq: u64,
}

impl GuestKqueues {
    pub fn is_kqueue(&self, fd: u64) -> bool { self.kqs.contains_key(&fd) }

    pub fn kind_of(&self, fd: u64) -> FdKind {
        if self.is_kqueue(fd) { FdKind::Kqueue } else { self.pipes.kind_of(fd).unwrap_or(FdKind::Other) }
    }

    /// A forwarded `kqueue()` returned guest fd `fd`: an empty table for it (M48 §3c).
    pub fn create(&mut self, fd: u64) -> Result<(), String> {
        let k = self.kind_of(fd);
        if k != FdKind::Other {
            return Err(format!("M48: kevent on fd {fd}: kqueue() returned fd {fd}, which the model \
                                still holds as {k:?}: a close it never saw"));
        }
        self.kqs.insert(fd, Kqueue::default());
        Ok(())
    }

    /// A forwarded `pipe()` returned the guest pair `(r, w)` (M38's `Ret::FdPair`), empty.
    pub fn pipe(&mut self, r: u64, w: u64) -> Result<(), String> {
        for fd in [r, w] {
            let k = self.kind_of(fd);
            if k != FdKind::Other {
                return Err(format!("M48: pipe {fd}: pipe() returned fd {fd}, which the model still \
                                    holds as {k:?}: a close it never saw"));
            }
        }
        let id = self.pipes.next;
        self.pipes.next += 1;
        self.pipes.pipes.insert(id, Pipe { count: Some(0), readers: 0, writers: 0 });
        self.pipes.bind(r, id, false);
        self.pipes.bind(w, id, true);
        Ok(())
    }

    /// `fd` closed. A kqueue's table goes with it; every knote on `fd` leaves every kqueue
    /// (`kern_event.c:knote_fdclose`); and a pipe end loses a descriptor. The close of a kqueue a
    /// thread is blocked on is refused, because the kernel's answer to that thread is unmeasured.
    pub fn close(&mut self, fd: u64) -> Result<(), String> {
        if let Some(w) = self.kqs.get(&fd).and_then(|k| k.waiter) {
            return Err(format!("M48: kevent on kq {fd}: closed while thread {} is blocked on it; the \
                                kernel's answer to the waiter is unmeasured", w.tid));
        }
        self.kqs.remove(&fd);
        for k in self.kqs.values_mut() {
            k.knotes.retain(|&(ident, filter), _| !(ident == fd && matches!(filter, EVFILT_READ | EVFILT_WRITE)));
        }
        if self.pipes.unbind(fd) { self.refresh()?; }
        Ok(())
    }

    /// `dup`, `dup2` or `F_DUPFD` made `new` a copy of `old`. Closing `new` first is `dup2`'s
    /// implicit close. A kqueue is refused (M48 §3c: the kernel's kqueue is per open file, and
    /// sharing one is unmeasured). A pipe end gains a descriptor.
    pub fn dup(&mut self, old: u64, new: u64) -> Result<(), String> {
        if old == new { return Ok(()); }
        if self.is_kqueue(old) {
            return Err(format!("M48: kevent on fd {old}: a dup of a guest kqueue to fd {new}: the \
                                kernel's kqueue is per open file and sharing one is unmeasured (M48 §3c)"));
        }
        self.close(new)?;
        if let Some(&(id, write)) = self.pipes.ends.get(&old) { self.pipes.bind(new, id, write); }
        Ok(())
    }

    /// A read from `fd` returned `n` bytes. Only a pipe's read end counts.
    pub fn note_read(&mut self, fd: u64, n: u64) -> Result<(), String> {
        let Some(&(id, false)) = self.pipes.ends.get(&fd) else { return Ok(()) };
        let p = self.pipes.pipes.get_mut(&id).expect("a bound end's pipe exists");
        p.count = p.count.and_then(|c| c.checked_sub(n));
        self.refresh()
    }

    /// A write to `fd` returned `n` bytes. Only a pipe's write end counts.
    pub fn note_write(&mut self, fd: u64, n: u64) -> Result<(), String> {
        let Some(&(id, true)) = self.pipes.ends.get(&fd) else { return Ok(()) };
        let p = self.pipes.pipes.get_mut(&id).expect("a bound end's pipe exists");
        p.count = p.count.map(|c| c + n);
        self.refresh()
    }

    pub fn pipe_count(&self, fd: u64) -> Option<u64> {
        self.pipes.ends.get(&fd).and_then(|(id, _)| self.pipes.pipes[id].count)
    }

    /// Re-evaluate every pipe knote: one that became ready joins the activation order at its tail,
    /// and one that stopped being ready leaves it (level-triggered).
    fn refresh(&mut self) -> Result<(), String> {
        let Self { kqs, pipes, seq } = self;
        for k in kqs.values_mut() {
            for (&(ident, filter), n) in k.knotes.iter_mut() {
                let Some(watch) = n.watch else { continue };
                match pipes.ready(ident, watch, filter)? {
                    Some(_) if n.active.is_none() => { *seq += 1; n.active = Some(*seq); }
                    Some(_) => {}
                    None => n.active = None,
                }
            }
        }
        Ok(())
    }

    /// Apply `changes` to kqueue `kq`, in list order (`kern_event.c:kevent_register`, F8). All or
    /// nothing: they go to a copy that replaces the table only if every change is admitted.
    pub fn apply(&mut self, kq: u64, changes: &[Kevent]) -> Result<(), String> {
        let mut k = self.kqs.get(&kq).cloned()
            .ok_or_else(|| format!("M48: kevent on fd {kq}, which is not a guest kqueue"))?;
        let mut seq = self.seq;
        for (i, c) in changes.iter().enumerate() {
            self.apply_one(&mut k, &mut seq, i, c)?;
        }
        self.kqs.insert(kq, k);
        self.seq = seq;
        Ok(())
    }

    fn apply_one(&self, k: &mut Kqueue, seq: &mut u64, i: usize, c: &Kevent) -> Result<(), String> {
        // F8: input flags lose EV_SYSFLAGS at copyin.
        let flags = c.flags & !EV_SYSFLAGS;
        let at = format!("M48: kevent change {i}: (ident {:#x}, filter {}, flags {flags:#x}, fflags {:#x}, data {:#x})",
                         c.ident, c.filter, c.fflags, c.data);
        const MODELLED: u16 = EV_ADD | EV_DELETE | EV_ENABLE | EV_DISABLE | EV_CLEAR | EV_ONESHOT;
        if flags & !MODELLED != 0 {
            return Err(format!("{at}: flags {:#x} are not modelled (EV_RECEIPT, EV_DISPATCH, \
                                EV_UDATA_SPECIFIC and the rest are unmeasured)", flags & !MODELLED));
        }
        if flags & (EV_ADD | EV_DELETE) == EV_ADD | EV_DELETE || flags & (EV_ENABLE | EV_DISABLE) == EV_ENABLE | EV_DISABLE {
            return Err(format!("{at}: contradictory flags are not modelled"));
        }
        if !matches!(c.filter, EVFILT_USER | EVFILT_READ | EVFILT_WRITE) {
            return Err(format!("{at}: filter {} is not modelled: only EVFILT_USER and the pipe filters \
                                are (M48 §3c)", c.filter));
        }
        let key = (c.ident, c.filter);
        if flags & EV_DELETE != 0 {
            return match k.knotes.remove(&key) {
                Some(_) => Ok(()),
                None => Err(format!("{at}: EV_DELETE of a knote that is not registered: the kernel \
                                     answers an ENOENT EV_ERROR event, which is not modelled")),
            };
        }
        let fd_filter = c.filter != EVFILT_USER;
        if fd_filter && (c.fflags != 0 || c.data != 0) {
            return Err(format!("{at}: a pipe filter with fflags or data (NOTE_LOWAT and the rest) is not modelled"));
        }
        let Some(n) = k.knotes.get_mut(&key) else {
            if flags & EV_ADD == 0 {
                return Err(format!("{at}: the knote is not registered and the change has no EV_ADD: \
                                    the kernel answers an ENOENT EV_ERROR event, which is not modelled"));
            }
            let mut n = Knote { flags, sfflags: 0, sdata: c.data, udata: c.udata,
                                enabled: flags & EV_DISABLE == 0, active: None, watch: None };
            if fd_filter {
                if flags & (EV_CLEAR | EV_ONESHOT) != 0 {
                    return Err(format!("{at}: EV_CLEAR or EV_ONESHOT on a pipe filter is not modelled"));
                }
                let watch = self.kind_of(c.ident);
                match (watch, c.filter) {
                    (FdKind::PipeRead(_), EVFILT_READ) | (FdKind::PipeWrite(_), EVFILT_READ | EVFILT_WRITE) => {}
                    (FdKind::Other, EVFILT_READ) if flags == EV_ADD | EV_ENABLE => {}
                    (FdKind::Other, f) => return Err(format!(
                        "M48: kevent on fd {}: change {i}'s filter {f} with flags {flags:#x} asks about a \
                         descriptor that is not a guest pipe end: readiness of a file, socket, tty or \
                         inherited descriptor is not modelled (M48 §7); only uv__stream_try_select's \
                         EVFILT_READ, EV_ADD|EV_ENABLE is answered, never ready (K3)", c.ident)),
                    (w, f) => return Err(format!("{at}: filter {f} on {w:?} is not modelled")),
                }
                n.active = match self.pipes.ready(c.ident, watch, c.filter)? {
                    Some(_) => { *seq += 1; Some(*seq) }
                    None => None,
                };
                n.watch = Some(watch);
            } else {
                if c.fflags & !NOTE_FFLAGSMASK != 0 {
                    return Err(format!("{at}: an EV_ADD whose fflags carry NOTE_TRIGGER or a NOTE_FF* \
                                        operation: what the kernel then delivers as fflags is unmeasured"));
                }
                n.sfflags = c.fflags;
            }
            k.knotes.insert(key, n);
            return Ok(());
        };
        // A touch (K9).
        let kind = flags & (EV_CLEAR | EV_ONESHOT);
        if kind != 0 && kind != n.flags & (EV_CLEAR | EV_ONESHOT) {
            return Err(format!("{at}: a touch that changes EV_CLEAR or EV_ONESHOT on a registered knote \
                                is not modelled"));
        }
        n.udata = c.udata;
        if flags & EV_ENABLE != 0 { n.enabled = true; }
        if flags & EV_DISABLE != 0 { n.enabled = false; }
        if !fd_filter {
            // kern_event.c:filt_usertouch (F8).
            let ff = c.fflags & NOTE_FFLAGSMASK;
            match c.fflags & NOTE_FFCTRLMASK {
                NOTE_FFAND => n.sfflags &= ff,
                NOTE_FFOR => n.sfflags |= ff,
                NOTE_FFCOPY => n.sfflags = ff,
                _ => {} // NOTE_FFNOP
            }
            n.sdata = c.data;
            if c.fflags & NOTE_TRIGGER != 0 && n.active.is_none() {
                *seq += 1;
                n.active = Some(*seq);
            }
        }
        Ok(())
    }

    /// Does `kq` hold an event a scan would deliver?
    pub fn has_events(&self, kq: u64) -> bool {
        self.kqs.get(&kq).is_some_and(|k| k.knotes.values().any(|n| n.enabled && n.active.is_some()))
    }

    /// Deliver up to `max` active, enabled knotes of `kq` in activation order
    /// (`kern_event.c:kqueue_process`, F8): `EV_ONESHOT` drops a knote, `EV_CLEAR` deactivates it,
    /// and any other is re-activated at the tail. A pipe knote's `data` and `EV_EOF` are read now.
    pub fn take_events(&mut self, kq: u64, max: usize) -> Result<Vec<Kevent>, String> {
        let Self { kqs, pipes, seq } = self;
        let k = kqs.get_mut(&kq).expect("take_events on a guest kqueue");
        let mut due: Vec<((u64, i16), u64)> = k.knotes.iter()
            .filter_map(|(&key, n)| n.active.filter(|_| n.enabled).map(|s| (key, s)))
            .collect();
        due.sort_by_key(|&(_, s)| s);
        due.truncate(max);
        let mut out = Vec::with_capacity(due.len());
        for (key, _) in due {
            let n = k.knotes.get_mut(&key).expect("collected from this table just above");
            let (eof, data, fflags) = match n.watch {
                None => (0, n.sdata, n.sfflags),
                Some(w) => {
                    let (eof, data) = pipes.ready(key.0, w, key.1)?
                        .expect("an active pipe knote is ready: refresh keeps the two in step");
                    (eof, data, 0)
                }
            };
            out.push(Kevent { ident: key.0, filter: key.1, flags: (n.flags & ADD_FLAGS_RETURNED) | eof,
                              fflags, data, udata: n.udata });
            if n.flags & EV_ONESHOT != 0 {
                k.knotes.remove(&key);
            } else if n.flags & EV_CLEAR != 0 {
                n.active = None;
            } else {
                *seq += 1;
                n.active = Some(*seq);
            }
        }
        Ok(out)
    }

    pub fn waiter(&self, kq: u64) -> Option<Waiter> { self.kqs.get(&kq).and_then(|k| k.waiter) }

    /// Record `w` as `kq`'s waiter. `Box_::guest_kevent` refuses a second one first (K2).
    pub fn set_waiter(&mut self, kq: u64, w: Waiter) {
        let k = self.kqs.get_mut(&kq).expect("set_waiter on a guest kqueue");
        assert!(k.waiter.is_none(), "M48: a second waiter on kq {kq}; guest_kevent refuses it first (K2)");
        k.waiter = Some(w);
    }

    pub fn take_waiter(&mut self, kq: u64) -> Option<Waiter> {
        self.kqs.get_mut(&kq).and_then(|k| k.waiter.take())
    }

    /// Every kqueue whose waiter now has an event, in kqueue fd order.
    pub fn ready_waiters(&self) -> Vec<u64> {
        self.kqs.keys().copied().filter(|&kq| self.waiter(kq).is_some() && self.has_events(kq)).collect()
    }

    pub fn knote(&self, kq: u64, ident: u64, filter: i16) -> Option<&Knote> {
        self.kqs.get(&kq).and_then(|k| k.knotes.get(&(ident, filter)))
    }
}
```

Green, then clippy on the crate:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --lib gkq::tests --no-fail-fast -- --test-threads=1 > $L/t4-gkq.log 2>&1; echo "exit=$?"; grep -a 'test result' $L/t4-gkq.log
cargo clippy -p retrace-box --all-targets -- -D warnings > $L/t4-gkq-clippy.log 2>&1; echo "clippy exit=$?"
```

Expected: 11 passed and clippy exit 0. Every item is `pub` in a `pub mod` of a library, or used inside the module, so none is dead code before the box calls it.

- [ ] **Step 6: The box-level tests, red first.** Create `crates/retrace-box/tests/gkq.rs`:

```rust
//! M48 §3c–§3d, box level: guest kqueues and the deadline queue, with the kernel's side driven by
//! hand as `kqmanager.rs` drives M46's manager. Review Focus 1 is pinned here against guest
//! memory, which the pure `gkq.rs` never sees. Review Focus 2 is pinned here with a reply that
//! differs from the 0 a blocking landmark writes: a kevent deadline wake always answers 0, so no
//! fixture can show a reply lost to a stale saved context.
//!
//! K10: a static box has no commpage and so no guest clock, so the static tests use NULL or zero
//! timeouts only. The two deadline tests load `hello_dyn` (the `stackgrow.rs` pattern); its stage 1
//! is identity-mapped, so a `guest_mmap` IPA is also the VA `guest_kevent` reads.
use retrace_arch::{Kevent, EVFILT_READ, EVFILT_USER, EV_ADD, EV_CLEAR, NOTE_FFCOPY, NOTE_TRIGGER,
                   PSTATE_C, SYS_READ, SYS_WRITE, TIMER_IDENT_BASE};
use retrace_box::thread::{BlockReason, ThreadCtx, ThreadState};
use retrace_box::Box_;
use retrace_guest::{parse_macho, slice_arm64e, DYLD_PATH, HELLO_DYN, SPINLOOP};

const KQ: u64 = 3;
/// `kqueue` and `pipe` (SDK `sys/syscall.h`), as the generic arm hands them to the hook.
const SYS_KQUEUE: u64 = 362;
const SYS_PIPE: u64 = 42;
const RW: u64 = 3;
const ANON: u64 = 0x1002; // MAP_ANON | MAP_PRIVATE

fn tb() -> Box_ {
    Box_::load(&parse_macho(&std::fs::read(SPINLOOP).unwrap()))
}

fn dynbox() -> Box_ {
    let exe = parse_macho(&std::fs::read(HELLO_DYN).unwrap());
    let dyld = parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()));
    Box_::load_dynamic(&exe, &dyld, &["hello_dyn".to_string()])
}

/// A scratch page, and kqueue `KQ` made as a forwarded `kqueue()` makes one: a guest fd slot, then
/// the hook.
fn setup(b: &mut Box_) -> u64 {
    let base = b.guest_mmap(0, 0x4000, RW, ANON).unwrap();
    assert_eq!(b.fds_mut().alloc(), KQ);
    b.note_fd_effects(SYS_KQUEUE, [0; 8], KQ, 0, false).unwrap();
    base
}

fn kev(ident: u64, filter: i16, flags: u16, fflags: u32, data: i64, udata: u64) -> Kevent {
    Kevent { ident, filter, flags, fflags, data, udata }
}

/// `kevent(KQ, at, changes.len(), at + 0x100, nevents, timeout)`, the changes poked at `at`.
fn kevent(b: &mut Box_, at: u64, changes: &[Kevent], nevents: u64, timeout: u64) -> Result<(u64, bool), String> {
    for (i, c) in changes.iter().enumerate() { b.poke_guest(at + 32 * i as u64, &c.to_bytes()); }
    let list = if changes.is_empty() { 0 } else { at };
    b.guest_kevent([KQ, list, changes.len() as u64, at + 0x100, nevents, timeout, 0, 0])
}

fn spawn(b: &mut Box_) -> usize {
    let ctx = ThreadCtx { spsr: b.spsr(), ..ThreadCtx::zeroed() };
    b.threads_mut().spawn(ctx, (0, 0))
}

fn timespec(b: &mut Box_, at: u64, sec: u64, nsec: u64) -> u64 {
    b.poke_guest(at, &sec.to_le_bytes());
    b.poke_guest(at + 8, &nsec.to_le_bytes());
    at
}

fn synthetic_tsc(b: &Box_) -> u64 {
    let s = b.dbg_internal_state();
    let v = s.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next()).unwrap();
    u64::from_str_radix(v, 16).unwrap()
}

/// Review Focus 2. `switch_to_thread` returns early for the current thread, so a reply written only
/// to its saved context would never load. The reply here, 0x2a with the carry set, differs from
/// what the blocking landmark wrote, so a lost one shows.
#[test]
fn a_wake_of_the_current_thread_writes_the_vcpu() {
    let mut b = tb();
    let base = setup(&mut b);
    b.threads_mut().block(BlockReason::Kevent { kq: KQ, deadline: None });
    b.set_x0_err_and_return(0, false);
    let stale = b.threads().ctx_of(0).regs.x[0];
    let ev = kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234).to_bytes();
    b.deliver_wake(0, 0x2a, true, &[(base, ev.to_vec())]).unwrap();
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    assert_eq!(b.vcpu_get_x(0), 0x2a, "the reply is on the vCPU, where the current thread reads it");
    assert_ne!(b.regs_snapshot().cpsr & PSTATE_C, 0, "err sets the carry in the live CPSR");
    assert_eq!(b.threads().ctx_of(0).regs.x[0], stale, "the current thread's table entry is stale, and not where the reply goes");
    assert_eq!(b.read_bytes_for_test(base, 32), ev);
    b.schedule_after_block();
    assert_eq!((b.threads().current(), b.vcpu_get_x(0)), (0, 0x2a),
        "the pick returns the same thread and the switch returns early: the reply survives");
}

/// Review Focus 1. libuv's `uv__kqueue_runtime_detection` passes ONE buffer as the change list and
/// the event list (M47 `node.entry.txt`), so the call must read both changes before it writes an
/// event, as the kernel's copyin loop does. Native then holds one event over slot 0 and the
/// untouched trigger in slot 1 (`native/kqdetect.out`; walls.md §1 row 1).
#[test]
fn an_event_list_aliasing_the_change_list_is_read_before_it_is_written() {
    const ID: u64 = 0x1e7e_7711;
    let mut b = tb();
    let base = setup(&mut b);
    let add = kev(ID, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    let trig = kev(ID, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    b.poke_guest(base, &add.to_bytes());
    b.poke_guest(base + 32, &trig.to_bytes());
    let zero = timespec(&mut b, base + 0x200, 0, 0);
    assert_eq!(b.guest_kevent([KQ, base, 2, base, 1, zero, 0, 0]), Ok((1, false)), "native's one event");
    assert_eq!(b.read_bytes_for_test(base, 32), add.to_bytes(),
        "slot 0 holds the event: the add's flags, kn_sfflags 0, kn_sdata 0 and the trigger's udata 0 (F8)");
    assert_eq!(b.read_bytes_for_test(base + 32, 32), trig.to_bytes(), "one event is one slot: slot 1 keeps the trigger");
    assert!(!b.dbg_gkq().has_events(KQ), "the trigger was read and applied, and EV_CLEAR reset the knote at its delivery");
}

/// §3c: a `NOTE_TRIGGER` from another thread, in `uv_async_send`'s shape (one change, no event
/// list), wakes the waiter with its event (F8) at the trigger, not at a later switch. It is also
/// the other half of `deliver_wake`'s contract: a thread that is not on the vCPU gets its reply in
/// its saved context (a carry set there is cleared), and the running thread's registers are
/// untouched.
#[test]
fn a_trigger_from_another_thread_wakes_the_waiter_with_its_event() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    assert_eq!(kevent(&mut b, base, &[kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234)], 0, 0), Ok((0, false)));
    assert_eq!(kevent(&mut b, base, &[], 1, 0), Ok((0, false)), "a block records 0");
    assert_eq!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: None }));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1);
    b.threads_mut().ctx_mut(0).regs.cpsr |= PSTATE_C;
    b.vcpu_set_x(0, 0x77);
    let trig = kev(7, EVFILT_USER, 0, NOTE_TRIGGER | NOTE_FFCOPY | 5, 9, 0x5678);
    assert_eq!(kevent(&mut b, base + 0x1000, &[trig], 0, 0), Ok((0, false)));
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    let ctx = b.threads().ctx_of(0);
    assert_eq!((ctx.regs.x[0], ctx.regs.cpsr & PSTATE_C), (1, 0), "one event, carry cleared, in main's saved context");
    assert_eq!(b.vcpu_get_x(0), 0x77, "the waker's registers are untouched: its own return is the arm's to set");
    assert_eq!(b.read_bytes_for_test(base + 0x100, 32), kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 5, 9, 0x5678).to_bytes(),
        "the creating flags, kn_sfflags after NOTE_FFCOPY, kn_sdata, and the trigger's udata (F8, K9)");
    assert_eq!(b.dbg_gkq().waiter(KQ), None);
    assert!(!b.dbg_gkq().has_events(KQ), "EV_CLEAR reset the knote at the delivery");
}

/// F7: with no event list the call applies its changes and returns 0, whatever its timeout. A zero
/// timeout polls.
#[test]
fn a_call_with_no_event_list_applies_its_changes_and_never_blocks() {
    let mut b = tb();
    let base = setup(&mut b);
    let add = kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    let trig = kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    assert_eq!(kevent(&mut b, base, &[add, trig], 0, 0), Ok((0, false)));
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable, "no scan, so no block, with a NULL timeout");
    assert!(b.dbg_gkq().has_events(KQ), "the trigger is applied, waiting for a scan");
    let zero = timespec(&mut b, base + 0x200, 0, 0);
    assert_eq!(kevent(&mut b, base, &[], 1, zero), Ok((1, false)));
    assert_eq!(kevent(&mut b, base, &[], 1, zero), Ok((0, false)), "a zero timeout returns 0 without blocking");
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
}

/// K5: the kernel copies the timeout in first, so an unmapped one is EFAULT with nothing applied;
/// here the one change would be refused if it were applied.
#[test]
fn an_unmapped_timeout_answers_efault_and_applies_no_change() {
    let mut b = tb();
    let base = setup(&mut b);
    let r = kevent(&mut b, base, &[kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)], 1, 1 << 40);
    assert_eq!(r, Ok((14, true)), "EFAULT, carry set");
    assert!(b.dbg_gkq().knote(KQ, 7, EVFILT_USER).is_none());
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
}

/// Review Focus 2's path, and the header's "not special-cased" rule: node's 1 ns wait converts to
/// 0 ticks (F6), so it blocks with its deadline already reached and is woken by the deadline queue
/// in the same `schedule_after_block`, with no idle jump.
#[test]
fn a_one_nanosecond_wait_is_woken_in_the_same_settle_without_a_jump() {
    let mut b = dynbox();
    let base = setup(&mut b);
    let ts = timespec(&mut b, base + 0x200, 0, 1);
    assert_eq!(kevent(&mut b, base, &[], 1, ts), Ok((0, false)), "nothing active: it blocks and records 0");
    let ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: Some(d) }) = b.threads().state_of(0) else {
        panic!("blocked with a deadline: {:?}", b.threads().state_of(0));
    };
    b.set_x0_err_and_return(0, false);
    let before = synthetic_tsc(&b);
    b.schedule_after_block();
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable, "the deadline {d:#x} was already due");
    assert_eq!(b.threads().current(), 0);
    assert_eq!(synthetic_tsc(&b), before, "no idle jump: the deadline queue's own pass woke it");
    assert_eq!(b.dbg_gkq().waiter(KQ), None);
}

/// §3d, R7: with nothing runnable the one jump goes to the EARLIEST deadline of all, here a
/// thread's 1 ms ahead of another's 3 ms and of an M46 timer 7 ms after that. It wakes only that
/// thread. Before M48 there was no thread deadline to jump to, and the jump went to the timer.
#[test]
fn the_idle_jump_lands_on_the_earliest_thread_deadline_before_a_later_timer() {
    let mut b = dynbox();
    let base = setup(&mut b);
    assert_eq!(b.fds_mut().alloc(), KQ + 1);
    b.note_fd_effects(SYS_KQUEUE, [0; 8], KQ + 1, 0, false).unwrap();
    let t1 = spawn(&mut b);
    let ms1 = timespec(&mut b, base + 0x200, 0, 1_000_000);
    assert_eq!(kevent(&mut b, base, &[], 1, ms1), Ok((0, false)));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1, "1 ms is not reached: thread 1 runs");
    let ms3 = timespec(&mut b, base + 0x300, 0, 3_000_000);
    assert_eq!(b.guest_kevent([KQ + 1, 0, 0, base + 0x400, 1, ms3, 0, 0]), Ok((0, false)));
    let ThreadState::Blocked(BlockReason::Kevent { deadline: Some(d1), .. }) = b.threads().state_of(t1) else {
        panic!("thread 1 blocked with a deadline: {:?}", b.threads().state_of(t1));
    };
    b.dbg_kq_mut().add_timer(TIMER_IDENT_BASE, d1 + 168_000, 0, 0).unwrap();
    b.set_x0_err_and_return(0, false);
    let before = synthetic_tsc(&b);
    b.schedule_after_block();
    assert_eq!(synthetic_tsc(&b) - before, 24_000, "F6: exactly to thread 0's 1 ms deadline (24000 ticks), no further");
    assert_eq!((b.threads().current(), b.threads().state_of(0)), (0, ThreadState::Runnable));
    assert_eq!(b.threads().state_of(t1), ThreadState::Blocked(BlockReason::Kevent { kq: KQ + 1, deadline: Some(d1) }),
        "3 ms is not reached");
    assert_eq!(b.dbg_kq().armed_count(), 1, "the later timer neither fired nor was jumped to");
}

/// R4: a write's return, seen by the hook after the generic arm, makes a watched read end readable
/// and wakes its waiter with `data` the count (F9). The read's return drains it. A read past the
/// count means a write reached the pipe by a path the hook does not see, and a watched pipe whose
/// count is lost is refused by value (K1), not answered from a wrong count.
#[test]
fn a_pipe_write_wakes_a_reader_blocked_on_its_read_end() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    let (r, w) = (b.fds_mut().alloc(), b.fds_mut().alloc());
    b.note_fd_effects(SYS_PIPE, [0; 8], r, w, false).unwrap();
    assert_eq!(kevent(&mut b, base, &[kev(r, EVFILT_READ, EV_ADD, 0, 0, 0xabc)], 0, 0), Ok((0, false)));
    assert_eq!(kevent(&mut b, base, &[], 1, 0), Ok((0, false)), "an empty pipe: it blocks");
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1);
    b.note_fd_effects(SYS_WRITE, [w, base + 0x800, 5, 0, 0, 0, 0, 0], 5, 0, false).unwrap();
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    assert_eq!(b.threads().ctx_of(0).regs.x[0], 1);
    assert_eq!(b.read_bytes_for_test(base + 0x100, 32), kev(r, EVFILT_READ, EV_ADD, 0, 5, 0xabc).to_bytes());
    b.note_fd_effects(SYS_READ, [r, base + 0x800, 64, 0, 0, 0, 0, 0], 5, 0, false).unwrap();
    assert_eq!(b.dbg_gkq().pipe_count(r), Some(0));
    let e = b.note_fd_effects(SYS_READ, [r, base + 0x800, 64, 0, 0, 0, 0, 0], 1, 0, false).unwrap_err();
    assert!(e.starts_with(&format!("M48: pipe {r}: the byte count is unknown")), "{e}");
}

/// K8, box level: a trigger that would wake a thread with a signal pending is refused by value and
/// leaves it blocked.
#[test]
fn a_wake_with_a_signal_pending_is_refused_and_wakes_nobody() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    assert_eq!(kevent(&mut b, base, &[kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0)], 1, 0), Ok((0, false)));
    b.set_x0_err_and_return(0, false);
    b.threads_mut().pend(0, 30);
    b.schedule_after_block();
    assert_eq!(b.threads().current(), t1);
    let e = kevent(&mut b, base + 0x1000, &[kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0)], 0, 0).unwrap_err();
    assert!(e.starts_with("M48: a signal is pending on thread 0 (set 0x20000000"), "{e}");
    assert_eq!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: None }), "woke nobody");
}

/// Restore parity at box level: the kqueue, its knote and its waiter ride in `BoxState`, and a
/// trigger on the restored box wakes the restored waiter.
#[test]
fn a_blocked_kevent_survives_a_checkpoint() {
    let mut b = tb();
    let base = setup(&mut b);
    let t1 = spawn(&mut b);
    assert_eq!(kevent(&mut b, base, &[kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234)], 1, 0), Ok((0, false)));
    b.set_x0_err_and_return(0, false);
    b.schedule_after_block();
    let gkq = b.dbg_gkq().clone();
    let st = b.checkpoint();
    drop(b); // one VM per process
    let mut r = Box_::from_checkpoint(&st);
    assert_eq!(r.dbg_gkq(), &gkq, "the kqueue, its knote and its waiter");
    assert_eq!(r.threads().state_of(0), ThreadState::Blocked(BlockReason::Kevent { kq: KQ, deadline: None }));
    assert_eq!(r.threads().current(), t1);
    assert_eq!(kevent(&mut r, base + 0x1000, &[kev(7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0x1234)], 0, 0), Ok((0, false)));
    assert_eq!(r.threads().state_of(0), ThreadState::Runnable, "the restored waiter is woken");
    assert_eq!(r.read_bytes_for_test(base + 0x100, 32), kev(7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0x1234).to_bytes());
}
```

Run it red:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --test gkq --no-fail-fast -- --test-threads=1 > $L/t4-box-red.log 2>&1; echo "exit=$?"; grep -a -E 'error\[E0599\]' $L/t4-box-red.log | sort | uniq -c | head
```

Expected: exit 101, with E0599 for `guest_kevent`, `deliver_wake`, `note_fd_effects` and `dbg_gkq`.

- [ ] **Step 7: The box.** In `crates/retrace-box/src/lib.rs`:

**(a) The field, through every path** (the six-site pattern in Interfaces). `excl`'s doc says "Declared last.", which the new field makes false. Replace its sentence "Declared last. It holds a `Vec`, so it has Drop, but it comes after `vcpu`/`vm`, so the load-bearing vcpu-before-vm drop order is unaffected." with "It holds a `Vec`, so it has Drop, but it is declared after `vcpu`/`vm`, so the load-bearing vcpu-before-vm drop order is unaffected." Then add the field to the `Box_` struct after `excl`:

```rust
    /// M48 §3c (K1): the guest's own kqueues and its pipes' byte counts. Box state, not trace state:
    /// record and replay rebuild it from the guest's own syscalls, and every rebuild path carries it
    /// (`BoxState`). Declared after `vcpu`/`vm`, so the drop order is unaffected.
    gkq: gkq::GuestKqueues,
```

Add the field to `BoxState` after `excl`:

```rust
    // M48 §3c: carried because a mid-run capture cannot re-derive it. The kqueues were created,
    // their knotes registered and their waiters blocked behind the checkpoint. Dropped, a seek into
    // a blocked kevent would replay the trigger against no kqueue (kq_e2e's seek tests).
    pub gkq: gkq::GuestKqueues,
```

Then:
- In `checkpoint()`, add `gkq: self.gkq.clone(),` after `excl: self.excl.clone(),`.
- In `from_checkpoint`, add `gkq: state.gkq.clone(),` after `excl: state.excl.clone(),`.
- In the three literal constructors (`load_with_pac`, `load_dynamic`, `restore`), replace `kq: kq::WorkqKqueue::default(), excl: None` with `kq: kq::WorkqKqueue::default(), excl: None, gkq: gkq::GuestKqueues::default()`. The string occurs exactly three times (checked at plan time); use one `replace_all` edit. A snapshot is taken at process start, so empty is right.
- Extend `dbg_internal_state` to end with ` gkq={:?}`, passing `self.gkq` last.
- After `dbg_kq_mut`, add:

```rust
    /// Test-only (M48): the guest kqueues, for the box tests and the restore-parity checks.
    #[doc(hidden)]
    pub fn dbg_gkq(&self) -> &gkq::GuestKqueues { &self.gkq }
```

**(b) The syscall numbers the hook reads**, beside `PTHREAD_KPORT_OFF`. Neither Task 2 nor `retrace-arch` names them:

```rust
/// M48 §3c: `kqueue` and the `writev` pair (SDK `sys/syscall.h`), which `Box_::note_fd_effects`
/// reads. `writev` is forwarded (its row is `NestedSource`), so its return moves a pipe's count as
/// `write`'s does. `readv` needs no entry: its row is `NestedDest`, which the generic arm refuses
/// (`writes_via_nested_pointer`), so it never completes.
const SYS_KQUEUE: u64 = 362;
const SYS_WRITEV: u64 = 121;
const SYS_WRITEV_NOCANCEL: u64 = 412;
```

**(c) `guest_kevent`, `note_fd_effects` and `deliver_wake`**, after `guest_kevent_qos`:

```rust
    /// M48 §3c (K1): `kevent(kq, changelist, nchanges, eventlist, nevents, timeout)` (363) on a
    /// guest kqueue, **modelled, never forwarded**. Forwarded, it would act on a kqueue retrace never
    /// created, and a wait would block the recorder. The generic arm asserts against it (Task 2).
    ///
    /// In the kernel's order (`kern_event.c`, F7):
    /// 1. The timeout is copied in before the kqueue is looked up (K5).
    /// 2. The kqueue, the counts and the lists are checked. Every change is read before any event
    ///    is written (Review Focus 1: libuv's runtime detection passes one buffer as both lists).
    /// 3. The changes are applied, all or nothing (`GuestKqueues::apply`). A waiter whose kqueue
    ///    now holds an event is woken, with its reply written at the wake (`deliver_wake`).
    /// 4. With no event list, it returns 0, whatever the timeout (F7).
    /// 5. Otherwise it writes the events it finds and returns their count.
    /// 6. With none found, a zero timeout returns 0, and anything else blocks the caller. A NULL
    ///    timeout blocks with no deadline. A relative one blocks until `now_guest()` plus the
    ///    timeout in ticks (F6). The blocking landmark returns 0, and the real answer comes at the
    ///    wake.
    ///
    /// Returns `(ret, err)`. `Err` is a refusal by value (R5): the record arm panics with it, and
    /// the mirror wraps it as a divergence. The inputs are `args`, guest memory, the guest clock and
    /// box state, which record and replay hold identically, so nothing is recorded (R3).
    pub fn guest_kevent(&mut self, args: [u64; 8]) -> Result<(u64, bool), String> {
        const EFAULT: u64 = 14;
        // The longest list measured (P3: libuv's loop passes 1024). Longer is refused, not guessed.
        const MAX_LIST: i32 = 1024;
        let fail = |why: String| format!("{why}. args=[{}]", Self::fmt_args(args));
        let (kq, clist, elist, tspec) = (args[0], args[1], args[3], args[5]);
        let (nchanges, nevents) = (args[2] as u32 as i32, args[4] as u32 as i32);
        // 1. The timeout.
        let timeout = if tspec == 0 { None } else {
            let ts = self.read_va_prefix(tspec, 16);
            if ts.len() < 16 { return Ok((EFAULT, true)); }
            let sec = i64::from_le_bytes(ts[..8].try_into().unwrap());
            let nsec = i64::from_le_bytes(ts[8..].try_into().unwrap());
            if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
                return Err(fail(format!("M48: kevent timeout {{{sec}, {nsec}}} is not a valid timespec: \
                                         the kernel answers EINVAL, which is not modelled")));
            }
            Some((sec as u64, nsec as u64))
        };
        // 2. The kqueue, the counts and the lists.
        if !self.gkq.is_kqueue(kq) {
            return Err(fail(format!("M48: kevent on fd {kq}, which is not a guest kqueue: the kernel \
                                     answers EBADF, which is not modelled")));
        }
        if !(0..=MAX_LIST).contains(&nchanges) || !(0..=MAX_LIST).contains(&nevents) {
            return Err(fail(format!("M48: kevent nchanges {nchanges} or nevents {nevents} is outside \
                                     0..={MAX_LIST}: a negative count is EINVAL, and a longer list is \
                                     unmeasured (P3)")));
        }
        let (nchanges, nevents) = (nchanges as usize, nevents as usize);
        let need = nchanges * gkq::KEVENT_BYTES;
        let bytes = self.read_va_prefix(clist, need);
        if nchanges > 0 && bytes.is_empty() { return Ok((EFAULT, true)); }
        if bytes.len() < need {
            return Err(fail(format!("M48: kevent change list maps {} of {need} bytes: the kernel applies \
                                     the mapped prefix before its EFAULT, which is not modelled (K5)", bytes.len())));
        }
        let changes: Vec<retrace_arch::Kevent> = bytes.chunks_exact(gkq::KEVENT_BYTES)
            .map(|c| retrace_arch::Kevent::from_bytes(c.try_into().expect("chunks_exact")))
            .collect();
        for (i, c) in changes.iter().enumerate() {
            if matches!(c.filter, retrace_arch::EVFILT_READ | retrace_arch::EVFILT_WRITE) && !self.fds.is_open(c.ident) {
                return Err(fail(format!("M48: kevent change {i}: filter {} on fd {}, which is not open: the \
                                         kernel answers an EBADF EV_ERROR event, which is not modelled (K4)",
                                        c.filter, c.ident)));
            }
        }
        let room = nevents * gkq::KEVENT_BYTES;
        if room > 0 && self.read_va_prefix(elist, room).len() < room {
            return Err(fail(format!("M48: kevent event list at {elist:#x} does not translate for {nevents} \
                                     entries: the kernel applies the changes before its EFAULT, which is not \
                                     modelled (K5)")));
        }
        let cur = self.threads.current();
        if let Some(w) = self.gkq.waiter(kq).filter(|_| nevents > 0) {
            return Err(fail(format!("M48: kevent on kq {kq}: thread {cur} scans it while thread {} is \
                                     blocked on it; which thread takes an event is unmeasured (K2)", w.tid)));
        }
        // 3. The changes, then any waiter they readied.
        self.gkq.apply(kq, &changes).map_err(fail)?;
        self.wake_kevent_waiters().map_err(fail)?;
        // 4. F7: no event list, no scan.
        if nevents == 0 { return Ok((0, false)); }
        // 5. The events found.
        let events = self.gkq.take_events(kq, nevents).map_err(fail)?;
        if !events.is_empty() {
            let out: Vec<u8> = events.iter().flat_map(|e| e.to_bytes()).collect();
            self.write_va_committing(elist, &out).map_err(|m| fail(format!("M48: kevent event list: {m}")))?;
            return Ok((events.len() as u64, false));
        }
        // 6. None found: poll, or block.
        let deadline = match timeout {
            Some((0, 0)) => return Ok((0, false)),
            None => None,
            // F6: 24 MHz, so ns * 3 / 125 ticks, truncating. 1 ns is 0 ticks, so node's 1 ns wait is
            // due as it blocks, and this landmark's own settle wakes it (§11b item 8).
            Some((sec, nsec)) => {
                let ns = sec as u128 * 1_000_000_000 + nsec as u128;
                Some(self.now_guest().saturating_add(u64::try_from(ns * 3 / 125).unwrap_or(u64::MAX)))
            }
        };
        self.gkq.set_waiter(kq, gkq::Waiter { tid: cur, events: elist, nevents });
        self.threads.block(thread::BlockReason::Kevent { kq, deadline });
        Ok((0, false))
    }

    /// M48 §3c: what a completed call did to the guest's kqueues and pipes. It is called after the
    /// call's return is set:
    /// - on record, by the generic forward arm and the console-close arm (K6), with the forward's
    ///   values;
    /// - on replay, by the generic mirror, with the recorded ones.
    ///
    /// The same method with the same arguments on both sides (symmetry rule 1). It reads these calls:
    /// - `kqueue` creates the returned fd's table, and `pipe` (`Ret::FdPair`) the pair's count (R4);
    /// - `close`/`close_nocancel` drop a table, the closed fd's knotes and a pipe end;
    /// - `dup`, `dup2` and `F_DUPFD`/`F_DUPFD_CLOEXEC` copy a pipe end, and refuse a kqueue;
    /// - `read`, `read_nocancel`, the `write` pair and the `writev` pair move a pipe's count by
    ///   their return.
    ///
    /// Then it wakes any `kevent` waiter a pipe change made ready. A failed call changed nothing.
    /// `Err` is a refusal by value. On record the forward has already happened, so the recorder
    /// stops after the call's landmark.
    pub fn note_fd_effects(&mut self, num: u64, args: [u64; 8], ret: u64, ret1: u64, err: bool) -> Result<(), String> {
        if err { return Ok(()); }
        let fail = |why: String| format!("{why}. syscall {num} args=[{}] ret={ret:#x}", Self::fmt_args(args));
        // A `match` at statement start ends at its brace, so its result is bound before `map_err`.
        let effect = match num {
            SYS_KQUEUE => self.gkq.create(ret),
            n if retrace_arch::returns_fd_pair(n) => self.gkq.pipe(ret, ret1),
            n if retrace_arch::is_close_syscall(n) => self.gkq.close(args[0]),
            n if n == retrace_arch::SYS_DUP || n == retrace_arch::SYS_DUP2 || retrace_arch::is_fcntl_dupfd(n, &args) =>
                self.gkq.dup(args[0], ret),
            retrace_arch::SYS_READ | retrace_arch::SYS_READ_NOCANCEL => self.gkq.note_read(args[0], ret),
            n if retrace_arch::is_write_syscall(n) || n == SYS_WRITEV || n == SYS_WRITEV_NOCANCEL =>
                self.gkq.note_write(args[0], ret),
            _ => Ok(()),
        };
        effect.map_err(fail)?;
        self.wake_kevent_waiters().map_err(fail)
    }

    /// M48 (header, "Delivery happens at the wake"; §11a item 6): make `tid`, blocked in a timed-wait
    /// primitive, runnable, and give it its reply. `events` are `(guest VA, bytes)` writes, through
    /// the guest's stage-1 walk.
    ///
    /// **The current thread's reply goes onto the vCPU; any other thread's goes into its saved
    /// context** (M46's `enter_manager` posture). `switch_to_thread` returns early for the current
    /// thread, so a reply written only to the table would never load (Review Focus 2). The blocking
    /// landmark's `set_x0_err_and_return` already made the context a post-return one, so only `x0`
    /// and the carry change: never PC, ELR or SPSR. The refusal (K8) comes before any write.
    pub fn deliver_wake(&mut self, tid: usize, ret: u64, err: bool, events: &[(u64, Vec<u8>)]) -> Result<(), String> {
        self.threads.wake(tid)?;
        for (va, bytes) in events {
            self.write_va_committing(*va, bytes).unwrap_or_else(|m| panic!(
                "M48: kevent event list of thread {tid} at {va:#x}: {m}. guest_kevent checked it at the call"));
        }
        let c = if err { retrace_arch::PSTATE_C } else { 0 };
        if tid == self.threads.current() {
            self.vcpu.set_reg(reg::x(0), ret).unwrap();
            let cpsr = self.vcpu.get_reg(reg::CPSR).unwrap();
            self.vcpu.set_reg(reg::CPSR, (cpsr & !retrace_arch::PSTATE_C) | c).unwrap();
        } else {
            let ctx = self.threads.ctx_mut(tid);
            ctx.regs.x[0] = ret;
            ctx.regs.cpsr = (ctx.regs.cpsr & !retrace_arch::PSTATE_C) | c;
        }
        Ok(())
    }
```

**(d) The wake paths**, after `fire_due_timers`:

```rust
    /// M48 §3d: wake every blocked thread whose deadline the guest clock has reached, in deadline
    /// order with ties by thread index (R7), each with its primitive's timeout answer. The clock is
    /// read only when some thread has a deadline, so a static box, and every pre-M48 guest, never
    /// reach it. Below the trace, so a refusal panics on both sides alike (R5).
    fn wake_due_threads(&mut self) {
        if self.threads.earliest_deadline().is_none() { return; }
        let now = self.now_guest();
        for tid in self.threads.due_waiters(now) {
            // An earlier wake in this pass may already have made it runnable: one timeout can wake
            // other waiters too (a psynch timeout does).
            if !matches!(self.threads.state_of(tid), thread::ThreadState::Blocked(_)) { continue; }
            match self.threads.state_of(tid) {
                thread::ThreadState::Blocked(thread::BlockReason::Kevent { kq, .. }) => self.kevent_timed_out(tid, kq),
                s => unreachable!("M48: due_waiters returned thread {tid} in {s:?}, which has no deadline"),
            }
        }
    }

    /// M48 §3c: `tid`'s `kevent` on `kq` reached its deadline. It takes what the kqueue holds for it,
    /// which is nothing unless an activation path missed its wake (K7), so it ordinarily returns 0,
    /// as `kern_event.c:kqueue_scan` does at its deadline. Below the trace, so a refusal (K8)
    /// panics with its text on both sides (R5).
    fn kevent_timed_out(&mut self, tid: usize, kq: u64) {
        let w = self.gkq.take_waiter(kq).filter(|w| w.tid == tid).unwrap_or_else(|| panic!(
            "M48: thread {tid} is blocked in kevent on kq {kq}, which records no such waiter"));
        self.deliver_kevent(kq, w).unwrap_or_else(|m| panic!("{m}"));
    }

    /// M48 §3c: wake every thread blocked in `kevent` whose kqueue now holds an event, in kqueue fd
    /// order: after a call's changes (a `NOTE_TRIGGER`), and after a pipe changed.
    fn wake_kevent_waiters(&mut self) -> Result<(), String> {
        for kq in self.gkq.ready_waiters() {
            let w = self.gkq.take_waiter(kq).expect("ready_waiters lists only kqueues with a waiter");
            self.deliver_kevent(kq, w)?;
        }
        Ok(())
    }

    /// Wake `w`, the waiter of `kq`, with up to `w.nevents` of its events: written to its event
    /// list, `x0` their count, the carry clear.
    fn deliver_kevent(&mut self, kq: u64, w: gkq::Waiter) -> Result<(), String> {
        let events = self.gkq.take_events(kq, w.nevents)?;
        let out: Vec<u8> = events.iter().flat_map(|e| e.to_bytes()).collect();
        let writes = if out.is_empty() { vec![] } else { vec![(w.events, out)] };
        self.deliver_wake(w.tid, events.len() as u64, false, &writes)
    }
```

**(e) The deadline queue.** Replace `schedule_after_block`'s doc's numbered list and its body:

```rust
    /// M46 §3e added time, and M48 §3d makes it one deadline queue (R7). It is still a pure function
    /// of box state, because every path that reaches here (`run()`, `step()`, replay's
    /// `finish_event`) reaches it at the same point in the guest's own syscall sequence:
    /// 1. **Overdue timers fire** (M46).
    /// 2. **Due waiters wake** (M48, `wake_due_threads`). Each thread whose timed wait has reached its
    ///    deadline wakes, in deadline order with ties by thread index, with its primitive's timeout
    ///    answer. A deadline already past at the call (node's 1 ns waits) wakes here, in the settle
    ///    that blocked it.
    /// 3. **The pick.**
    /// 4. **The idle jump.** If nothing is runnable and any deadline exists, `synthetic_tsc` jumps so
    ///    that the guest clock reads the earliest of ALL deadlines, workqueue timers and thread
    ///    deadlines alike. Rules 1–2 then run again, and the pick is retried, exactly once. The
    ///    clock never moves backwards (`kq::tsc_for_deadline`).
    /// 5. **Otherwise it is a deadlock**, as since M14. The panic lists every thread's state (each
    ///    blocked reason with its deadline), the knote table and the guest kqueues.
    ///
    /// A deadline is reached only when some thread blocks. A guest that spins without blocking never
    /// lets a timer fire or a timed wait end. This is the cooperative scheduler's limit, extended to
    /// time (`docs/current-state.md`).
    pub fn schedule_after_block(&mut self) {
        self.fire_due_timers();
        self.wake_due_threads();
        let mut next = self.threads.pick_next();
        if next.is_none() {
            let earliest = [self.kq.earliest_deadline(), self.threads.earliest_deadline()].into_iter().flatten().min();
            if let Some(deadline) = earliest {
                self.synthetic_tsc = kq::tsc_for_deadline(self.synthetic_tsc, self.timebase_offset(), deadline);
                self.fire_due_timers();
                self.wake_due_threads();
                next = self.threads.pick_next();
            }
        }
        match next {
            Some(tid) => self.switch_to_thread(tid),
            None => panic!(
                "M14: DEADLOCK — no runnable thread. {} live of {} total. States: {:?}. Knotes: {:?}. \
                 Guest kqueues: {:?}",
                self.threads.live(),
                self.threads.len(),
                (0..self.threads.len()).map(|i| self.threads.state_of(i)).collect::<Vec<_>>(),
                self.kq,
                self.gkq
            ),
        }
    }
```

Keep the doc's first two paragraphs (the pick and symmetry rule 2) as they are.

- [ ] **Step 8: Green at box level, and the whole box chunk.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --test gkq --no-fail-fast -- --test-threads=1 > $L/t4-box-gkq.log 2>&1; echo "gkq exit=$?"; grep -a 'test result' $L/t4-box-gkq.log
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t4-box.log 2>&1; echo "box exit=$?"; grep -a -E '^test result|FAILED|panicked' $L/t4-box.log | head -60
```

Expected:
- `gkq` 10 passed.
- The whole `retrace-box` chunk is green, including `kqmanager`, `threads`, `checkpointparity` and `restoreparity`. Those exercise `schedule_after_block`, `BoxState` and `dbg_internal_state` without a timed thread wait, so for them the new queue must be a no-op.

- [ ] **Step 9: The fixture.** Create `crates/retrace-guest/c/kq_dyn.c`:

```c
// M48 §3h: the guest-kqueue fixture (K1). Every mode prints what it observed, and kq_e2e compares
// that with this binary's NATIVE output, so a line holds only what native and retrace both
// determine: counts, filters, flags, fflags, data, udata, and booleans. It never holds a raw pipe fd
// or a time. stdout is line-buffered, so a line reaches the trace as it is printed, and a refused
// mode's missing "bad done" is a fact, not a buffer that was never flushed.
//
// argv[1] selects the mode:
//   probe      libuv's uv__kqueue_runtime_detection, byte for byte (M47 node.entry.txt; walls.md §1
//              row 1). An EVFILT_USER add and its NOTE_TRIGGER go in one call whose event list is
//              the change list, on a throwaway kqueue that is then closed. The next kqueue() reuses
//              the fd and must start empty.
//   wake       main blocks in kevent with no timeout. A second thread triggers its EVFILT_USER
//              knote in uv_async_send's shape (one change, no event list).
//   timeout    main waits 5 ms with nothing to wake it, while a second thread makes three calls and
//              exits. Only the clock can end the wait.
//   tryselect  libuv's uv__stream_try_select (walls.md §1): EVFILT_READ, EV_ADD|EV_ENABLE on fd 1,
//              one event, a 1 ns timeout, on the only thread. With stdout a pipe, native answers 0.
//   pipe       main waits on a pipe's read end, and a second thread writes 5 bytes. Then the write
//              end's room, and EOF once the write end is closed.
//   oneshot    an EV_ONESHOT knote is delivered once, and a second poll finds nothing.
//   bad filter an EVFILT_TIMER change on a guest kqueue: retrace refuses it by value.
//   bad notkq  kevent on fd 1, which is not a kqueue: retrace refuses it by value.
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/event.h>
#include <unistd.h>

// F6: the timebase is 24 MHz, so 5 ms is 120000 ticks.
#define FIVE_MS_TICKS 120000ULL

static int kq;
static int p[2];

static void show(const char *what, int n, const struct kevent *e) {
    printf("%s n=%d", what, n);
    for (int i = 0; i < n; i++)
        printf(" [ident=%#lx filter=%d flags=%#x fflags=%#x data=%#lx udata=%#lx]",
               (unsigned long)e[i].ident, e[i].filter, e[i].flags, e[i].fflags, (long)e[i].data,
               (unsigned long)(uintptr_t)e[i].udata);
    printf("\n");
}

static void *trigger(void *arg) {
    (void)arg;
    struct kevent t;
    EV_SET(&t, 7, EVFILT_USER, 0, NOTE_TRIGGER | NOTE_FFCOPY | 5, 9, (void *)0x5678);
    kevent(kq, &t, 1, NULL, 0, NULL);
    return NULL;
}

static void *busy(void *arg) {
    (void)arg;
    for (int i = 0; i < 3; i++) (void)getppid();
    return NULL;
}

static void *writer(void *arg) {
    (void)arg;
    write(p[1], "hello", 5);
    return NULL;
}

static int mode_probe(void) {
    kq = kqueue();
    struct kevent ch[2];
    EV_SET(&ch[0], 0x1e7e7711, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    EV_SET(&ch[1], 0x1e7e7711, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    int n = kevent(kq, ch, 2, ch, 1, &(struct timespec){0, 0});
    show("detect", n, ch);
    show("slot1", 1, &ch[1]);
    int first = kq;
    close(kq);
    kq = kqueue();
    struct kevent ev;
    n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 0});
    printf("reused=%d n=%d\n", kq == first, n);
    return 0;
}

static int mode_wake(void) {
    kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, 7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, (void *)0x1234);
    kevent(kq, &ev, 1, NULL, 0, NULL);
    pthread_t t;
    pthread_create(&t, NULL, trigger, NULL);
    // Under retrace this always blocks: the new thread runs only once main does.
    int n = kevent(kq, NULL, 0, &ev, 1, NULL);
    show("wake", n, &ev);
    pthread_join(t, NULL);
    return 0;
}

static int mode_timeout(void) {
    kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, 7, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, 0);
    kevent(kq, &ev, 1, NULL, 0, NULL);
    pthread_t t;
    pthread_create(&t, NULL, busy, NULL);
    uint64_t t0 = mach_absolute_time();
    int n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 5000000});
    uint64_t waited = mach_absolute_time() - t0;
    printf("timeout n=%d waited=%d\n", n, waited >= FIVE_MS_TICKS);
    pthread_join(t, NULL);
    return 0;
}

static int mode_tryselect(void) {
    kq = kqueue();
    struct kevent ch, ev;
    EV_SET(&ch, 1, EVFILT_READ, EV_ADD | EV_ENABLE, 0, 0, 0);
    uint64_t t0 = mach_absolute_time();
    int n = kevent(kq, &ch, 1, &ev, 1, &(struct timespec){0, 1});
    uint64_t waited = mach_absolute_time() - t0;
    printf("tryselect n=%d waited=%d\n", n, waited >= FIVE_MS_TICKS);
    close(kq);
    return 0;
}

static int mode_pipe(void) {
    pipe(p);
    kq = kqueue();
    struct kevent ev;
    EV_SET(&ev, p[0], EVFILT_READ, EV_ADD, 0, 0, (void *)0xabc);
    kevent(kq, &ev, 1, NULL, 0, NULL);
    pthread_t t;
    pthread_create(&t, NULL, writer, NULL);
    int n = kevent(kq, NULL, 0, &ev, 1, NULL);
    ev.ident = ev.ident == (uintptr_t)p[0]; // an fd number is the host's choice; which end it is is not
    show("readable", n, &ev);
    char buf[16];
    printf("read=%zd\n", read(p[0], buf, sizeof buf));
    pthread_join(t, NULL);
    EV_SET(&ev, p[1], EVFILT_WRITE, EV_ADD, 0, 0, 0);
    n = kevent(kq, &ev, 1, &ev, 1, &(struct timespec){0, 0});
    ev.ident = ev.ident == (uintptr_t)p[1];
    show("writable", n, &ev);
    close(p[1]);
    n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 0});
    ev.ident = ev.ident == (uintptr_t)p[0];
    show("eof", n, &ev);
    return 0;
}

static int mode_oneshot(void) {
    kq = kqueue();
    struct kevent ch[2], ev;
    EV_SET(&ch[0], 7, EVFILT_USER, EV_ADD | EV_ONESHOT, 0, 0, 0);
    EV_SET(&ch[1], 7, EVFILT_USER, 0, NOTE_TRIGGER, 0, 0);
    int n = kevent(kq, ch, 2, &ev, 1, &(struct timespec){0, 0});
    show("oneshot", n, &ev);
    n = kevent(kq, NULL, 0, &ev, 1, &(struct timespec){0, 0});
    printf("again n=%d\n", n);
    return 0;
}

static int mode_bad(const char *what) {
    kq = kqueue();
    struct kevent ev;
    if (!strcmp(what, "notkq")) {
        EV_SET(&ev, 7, EVFILT_USER, EV_ADD, 0, 0, 0);
        printf("notkq n=%d\n", kevent(1, &ev, 1, NULL, 0, NULL));
    } else {
        EV_SET(&ev, 1, EVFILT_TIMER, EV_ADD | EV_ONESHOT, 0, 1000, 0);
        printf("filter n=%d\n", kevent(kq, &ev, 1, NULL, 0, NULL));
    }
    printf("bad done\n");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *mode = argc > 1 ? argv[1] : "probe";
    if (!strcmp(mode, "probe")) return mode_probe();
    if (!strcmp(mode, "wake")) return mode_wake();
    if (!strcmp(mode, "timeout")) return mode_timeout();
    if (!strcmp(mode, "tryselect")) return mode_tryselect();
    if (!strcmp(mode, "pipe")) return mode_pipe();
    if (!strcmp(mode, "oneshot")) return mode_oneshot();
    if (!strcmp(mode, "bad")) return mode_bad(argc > 2 ? argv[2] : "filter");
    fprintf(stderr, "kq_dyn: unknown mode %s\n", mode);
    return 2;
}
```

Wire it with the `madv_dyn` recipe in `crates/retrace-guest/build.rs`, after `madv_dyn`'s block:

```rust
    // kq_dyn: the M48 guest-kqueue fixture — modes probe, wake, timeout, tryselect, pipe, oneshot
    // and bad (spec §3h). Same recipe as hello_dyn; pthreads are in libSystem.
    let src = format!("{}/c/kq_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/kq_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang kq_dyn");
    assert!(status.success(), "kq_dyn guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, after `MADV_DYN`, add:

```rust
/// M48: the guest's own kqueues by mode — `probe`, `wake`, `timeout`, `tryselect`, `pipe`,
/// `oneshot`, `bad filter`, `bad notkq` (spec §3h; see the source's header).
pub const KQ_DYN: &str = concat!(env!("OUT_DIR"), "/kq_dyn");
```

Then, in its `tests` module after `madv_guest_parses`:

```rust
    #[test]
    fn kq_dyn_guest_parses() {
        // M48: proves the build.rs wiring and the path constant; behaviour is kq_e2e's.
        let l = parse_macho(&std::fs::read(KQ_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

Run the native answers once by hand and keep them for the report. Stdout must be a pipe, as a test gives it: `tryselect`'s answer for fd 1 depends on what fd 1 is (K3). So each run pipes through `cat`, under `bash` for `PIPESTATUS`:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-guest --lib kq_dyn_guest_parses -- --test-threads=1 > $L/t4-guest.log 2>&1; echo "exit=$?"
K=$(ls -t target/debug/build/retrace-guest-*/out/kq_dyn | head -1)
for m in probe wake timeout tryselect pipe oneshot; do echo "== $m"; bash -c '"$0" "$1" | cat; echo "exit=${PIPESTATUS[0]}"' "$K" "$m"; done > $L/t4-native.log 2>&1
cat $L/t4-native.log
```

Expected (`T0(M3)` values from the addendum shown at their expected values):

```
== probe
detect n=1 [ident=0x1e7e7711 filter=-10 flags=0x21 fflags=0 data=0 udata=0]
slot1 n=1 [ident=0x1e7e7711 filter=-10 flags=0 fflags=0x1000000 data=0 udata=0]
reused=1 n=0
== wake
wake n=1 [ident=0x7 filter=-10 flags=0x21 fflags=0x5 data=0x9 udata=0x5678]
== timeout
timeout n=0 waited=1
== tryselect
tryselect n=0 waited=0
== pipe
readable n=1 [ident=0x1 filter=-1 flags=0x1 fflags=0 data=0x5 udata=0xabc]
read=5
writable n=1 [ident=0x1 filter=-2 flags=0x1 fflags=0 data=0x4000 udata=0]
eof n=1 [ident=0x1 filter=-1 flags=0x8001 fflags=0 data=0 udata=0xabc]
== oneshot
oneshot n=1 [ident=0x7 filter=-10 flags=0x11 fflags=0 data=0 udata=0]
again n=0
```

Each `exit=` is 0. A native line that differs from this block is a finding about the model's constants, not about the fixture. Report it, and fix the model (F8, F9, K9) before Step 11, because `kq_e2e` compares retrace with native.

- [ ] **Step 10: The gate, red first.** Create `crates/retrace/tests/kq_e2e.rs`:

```rust
// M48 gate (spec §3c, §3d, §3h; K1). The guest's own kqueues, end to end, on the repo-owned
// fixture `crates/retrace-guest/c/kq_dyn.c`. Every recorded run is compared with the SAME binary
// run natively, and every assertion is on the guest's output, the trace, a replay session's state,
// or the recorder's own words. Never an exit code alone: before M48 the recorder stopped at the
// first kevent, and a model that answered every wait with "0 events" would still exit 0.
mod util;

use retrace_core::{Advance, ReplaySession};
use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// One `kevent` landmark: (landmark index, thread, args, ret).
type Kev = (usize, u32, [u64; 8], u64);

/// The fixture run natively, with stdout a pipe, as `Command::output` gives it and as the recorder's
/// own stdout is under test.
fn native(argv: &[&str]) -> Vec<u8> {
    let o = std::process::Command::new(retrace_guest::KQ_DYN).args(argv).output().unwrap();
    assert!(o.status.success(), "native kq_dyn {argv:?}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

/// Every event of `trace` with its landmark index (the replay session's `idx`).
fn events(trace: &Path) -> Vec<(usize, Event)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().collect()
}

fn kevents(trace: &Path) -> Vec<Kev> {
    events(trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, ret, thread, .. } if num == retrace_arch::SYS_KEVENT => Some((i, thread, args, ret)),
        _ => None,
    }).collect()
}

/// Record `argv`, and assert:
/// - exit 0 with native's exact output;
/// - R3: every kevent landmark carries no writes and `ret1` 0, and there is at least one;
/// - two byte-identical replays.
fn records_as_native(argv: &[&str]) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::KQ_DYN, argv);
    assert_eq!(rec.code, 0, "{argv:?}: record: {}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&native(argv)), "{argv:?}: retrace vs native");
    let ks: Vec<(usize, u64, usize)> = events(&trace).into_iter().filter_map(|(i, e)| match e {
        Event::Syscall { num, ret1, writes, .. } if num == retrace_arch::SYS_KEVENT => Some((i, ret1, writes.len())),
        _ => None,
    }).collect();
    assert!(!ks.is_empty(), "{argv:?}: no kevent landmark, so this run proves nothing about the model");
    for (i, ret1, writes) in ks {
        assert!(ret1 == 0 && writes == 0, "{argv:?}: R3: kevent landmark {i} recorded ret1={ret1:#x} and {writes} writes");
    }
    for k in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{argv:?}: replay {k}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{argv:?}: replay {k} stdout");
    }
    (rec, trace)
}

/// The first landmark after `after` that `thread` issues.
fn next_on(trace: &Path, thread: u32, after: usize) -> usize {
    events(trace).into_iter().find_map(|(i, e)| match e {
        Event::Syscall { thread: t, .. } if i > after && t == thread => Some(i),
        _ => None,
    }).unwrap_or_else(|| panic!("no landmark of thread {thread} after {after}"))
}

fn synthetic_tsc(state: &str) -> u64 {
    let v = state.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no synthetic_tsc in {state}"));
    u64::from_str_radix(v, 16).unwrap()
}

/// A session at landmark `i`'s `svc`: the instruction that issues it, not yet retired
/// (`gcdtimer_e2e`'s helper). A tamper at `(i, 0)` would be overwritten inside window `i`.
fn session_at_svc(trace: &Path, i: usize) -> ReplaySession {
    const SVC_0X80: [u8; 4] = 0xd400_1001u32.to_le_bytes();
    let mut s = retrace_core::seek(trace, i, 0).unwrap();
    for _ in 0..1_000_000 {
        if s.read_mem_prefix(s.pc(), 4) == SVC_0X80 {
            return s;
        }
        s.step_insns(1).unwrap_or_else(|e| panic!("stepping window {i}: {e}"));
    }
    panic!("no svc within 1M instructions of landmark {i}");
}

/// Advance `s` until one landmark moves the clock by more than 60000 ticks (2.5 ms; a timebase read
/// moves it 9216). Returns (that landmark, the clock after it).
fn the_idle_jump(mut s: ReplaySession) -> (usize, u64, ReplaySession) {
    loop {
        let (n, before) = (s.landmark(), synthetic_tsc(&s.dbg_internal_state()));
        match s.advance() {
            Ok(Advance::Exited(_)) => panic!("the run exited and no landmark jumped the clock"),
            Ok(_) => {}
            Err(d) => panic!("diverged at landmark {}: {}", d.landmark, d.detail),
        }
        let after = synthetic_tsc(&s.dbg_internal_state());
        if after - before > 60_000 {
            return (n, after, s);
        }
    }
}

/// A checkpoint at `(from, 0)`, continued to `to`, must equal a cold seek to `(to, 0)`, in every
/// register, main's saved context, box state (the guest kqueues included) and memory.
fn warm_matches_cold(trace: &Path, from: usize, to: usize) -> ReplaySession {
    let cp = retrace_core::seek(trace, from, 0).unwrap().checkpoint();
    let warm = {
        let mut s = ReplaySession::from_checkpoint(trace, &cp).unwrap();
        s.advance_to_landmark(to).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_regs_of(0), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(trace, to, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_regs_of(0), "main's saved context: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.4, cold.dbg_internal_state(), "box state, the guest kqueues included: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.5).is_none(), "memory: checkpointed vs cold");
    cold
}

/// Main's wait in `wake`, `timeout` and `pipe`: thread 0, no changes, one event.
fn main_wait(ks: &[Kev]) -> Kev {
    *ks.iter().find(|k| k.1 == 0 && k.2[2] == 0 && k.2[4] == 1).expect("main's kevent wait")
}

/// Review Focus 1. libuv's runtime detection passes one buffer as both lists. Native returns ONE
/// event over slot 0 and leaves the second change in slot 1; the kqueue is closed and its fd
/// reused, empty. The difference M48 makes: before it the recorder stopped at this call.
#[test]
fn the_runtime_detection_probe_returns_natives_one_event() {
    let (rec, trace) = records_as_native(&["probe"]);
    assert!(String::from_utf8_lossy(&rec.stdout).starts_with(
        "detect n=1 [ident=0x1e7e7711 filter=-10 flags=0x21 fflags=0 data=0 udata=0]\n\
         slot1 n=1 [ident=0x1e7e7711 filter=-10 flags=0 fflags=0x1000000 data=0 udata=0]\n"),
        "walls.md §1 row 1, native/kqdetect.out: {:?}", String::from_utf8_lossy(&rec.stdout));
    let ks = kevents(&trace);
    assert_eq!(ks.len(), 2, "detect, then the reused kqueue's poll: {ks:?}");
    let (_, _, args, ret) = ks[0];
    assert_eq!((args[1] == args[3], args[2], args[4], ret), (true, 2, 1, 1),
        "one buffer as both lists, two changes, one event slot, one event");
    assert_eq!((ks[1].2[0], ks[1].3), (args[0], 0), "the reused fd, and an empty kqueue behind it");
}

/// §3c: the `uv_async_send` shape. Main blocks with no timeout; a second thread's trigger (one
/// change, no event list) wakes it. The wake writes main's reply into its SAVED context at the
/// trigger's own landmark, while the waker keeps running.
#[test]
fn a_trigger_from_another_thread_wakes_the_blocked_waiter_after_the_wakers_landmark() {
    let (_, trace) = records_as_native(&["wake"]);
    let ks = kevents(&trace);
    let wait = main_wait(&ks);
    let trig = *ks.iter().find(|k| k.1 != 0).expect("the trigger, from the second thread");
    assert_eq!(wait.3, 0, "a blocking landmark records 0: the answer comes at the wake");
    assert_eq!((trig.2[2], trig.2[4], trig.3), (1, 0, 0), "one change, no event list, 0 (F7)");
    assert!(wait.0 < trig.0 && next_on(&trace, 0, wait.0) > trig.0,
        "main waits, the waker triggers, and only then does main run again: wait {}, trigger {}", wait.0, trig.0);
    let s = retrace_core::seek(&trace, trig.0 + 1, 0).unwrap();
    assert_eq!(s.current_thread(), trig.1, "a wake does not switch: the waker still runs");
    let regs = s.dbg_regs_of(0).unwrap();
    assert!(regs.contains("x0 =0x0000000000000001"), "main's saved x0 is its one event, written at the wake:\n{regs}");
}

/// §3d, R7: a 5 ms wait with nothing to wake it ends by the idle jump, which lands EXACTLY on the
/// deadline: 120000 ticks after the call's own clock (F6), not past it.
#[test]
fn a_five_millisecond_timeout_is_reached_by_the_idle_jump() {
    let (rec, trace) = records_as_native(&["timeout"]);
    assert_eq!(rec.stdout, b"timeout n=0 waited=1\n");
    let wait = main_wait(&kevents(&trace));
    assert_ne!(wait.2[5], 0, "a timed wait");
    let s = session_at_svc(&trace, wait.0);
    let at_call = synthetic_tsc(&s.dbg_internal_state());
    let (n, after, s) = the_idle_jump(s);
    assert!(n > wait.0, "the jump comes after main's wait, once the other thread has exited");
    assert_eq!(after, at_call + 120_000, "F6: 5 ms is 120000 ticks, from the clock the call read");
    assert_eq!(s.current_thread(), 0, "the jump woke main, the only thread left");
}

/// Review Focus 2, end to end: libuv's `uv__stream_try_select` on the only thread. Its 1 ns wait
/// converts to 0 ticks, blocks, and is woken in its own landmark's settle with no jump, while it is
/// still the current thread. A kevent deadline answer is 0, as is the blocking landmark's, so this
/// test cannot tell a reply on the vCPU from one lost to the saved context; the box test
/// `a_wake_of_the_current_thread_writes_the_vcpu` pins that with a distinct reply. What it does pin:
/// the same-settle wake (no deadlock, no jump), K3's never-ready answer on fd 1, and native's output.
#[test]
fn a_timeout_on_the_only_thread_answers_on_the_vcpu() {
    let (rec, trace) = records_as_native(&["tryselect"]);
    assert_eq!(rec.stdout, b"tryselect n=0 waited=0\n");
    let ks = kevents(&trace);
    assert_eq!(ks.len(), 1, "{ks:?}");
    let (i, thread, args, ret) = ks[0];
    assert_eq!((thread, args[2], args[4], ret), (0, 1, 1, 0), "one change, one event slot, 0 events");
    let mut s = session_at_svc(&trace, i);
    let ch = s.read_mem_prefix(args[1], 32);
    assert_eq!((&ch[..8], &ch[8..10], &ch[10..12]),
        (&1u64.to_le_bytes()[..], &retrace_arch::EVFILT_READ.to_le_bytes()[..], &0x5u16.to_le_bytes()[..]),
        "EVFILT_READ, EV_ADD|EV_ENABLE on fd 1 (walls.md §1)");
    let at_call = synthetic_tsc(&s.dbg_internal_state());
    s.advance().unwrap_or_else(|d| panic!("diverged at landmark {}: {}", d.landmark, d.detail));
    assert_eq!((s.landmark(), s.current_thread()), (i + 1, 0), "woken in its own settle, on the only thread");
    assert_eq!(synthetic_tsc(&s.dbg_internal_state()), at_call, "no idle jump: the deadline was due at the call");
    assert!(s.dbg_regs().contains("x0 =0x0000000000000000"), "x0 on the vCPU:\n{}", s.dbg_regs());
}

/// R4, F9: a write's return moves the pipe's count and wakes the reader blocked on its read end,
/// with native's `data` (the count) and, after the write end closes, native's `EV_EOF`.
#[test]
fn a_pipe_write_wakes_the_reader_with_natives_byte_count_and_eof() {
    let (rec, trace) = records_as_native(&["pipe"]);
    let out = String::from_utf8_lossy(&rec.stdout);
    assert!(out.starts_with("readable n=1 [ident=0x1 filter=-1 flags=0x1 fflags=0 data=0x5 udata=0xabc]\nread=5\n")
            && out.ends_with("eof n=1 [ident=0x1 filter=-1 flags=0x8001 fflags=0 data=0 udata=0xabc]\n"), "{out:?}");
    let wait = main_wait(&kevents(&trace));
    let write = events(&trace).into_iter().find_map(|(i, e)| match e {
        Event::Syscall { num, ret: 5, thread, .. } if num == retrace_arch::SYS_WRITE && thread != 0 => Some(i),
        _ => None,
    }).expect("the writer thread's 5-byte write");
    assert!(wait.0 < write && next_on(&trace, 0, wait.0) > write, "main waits, the write lands, main runs");
    let regs = retrace_core::seek(&trace, write + 1, 0).unwrap().dbg_regs_of(0).unwrap();
    assert!(regs.contains("x0 =0x0000000000000001"), "the write's own landmark woke main with one event:\n{regs}");
}

/// F8: `EV_ONESHOT` drops the knote at its delivery.
#[test]
fn a_oneshot_knote_is_delivered_once_then_nothing() {
    let (rec, trace) = records_as_native(&["oneshot"]);
    assert_eq!(String::from_utf8_lossy(&rec.stdout),
        "oneshot n=1 [ident=0x7 filter=-10 flags=0x11 fflags=0 data=0 udata=0]\nagain n=0\n");
    assert_eq!(kevents(&trace).iter().map(|k| k.3).collect::<Vec<_>>(), vec![1, 0]);
}

/// Record a `bad` mode: the recorder must stop at the refusal, naming it, with the guest's kqueue()
/// landmark readable before it (so the empty-kevent check below is not vacuous).
fn refused(what: &str, why: &str) {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::KQ_DYN, &["bad", what]);
    assert_eq!(rec.code, 101, "{what}: the recorder must stop at the refusal (a panic). stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains(why), "{what}: the refusal must say {why:?}. stderr:\n{}", rec.stderr);
    let evs = events(&trace);
    assert!(evs.iter().any(|(_, e)| matches!(e, Event::Syscall { num: 362, .. })),
        "{what}: the kqueue() landmark before the refusal must be readable ({} events)", evs.len());
    assert!(kevents(&trace).is_empty(), "{what}: a refused call appends no landmark");
    assert!(!String::from_utf8_lossy(&rec.stdout).contains("bad done"), "{what}: the guest must not run past it");
}

/// R5: a filter the model does not have is refused by value, naming the change and the field.
/// Natively the call returns 0 (`filter n=0`).
#[test]
fn an_unmodelled_filter_stops_the_recorder_naming_it() {
    refused("filter", "M48: kevent change 0: (ident 0x1, filter -7, flags 0x11, fflags 0x0, data 0x3e8): filter -7 is not modelled");
}

/// Restore parity (§3c "through every path"): a checkpoint taken while main is blocked in kevent
/// carries the kqueue and its waiter, so the trigger replayed from it wakes the same thread with the
/// same event as a cold seek does.
#[test]
fn a_seek_into_a_blocked_kevent_matches_a_cold_seek() {
    let (_, trace) = records_as_native(&["wake"]);
    let ks = kevents(&trace);
    let (wait, trig) = (main_wait(&ks), *ks.iter().find(|k| k.1 != 0).unwrap());
    let state = retrace_core::seek(&trace, wait.0 + 1, 0).unwrap().dbg_internal_state();
    assert!(state.contains("waiter: Some(Waiter { tid: 0,"), "the checkpoint is inside main's blocked kevent:\n{state}");
    let cold = warm_matches_cold(&trace, wait.0 + 1, trig.0 + 2);
    assert!(cold.dbg_regs_of(0).unwrap().contains("x0 =0x0000000000000001"));
}

/// The same across a deadline: the checkpoint holds main blocked with its 5 ms deadline, and the
/// idle jump replayed from it lands where a cold seek's does.
#[test]
fn a_seek_inside_a_timed_kevent_matches_a_cold_seek() {
    let (_, trace) = records_as_native(&["timeout"]);
    let wait = main_wait(&kevents(&trace));
    let from = retrace_core::seek(&trace, wait.0 + 1, 0).unwrap();
    assert!(from.dbg_internal_state().contains("waiter: Some(Waiter { tid: 0,"), "inside main's timed kevent");
    let (n, after, _) = the_idle_jump(from);
    let cold = warm_matches_cold(&trace, wait.0 + 1, n + 1);
    assert_eq!((cold.current_thread(), synthetic_tsc(&cold.dbg_internal_state())), (0, after));
}

/// Review Focus 5. A shape the recording accepted but replay refuses is reached only after an
/// earlier silent divergence, so it must be a `Divergence` naming the call and the field, never a
/// panic. The tamper is in guest memory at the `svc` (main's registration's filter, EVFILT_USER to
/// EVFILT_TIMER), because a rewritten trace field is compared before the model runs.
#[test]
fn a_kevent_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_as_native(&["wake"]);
    let (i, _, args, _) = *kevents(&trace).iter().find(|k| k.1 == 0 && k.2[2] == 1 && k.2[4] == 0)
        .expect("main's registration: one change, no event list");
    let mut s = session_at_svc(&trace, i);
    assert_eq!(s.read_mem_prefix(args[1] + 8, 2), retrace_arch::EVFILT_USER.to_le_bytes(), "the change is EVFILT_USER");
    s.dbg_write_mem(args[1] + 8, &retrace_arch::EVFILT_TIMER.to_le_bytes()).unwrap();
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the tampered kevent at landmark {i} replayed") };
    assert_eq!(d.landmark, i);
    assert!(d.detail.starts_with("kevent refused on replay, though the recording accepted it")
            && d.detail.contains("M48: kevent change 0: (ident 0x7, filter -7")
            && d.detail.contains("filter -7 is not modelled"), "{}", d.detail);
}

/// The only test that sees the mirror's compare on an honest model: rewrite the detect landmark's
/// return, then its `ret1`, and replay must name each at that landmark (`kqinit_e2e`'s pattern).
#[test]
fn a_rewritten_kevent_return_is_a_divergence_naming_the_rc() {
    let (_, trace) = records_as_native(&["probe"]);
    let i = kevents(&trace)[0].0;
    for (ext, bad_ret, bad_ret1, why) in [
        ("rc2.bin", 2, 0, "kevent rc mismatch: replay 0x1 (err=false) != recorded 0x2 (err=false)"),
        ("ret1.bin", 1, 1, "kevent recorded ret1=0x1 with 0 writes"),
    ] {
        let mut ev = retrace_trace::Reader::open(&trace).unwrap();
        if let Event::Syscall { ret, ret1, .. } = &mut ev[i] { (*ret, *ret1) = (bad_ret, bad_ret1); }
        let bad = trace.with_extension(ext);
        let mut w = retrace_trace::Writer::create(&bad).unwrap();
        for e in &ev { w.append(e).unwrap(); }
        drop(w);
        let rp = util::replay(&bad);
        assert_eq!(rp.code, 3, "{ext}: replay of the rewritten trace must diverge (exit 3): {}", rp.stderr);
        assert!(rp.stderr.contains(&format!("DIVERGENCE at landmark {i} ")) && rp.stderr.contains(why),
            "{ext}: the divergence must be the mirror's {why:?}, at landmark {i}: {}", rp.stderr);
    }
}

/// R5: kevent on a descriptor that is not a guest kqueue is refused by value. Natively it is EBADF
/// (`notkq n=-1`).
#[test]
fn a_kevent_on_a_descriptor_that_is_not_a_kqueue_is_refused_by_value() {
    refused("notkq", "M48: kevent on fd 1, which is not a guest kqueue: the kernel answers EBADF, which is not modelled");
}
```

Run it red. Task 2's generic-arm assert is the only thing between kq_dyn's first kevent and the host:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test kq_e2e --no-fail-fast -- --test-threads=1 > $L/t4-e2e-red.log 2>&1; echo "exit=$?"; grep -a -E '^test |test result' $L/t4-e2e-red.log; grep -a -c 'kevent (363) reached the generic forward arm' $L/t4-e2e-red.log
```

Expected: exit 101 and 12 failed. Every recording stops at Task 2's assert, so the count of `kevent (363) reached the generic forward arm` is at least 1.

- [ ] **Step 11: The arms in `retrace-core`, green.** In `crates/retrace-core/src/lib.rs`:

**(a) The record arm**, directly after M45's `kevent_qos` arm in `record_box` (before the generic forward arm, which Task 2's assert guards):

```rust
            // M48 §3c (K1): kevent on a guest kqueue is MODELLED, never forwarded (see
            // Box_::guest_kevent). Forwarded, it would act on a host kqueue holding none of the
            // guest's knotes, and a wait would block the recorder. This arm may PANIC by design:
            // every unmodelled shape is refused by value before anything is appended.
            //
            // `writes` is empty and that is deliberate (R3): the event list the call fills, now or
            // at a later wake, is box output the mirror recomputes from the same guest memory and
            // box state. The exit-time full-memory comparison still covers every byte of it.
            Stop::Syscall { num, args } if num == retrace_arch::SYS_KEVENT => {
                let (rc, err) = b.guest_kevent(args).unwrap_or_else(|m| panic!("{m}"));
                w.append(&Event::Syscall { num, args, ret: rc, ret1: 0, err, writes: vec![], thread })
                    .map_err(|e| format!("append kevent: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, err);
            }
```

**(b) The hook in record's generic arm.** After its final `b.set_x0_err_and_return(ret, err);`, add:

```rust
                // M48 §3c: what the call did to the guest's kqueues and pipes (Box_::note_fd_effects),
                // after its return is set, as replay's generic mirror does. A refusal stops the
                // recorder after this landmark: the forward has already happened.
                b.note_fd_effects(num, args, ret, ret1, err).unwrap_or_else(|m| panic!("{m}"));
```

**(c) The hook in the console-close arm** (Ruling K6). After that arm's `b.set_x0_err_and_return(0, false);`, add:

```rust
                // M48 K6: replay finishes this landmark through the generic mirror, which calls the
                // hook, so record calls it too: the same method with the same arguments on both
                // sides (symmetry rule 1).
                b.note_fd_effects(num, args, 0, 0, false).unwrap_or_else(|m| panic!("{m}"));
```

**(d) The mirror**, in `ReplaySession::advance`, directly after M45's `kevent_qos` mirror, inside the same `Event::Syscall` chain after the arm-top `verify_thread`:

```rust
                            // M48 §3c: the record arm's mirror (symmetry rule 1): the same method
                            // with the same arguments, beside M45's so it inherits the arm-top
                            // `verify_thread` and adds none of its own. The model is a function of
                            // guest memory, the guest clock and box state, so the recomputed answer
                            // must equal the recording's. A refusal here is a divergence, never a
                            // panic: replay reaches one only after an earlier silent divergence
                            // (Review Focus 5).
                            if num == retrace_arch::SYS_KEVENT {
                                let (rc, e) = match self.b.guest_kevent(args) {
                                    Ok(r) => r,
                                    Err(m) => return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "kevent refused on replay, though the recording accepted it \
                                         — replay diverged before this landmark: {m}") }),
                                };
                                if (rc, e) != (*ret, *err) {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "kevent rc mismatch: replay {rc:#x} (err={e}) != recorded {ret:#x} (err={err})") });
                                }
                                // Record fixes `ret1: 0, writes: []`. A recording carrying either is
                                // not one this arm produced (M45's stance).
                                if *ret1 != 0 || !writes.is_empty() {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "kevent recorded ret1={ret1:#x} with {} writes; the model records neither", writes.len()) });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

**(e) The hook in replay's generic mirror.** Between its `self.b.apply_and_return(*ret, *err, writes);` and `return self.finish_event();`, add:

```rust
                            // M48 §3c: record's generic arm and console-close arm (K6) call the
                            // same hook with the same values. Replay reaches a refusal only after
                            // an earlier divergence, so it is reported as one.
                            if let Err(m) = self.b.note_fd_effects(num, args, *ret, *ret1, *err) {
                                return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                    "syscall {num} refused on replay, though the recording accepted it \
                                     — replay diverged before this landmark: {m}") });
                            }
```

The `verify_thread` count stays seven: the mirror is inside the chain the arm-top call already covers (header, Global Constraints). `.guest_kevent(` now appears exactly twice in this file, once before `pub fn advance(&mut self)` and once after it. `.note_fd_effects(` appears exactly three times: the console-close arm and the generic arm, both before `advance`, and the generic mirror after it. Those are the counts Task 10's audit check 2 expects. Do not add another call or route any of them through a helper. The record arm's guard is the single condition `num == retrace_arch::SYS_KEVENT`, which Task 10's control replaces with `false`.

Build the workspace, then run the gate green:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo build --workspace --all-targets > $L/t4-build.log 2>&1; echo "exit=$?"; grep -a -E '^(error|warning)' $L/t4-build.log | head
cargo test -p retrace --test kq_e2e --no-fail-fast -- --test-threads=1 > $L/t4-e2e.log 2>&1; echo "exit=$?"; grep -a -E '^test |test result' $L/t4-e2e.log
```

Expected:
- the build exits 0 with no warnings;
- `kq_e2e` exits 0 with 12 passed.

A failure in `records_as_native`'s first comparison is a retrace-vs-native difference: read both outputs in the log. It must be fixed in the model (F8, F9, K9), never by loosening the comparison.

- [ ] **Step 12: Regression.** The change reaches every blocking guest, since `schedule_after_block` gained the queue. It also reaches every fd-lifecycle syscall on both sides, since the hook runs in the generic arm and the generic mirror. So run:
  - the crates this task touched, whole;
  - the gates whose guests block, seek, or open and close descriptors the most.

Each target gets its own log and exit code:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t4-reg-box.log 2>&1; echo "box exit=$?"
cargo test -p retrace-core --no-fail-fast -- --test-threads=1 > $L/t4-reg-core.log 2>&1; echo "core exit=$?"
cargo test -p retrace-guest --no-fail-fast -- --test-threads=1 > $L/t4-reg-guest.log 2>&1; echo "guest exit=$?"
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/t4-reg-bins.log 2>&1; echo "bins exit=$?"
for t in kq_e2e gcdtimer_e2e kqinit_e2e dispatch_e2e thread_rust_e2e thread_watch_e2e thread_oracle sigthread_e2e sigblocked_e2e pipe_e2e closewrite_e2e dup2_e2e dupfd_e2e fdtable_e2e hello_dyn_e2e jq_e2e jq_file_e2e git_e2e hitorder_e2e checkpoint_seek determinism skiplines; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t4-reg-$t.log 2>&1; echo "$t exit=$?"; done
grep -a -h -E '^test result' $L/t4-reg-*.log | sort | uniq -c
```

Expected:
- every exit is 0;
- `retrace-box` counts 23 tests more than at plan time: 2 thread, 11 `gkq` unit and 10 in `tests/gkq.rs`;
- `retrace-guest` counts 1 more;
- `kq_e2e` counts 12.

`jq_e2e`, `jq_file_e2e` and `git_e2e` skip loudly without their tools; a `SKIPPED` line is a skip, not a pass. Report it as one. Each target above exists in `crates/retrace/tests/` at plan time. If one does not, drop it, say so, and do not invent one.

- [ ] **Step 13: Clippy, then commit.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo clippy -p retrace-box -p retrace-guest -p retrace-core -p retrace --all-targets -- -D warnings > $L/t4-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git commit -m "M48 t4: one deadline queue and guest kqueues — BlockReason::Kevent, the deadline queue in schedule_after_block, gkq.rs (K1, pipes included), guest_kevent/note_fd_effects/deliver_wake, the kevent arm and mirror, kq_dyn and kq_e2e"
git rev-parse HEAD
```

Expected: clippy exit 0. Note the commit hash for Step 14.

- [ ] **Step 14: Controls (on the committed tree).** Run each control alone. Restore with `git checkout <t4 commit> -- <file>`, confirm `git status --short` is empty, and record the actual failing text in the task report.
  1. **The reply on the vCPU** (Review Focus 2).
     - **Break:** in `Box_::deliver_wake`, replace the `if tid == self.threads.current() { … } else { … }` with the `else` body alone, so the reply always goes to the saved context.
     - **Run:** `cargo test -p retrace-box --test gkq --no-fail-fast -- --test-threads=1`.
     - **Expect:** `a_wake_of_the_current_thread_writes_the_vcpu` red at "the reply is on the vCPU".
     - **Then:** run `kq_e2e`'s `a_timeout_on_the_only_thread_answers_on_the_vcpu`. It is expected to stay GREEN. Its reply equals the blocking landmark's 0 (the test's doc says so). Record that it stayed green as the measured limit of the e2e pin.
     - **Restore:** `crates/retrace-box/src/lib.rs`.
  2. **Restore parity.**
     - **Break:** in `from_checkpoint`, replace `gkq: state.gkq.clone()` with `gkq: gkq::GuestKqueues::default()`.
     - **Run:** `cargo test -p retrace-box --test gkq` and `cargo test -p retrace --test kq_e2e`, each `--no-fail-fast -- --test-threads=1`.
     - **Expect:**
       - `a_blocked_kevent_survives_a_checkpoint` red at "the kqueue, its knote and its waiter";
       - `a_seek_into_a_blocked_kevent_matches_a_cold_seek` red with `warm: diverged at` and `which is not a guest kqueue`;
       - `a_seek_inside_a_timed_kevent_matches_a_cold_seek` red with the panic `M48: thread 0 is blocked in kevent on kq`, raised by the deadline wake.
     - **Restore:** `crates/retrace-box/src/lib.rs`.
  3. **The deadline queue.**
     - **Break:** delete both `self.wake_due_threads();` lines from `schedule_after_block`.
     - **Run:** `kq_e2e`.
     - **Expect:**
       - `a_timeout_on_the_only_thread_answers_on_the_vcpu` red, its record stopping at `M14: DEADLOCK`. The 1 ns deadline is due, the jump does not move the clock, and nothing wakes the thread.
       - In the box tests, `a_one_nanosecond_wait_is_woken_in_the_same_settle_without_a_jump` red at the same panic.
     - **Restore:** `crates/retrace-box/src/lib.rs`.
  4. **The mirror's hook** (symmetry rule 1).
     - **Break:** delete the `note_fd_effects` call from replay's generic mirror.
     - **Run:** `kq_e2e`.
     - **Expect:** every test that replays red. Replay never creates a kqueue, so its first `kevent` mirror is a divergence, `kevent refused on replay … which is not a guest kqueue`, and replay exits 3.
     - **Restore:** `crates/retrace-core/src/lib.rs`.

     The console-close arm's call (K6) has no control. Deleting it changes only a K3 knote on fd 0–2, which never activates, so no kq_dyn mode can observe the difference; it holds by construction. Say so in the report rather than claim a guard.

---

### Task 5: psynch condition variables (P1: `retrace-box`, `retrace-core`, fixture, e2e)

M48 §3e, with §3d's queue (Task 4), §3h, §4's `condvar_e2e` rows and Review Focus 2 (the Task-5 half), 3 and 5.
- **The port.** A pure `psynch.rs` ports libpthread-539.100.4's `kern/kern_synch.c` condition-variable paths and `kern/synch_internal.h`'s sequence arithmetic (plan F1), keyed by the guest cv **address**, the correlation the ulock pair uses.
- **The box.** `Box_::guest_psynch` dispatches by number, gives a timed `cvwait` its deadline on Task 4's queue (`BlockReason::Cv`), blocks the caller, and writes each woken thread's word through Task 4's `deliver_wake`. A deadline wake runs the port's timeout branch (`cv_timed_out`).
- **Never forwarded, nothing recorded.** One record arm and one mirror serve every SDK psynch number. `cvwait`, `cvsignal` and `cvbroad` are the port; the mutex pair, `cvclrprepost`, the rwlock calls and every shape no walk measured are refused by value (prefix `M48: psynch `). Replay reports a refusal as a `Divergence` naming the call.

**Rulings made in this task** (each is cited by its tag in the code and the report):
- **T5-a. Two fixture modes beyond §3h's four.** `onens` is node's `{0, 1 ns}` shape on the only thread, which Review Focus 2 pins. `mutex` reaches `psynch_mutexwait` (301) through a contended firstfit mutex, so §1 part 5's "stops the recorder by value" is guarded end to end for the port.
- **T5-b. A woken waiter's continuation runs at its wake.** The kernel runs `psynch_cvcontinue` when the woken thread next runs. For a signal wake that is exact: `ksyn_signal` already took the waiter off the queue, and the continuation reads only its own `kwe_psynchretval`. For a deadline wake the kernel's waiter stays queued until its continuation, so a `cvsignal` from a thread scheduled in between could still claim it ("the condition var granted", kern_synch.c:1318-1323). The model takes the other legal interleaving, the continuation first, deterministically. Both are kernel behaviours; the cooperative scheduler picks one.
- **T5-c. A cv's queue is freed when it empties** (`ksyn_wqrelease` with `qfreenow`), and a woken thread's reference goes with its wake (T5-b). The next call on that address starts a fresh queue from its own words (`ksyn_wqfind`). Under the cooperative scheduler an empty queue only follows an L == S transition, which zeroes L, U and S anyway (`KSYN_KWF_ZEROEDOUT`), so a kept queue and a fresh one answer alike.
- **T5-d. What is refused, and what is left out.** Refused by value, before anything changes:
  - every psynch number but 303–305 (`psynch_mutexwait`, `psynch_mutexdrop`, `psynch_cvclrprepost`, the rwlock calls): plan P4 measured none;
  - a `cvwait` whose mutex is nonzero (plan F3: it needs the mutex pair);
  - a process-shared cv (`flags & 0x30 == 0x10`: keyed by VM object, not address; every measured flags word is `0xa0`);
  - a targeted `cvsignal` (thread port nonzero; P4 saw none);
  - the two `EINVAL` sequence checks and `cvwait`'s `EBUSY` (a waiter already at its sequence): no correct guest passes them;
  - a `cvbroad` whose count exceeds the guest's thread count. The kernel's bound is `get_task_threadmax()`, a host value; a guest's count is its unreleased waiters, fewer than its threads, so this refuses only counts no guest produces;
  - a timeout whose `sec` is negative or whose interval passes 2^64 ns.

  Left out, because no cv path reads them or retrace never produces them: `kw_cvkernelseq`, `kw_lowseq`, `kw_highseq` (written, never read by a cv path); `kw_prepost`, `kw_intr` (mutex and rwlock only); thread cancellation (`__pthread_testcancel` and the scans' cancelled-thread skips: retrace models no cancellation). The two queue-insert collisions (`ksyn_queue_insert`'s `EBUSY`/`ESRCH` on a prepost the kernel inserts and ignores) are kept as `Err`s with no fixture: the kernel cannot reach them either.
- **T5-e. A signal pending on a thread a psynch call would wake is refused at the wake**, with Task 4's prefix `M48: a signal is pending on thread `, checked for every woken thread before the model or the thread table changes. That is M18's semaphore posture (spec §3e "Signals").
- **T5-f. Review Focus 5's replay-side refusal is planted in box state.** A `cvwait`'s refusals depend on its arguments (compared before the mirror runs) or on the queue. The test therefore plants a stale waiter through two test-only hooks, `Psynch::dbg_plant_waiter` and `ReplaySession::dbg_psynch_mut`, which is the state an earlier silent divergence would leave.
- **T5-g. The timeout's conversion is the port's own pure function**, `psynch::timeout_ticks`, with the F6 arithmetic Task 4 uses for `kevent` (`ns * 3 / 125`, truncating, so 1 ns is 0 ticks). The box adds `now_guest()` at the call.

**Files:**
- Create: `crates/retrace-box/src/psynch.rs` (11 unit tests)
- Modify: `crates/retrace-box/src/thread.rs` (`BlockReason::Cv`; the `deadline()` arm; `ThreadTable::wake`'s pattern)
- Modify: `crates/retrace-box/src/lib.rs` (`pub mod psynch;`; the `psynch` field through every path; `guest_psynch`, `cv_timed_out`; the `Cv` arm in `wake_due_threads`; `dbg_psynch`, `dbg_psynch_mut`)
- Modify: `crates/retrace-core/src/lib.rs` (the psynch arm in `record_box`, its mirror in `ReplaySession::advance`, `ReplaySession::dbg_psynch_mut`)
- Create: `crates/retrace-box/tests/psynch.rs` (4 tests)
- Create: `crates/retrace-guest/c/condvar_dyn.c`; modify `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` (`CONDVAR_DYN`, `condvar_dyn_guest_parses`)
- Create: `crates/retrace/tests/condvar_e2e.rs` (8 tests)

Tests added: 11 + 4 + 1 + 8 = 24, the header's prediction for Task 5.

**Interfaces:**
- Consumes:
  - **Task 2:** `retrace_arch::{SYS_PSYNCH_CVBROAD, SYS_PSYNCH_CVSIGNAL, SYS_PSYNCH_CVWAIT}` (`u64`, like every `SYS_` constant) and `is_psynch`; the 303–305 rows; the generic arm's `is_psynch` assert. The psynch constants `PTHRW_INC`, `PTHRW_COUNT_SHIFT`, `PTHRW_COUNT_MASK`, `PTHRW_MAX_READERS`, `PTH_RWL_MTX_WAIT`, `PTH_RWS_CV_CBIT`, `PTH_RWS_CV_PBIT`, `PTH_RWS_CV_MBIT` (`u32`, the kernel's `uint32_t` words), and `ETIMEDOUT`, `ECVCLEARED` and `ECVPREPOST` (`u64`, the errno word), exactly as Task 2 types them.
  - **Task 4** (its Interfaces block, consumed by name): `BlockReason::Kevent { kq, deadline }`'s guest-clock deadline domain; `impl BlockReason { fn deadline(&self) -> Option<u64> }`; `ThreadTable::wake(&mut self, tid) -> Result<(), String>` and its state assert; `Box_::deliver_wake(&mut self, tid: usize, ret: u64, err: bool, events: &[(u64, Vec<u8>)]) -> Result<(), String>`; the private `Box_::wake_due_threads`'s per-reason `match`; the field-through-every-path pattern (Box_ field after `gkq`, `BoxState` field, `checkpoint()`, `from_checkpoint`, the three literals, `dbg_internal_state`'s ` gkq={:?}`).
  - **t0 M4** (`T0(M4)`): the kernel's return words, the `cvwait` flags word and the `c_seq` offset (Step 1).
  - **Existing:** `Box_::{now_guest, fmt_args, settle_schedule, set_x0_err_and_return, threads, threads_mut, checkpoint, from_checkpoint}`, `ThreadTable::{block, pending_of, pend, ctx_mut, spawn}`, `retrace_arch::PSTATE_C`, `retrace_core::{seek, ReplaySession}`, `crates/retrace/tests/util`.
- Produces (Tasks 6, 10 and 11 consume these names exactly):
  - **`retrace_box::psynch`** (pub module): `Psynch` (`Clone + Debug + Default + PartialEq + Eq`) with `cvwait(&mut self, args: [u64; 8], tid: usize) -> Result<Outcome, String>`, `cvsignal(&mut self, args: [u64; 8]) -> Result<Outcome, String>`, `cvbroad(&mut self, args: [u64; 8], nthreads: usize) -> Result<Outcome, String>`, `time_out(&mut self, cv: u64, tid: usize) -> TimedOut`, `is_empty(&self) -> bool`, `kwq(&self, cv: u64) -> Option<&Kwq>` and `#[doc(hidden)] dbg_plant_waiter(&mut self, cv: u64, lockseq: u32, tid: usize)`; the types `Kwq`, `Kwe`, `KweState`, `Wake { tid, word }`, `Ret::{Word(u32), Block}`, `Outcome { ret, woken }`, `TimedOut { word, woken }`; the free functions `is_seqlower`, `is_seqlower_eq`, `is_seqhigher`, `is_seqhigher_eq`, `diff_genseq` (all `(u32, u32)`), `timeout_ticks(sec: u64, nsec: u64) -> Result<Option<u64>, String>` and `call_name(num: u64) -> &'static str`.
  - **`thread::BlockReason::Cv { addr: u64, deadline: Option<u64> }`**: `addr` is the guest cv address, `deadline` a guest-clock value (Task 4's domain), `None` for an untimed wait.
  - **`Box_::guest_psynch(&mut self, num: u64, args: [u64; 8]) -> Result<u64, String>`.** The caller's own `x0`, carry always clear: a word the call answers at once, or 0 when it blocks. `Err` is the refusal, raised before anything changes.
  - **The `psynch` field**: `Box_::psynch: psynch::Psynch`, declared after `gkq`, with `BoxState::psynch`, the `checkpoint()` and `from_checkpoint` lines, the three literals and ` psynch={:?}` in `dbg_internal_state`: the shapes Task 10's audit check 5 counts (`decl=2 lit=3 ckpt=1 from=1 dbg=1`). `#[doc(hidden)] pub fn dbg_psynch(&self) -> &psynch::Psynch` and `dbg_psynch_mut(&mut self) -> &mut psynch::Psynch`.
  - **The private `Box_::cv_timed_out(&mut self, tid: usize, addr: u64)`**, called from `wake_due_threads`.
  - **retrace-core:** the record arm `Stop::Syscall { num, args } if retrace_arch::is_psynch(num)`, the mirror `if retrace_arch::is_psynch(num) { … }` inside the `Syscall` chain after the arm-top `verify_thread`, each calling `guest_psynch(num, args)` exactly once, and `#[doc(hidden)] pub fn ReplaySession::dbg_psynch_mut(&mut self) -> &mut retrace_box::psynch::Psynch`.
  - **`retrace_guest::CONDVAR_DYN`**, modes `pingpong`, `broadcast`, `timedout`, `timedsignal`, `onens` and `mutex`.
  - **Refusal texts.** Every one starts `M48: psynch `: `M48: psynch <call> (<num>) is not modelled …`, `M48: psynch psynch_cvwait on cv <cv> with mutex <m> …`, `M48: psynch <call> on cv <cv> with flags <f> …`, `M48: psynch psynch_cvsignal on cv <cv> targets thread port <p> …`, `M48: psynch psynch_cvwait on cv <cv>: S <s> is not below L <l> …`, `M48: psynch <call> on cv <cv>: L …, U …, S … are out of order …`, `M48: psynch psynch_cvbroad on cv <cv> releases <n> waiters …`, `M48: psynch psynch_cvwait on cv <cv> by thread <t>: thread <u> already waits at sequence <q> …`, `M48: psynch psynch_cvwait timeout sec <s> …`. A woken thread's pending signal is `M48: a signal is pending on thread <tid> …` (T5-e). Replay wraps a refusal as `<call> refused on replay, though the recording accepted it — replay diverged before this landmark: <message>`, with `<call>` = `call_name(num)`; a mismatch starts `<call> rc mismatch: replay `.
  - **Pinned test names:** `the_sequence_window_wraps_as_synch_internal_h_computes`, `a_signal_across_the_sequence_wrap_wakes_the_waiter` (`psynch.rs` unit), `a_timed_wait_on_the_only_waiter_answers_on_the_vcpu`, `a_cvwait_refused_on_replay_is_a_divergence_naming_it_not_a_panic` and the seek test `a_seek_into_a_blocked_cvwait_matches_a_cold_seek` (`condvar_e2e`).

- [ ] **Step 1: Controller addendum.** Write `$L/task-5-addendum.md` from measurements §M4, pinning each value this task writes as `T0(M4)`, and cross-checking each against the port's arithmetic (`PTHRW_INC` 0x100, `CBIT` 1, `PBIT` 2, `MBIT` 0x40, `ECVCLEARED` 0x100, `ETIMEDOUT` 60):

  | Value | Expected (the port) | Where this task uses it |
  |---|---|---|
  | a woken waiter's `cvwait` word | `0`, carry clear (`psynch_cvcontinue`'s success branch) | unit tests 3–5, 7; box tests 1–2 |
  | the signaller's `cvsignal` with one waiter | `0x101`, carry clear (`ksyn_cvupdate_fixup` sets `CBIT` when the signal balances L and S; the probe's stub answered `0x100`) | unit tests 2, 3, 5, 7; box tests 1, 4; e2e 1, 4 |
  | the `cvbroad` to three | `0x301`, carry clear | unit test 4; box test 2; e2e 2 |
  | the `timeout` and `onens` errno | `0x13c` (`ETIMEDOUT \| ECVCLEARED`), carry set (plan F11) | unit test 6; e2e 3, 5 |
  | the `cvwait` flags word | `0xa0` | every test's argument builder |
  | the `c_seq` offset in `pthread_cond_t` | bytes 24–35 | `condvar_dyn.c`'s `CSEQ_OFF`; e2e 3 |

  Also pin from the walk census that only 303, 304 and 305 occurred, every `cvwait` had mutex 0, and no thread port was nonzero (T5-d refuses everything else), and that no `mutexwait`/`mutexdrop` fired natively in t0's probe modes.

  **If a measured word differs from the port's,** that is a Ruling, not an edit to a test: re-read the kernel path the port names, and either correct the port (a porting defect) or record why the measurement's shape differs from the test's. Never set a value to the probe stub's. If the `c_seq` offset differs, the addendum gives the measured one, and Step 8 uses it for `CSEQ_OFF` and Step 9's expected line. Do not start until the addendum exists.

- [ ] **Step 2: The port's unit tests, red first.** Add `pub mod psynch;` to `crates/retrace-box/src/lib.rs` on the line after `pub mod gkq;` (Task 4's). Create `crates/retrace-box/src/psynch.rs` containing only the test module below; Step 3 writes everything above it. Each test cites the kernel function whose answer it pins; the argument builders follow `pthread_cond.c` at the F1 tag (`_pthread_psynch_cond_wait`, `_pthread_psynch_cond_signal`).

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CV: u64 = 0x1_0000_8000;

    /// `cvwait`'s arguments as libpthread builds them: `cvlsgen` is S (with the bits it saved)
    /// over L after this waiter's increment, then U, mutex 0, mugen 0, node's flags 0xa0 (plan
    /// P4), and no timeout, which is the box's (`timeout_ticks`).
    fn wait(l: u32, s: u32, u: u32) -> [u64; 8] {
        [CV, ((s as u64) << 32) | l as u64, u as u64, 0, 0, 0xa0, 0, 0]
    }

    /// `cvsignal`'s: `cvlsgen` as above, U before the signal's own increment, no thread port, and
    /// the flags in x7.
    fn signal(l: u32, s: u32, u: u32) -> [u64; 8] {
        [CV, ((s as u64) << 32) | l as u64, u as u64, 0, 0, 0, 0, 0xa0]
    }

    /// `cvbroad`'s: `cvudgen` is the old U over the count of waiters being released.
    fn broad(l: u32, s: u32, u: u32, diffgen: u32) -> [u64; 8] {
        [CV, ((s as u64) << 32) | l as u64, ((u as u64) << 32) | diffgen as u64, 0xa0, 0, 0, 0, 0]
    }

    fn woke(tid: usize) -> Wake { Wake { tid, word: 0 } }

    /// Review Focus 3. synch_internal.h compares the count bits in a half window, so a word just
    /// past the 2^32 wrap is HIGHER than one just before it. A model comparing raw integers gets
    /// every row that crosses the wrap backwards.
    #[test]
    fn the_sequence_window_wraps_as_synch_internal_h_computes() {
        assert!(is_seqhigher(0x100, 0xffff_ff00) && is_seqlower(0xffff_ff00, 0x100), "one step across the wrap");
        assert!(!is_seqhigher(0xffff_ff00, 0x100) && !is_seqlower(0x100, 0xffff_ff00));
        assert!(!is_seqhigher_eq(0xffff_ff01, 0), "S just below the wrap is not at or above L just past it");
        assert_eq!(diff_genseq(0, 0xffff_ff00), PTHRW_INC, "L one increment past S, across the wrap");
        assert_eq!(diff_genseq(0x100, 0xffff_ff00), 0x200);
        assert_eq!(diff_genseq(0xffff_ff00, 0x100), 0xffff_fe00, "the other way is nearly the whole space");
        // The low byte is flag bits, not count.
        assert!(is_seqlower_eq(0x1ff, 0x100) && !is_seqhigher(0x1ff, 0x100));
        assert_eq!(diff_genseq(0x3ff, 0x101), 0x200);
        assert_eq!(diff_genseq(0x101, 0x1ff), 0, "equal counts with different bits are balanced");
        // The half window's edge: PTHRW_MAX_READERS / 2 is 0x7fff_ff80.
        assert!(is_seqhigher(0x7fff_ff00, 0), "inside the half window");
        assert!(!is_seqhigher(0x8000_0000, 0) && is_seqlower(0x8000_0000, 0), "past it, so lower");
    }

    /// Review Focus 3 through the port. A cv balanced just below the wrap (L = U = 0xffff_ff00, S the
    /// same with its C bit) takes one waiter, whose increment wraps L to 0, and one signal, whose
    /// increment wraps U to 0. The waiter is woken and the cv cleared exactly as for an unwrapped
    /// cv. A raw-integer model refuses the wait (S "above" L) or finds nobody to wake.
    #[test]
    fn a_signal_across_the_sequence_wrap_wakes_the_waiter() {
        let mut p = Psynch::default();
        assert_eq!(p.cvwait(wait(0, 0xffff_ff01, 0xffff_ff00), 1).unwrap(), Outcome { ret: Ret::Block, woken: vec![] });
        assert_eq!(p.kwq(CV).unwrap().queue, vec![Kwe { state: KweState::InWait, lockseq: 0, count: 1, thread: Some(1) }]);
        // libpthread stored S without its bits at the wait; the signal passes the old U.
        let s = p.cvsignal(signal(0, 0xffff_ff00, 0xffff_ff00)).unwrap();
        assert_eq!(s, Outcome { ret: Ret::Word(0x101), woken: vec![woke(1)] },
            "T0(M4): one waiter woken, and S, wrapping to 0, balances L, so the cv is cleared");
        assert!(p.is_empty(), "a cleared cv with an empty queue is freed (T5-c)");
    }

    /// `__psynch_cvsignal` on a fresh cv with one waiter.
    #[test]
    fn a_signal_wakes_the_one_waiter_and_answers_one_increment_with_the_c_bit() {
        let mut p = Psynch::default();
        // A fresh cv's S is 1 (its C bit, set at init), so the first waiter passes S = 1, L = 0x100.
        assert_eq!(p.cvwait(wait(0x100, 1, 0), 1).unwrap().ret, Ret::Block);
        assert_eq!(p.kwq(CV).unwrap().sword, 1, "a fresh queue takes the caller's S, bits included (ksyn_wqfind)");
        let s = p.cvsignal(signal(0x100, 0, 0)).unwrap();
        assert_eq!(s.ret, Ret::Word(0x101), "T0(M4): one increment, and the C bit because S now equals L");
        assert_eq!(s.woken, vec![woke(1)], "a signalled waiter returns 0 (psynch_cvcontinue)");
        assert!(p.is_empty());
    }

    /// `ksyn_handle_cvbroad`: every waiter up to L, in queue order.
    #[test]
    fn a_broadcast_wakes_every_waiter_in_sequence_order_and_answers_their_count() {
        let mut p = Psynch::default();
        for (tid, l, s) in [(1, 0x100, 1), (2, 0x200, 0), (3, 0x300, 0)] {
            assert_eq!(p.cvwait(wait(l, s, 0), tid).unwrap().ret, Ret::Block, "thread {tid}");
        }
        let b = p.cvbroad(broad(0x300, 0, 0, 0x300), 4).unwrap();
        assert_eq!(b.ret, Ret::Word(0x301), "T0(M4): three increments and the C bit");
        assert_eq!(b.woken, vec![woke(1), woke(2), woke(3)]);
        assert!(p.is_empty(), "the broadcast entry ksyn_handle_cvbroad queued is freed by the L == S fixup");
    }

    /// `ksyn_queue_find_signalseq` prefers the waiter at the signal's own sequence (U + 1), which
    /// is what keeps two waiters in arrival order.
    #[test]
    fn a_signal_wakes_the_lowest_sequence_waiter_first() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        p.cvwait(wait(0x200, 0, 0), 2).unwrap();
        assert_eq!(p.cvsignal(signal(0x200, 0, 0)).unwrap(), Outcome { ret: Ret::Word(0x100), woken: vec![woke(1)] },
            "the waiter at U + 1, and no C bit: thread 2 still waits");
        assert_eq!(p.kwq(CV).unwrap().sword, 0x101);
        // libpthread added the 0x100 to its S; the second signal advances U past the first.
        assert_eq!(p.cvsignal(signal(0x200, 0x100, 0x100)).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![woke(2)] });
        assert!(p.is_empty());
    }

    /// `psynch_cvcontinue`'s timeout branch for the only waiter.
    #[test]
    fn a_deadline_on_a_lone_waiter_answers_etimedout_with_ecvcleared() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        assert_eq!(p.time_out(CV, 1), TimedOut { word: 0x13c, woken: vec![] }, "T0(M4): ETIMEDOUT | ECVCLEARED (plan F11)");
        assert!(p.is_empty(), "its own count balanced L and S, so the cv is cleared and freed");
    }

    /// The same branch beside a second waiter: S counts the leaver, L stays ahead.
    #[test]
    fn a_deadline_beside_another_waiter_answers_plain_etimedout() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        p.cvwait(wait(0x200, 0, 0), 2).unwrap();
        assert_eq!(p.time_out(CV, 1), TimedOut { word: 60, woken: vec![] },
            "nothing cleared, and a real waiter remains, so no ECVPREPOST");
        assert_eq!(p.kwq(CV).unwrap().queue.len(), 1);
        // libpthread counted the timeout in its own S (_pthread_cond_updateval), so U catches up from S.
        assert_eq!(p.cvsignal(signal(0x200, 0x100, 0)).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![woke(2)] });
        assert!(p.is_empty());
    }

    /// `_ksyn_cvsignal_any` with no waiter at or below its sequence leaves a prepost, and the
    /// waiter it was meant for consumes it without blocking (`_psynch_cvwait`). Natively that waiter
    /// had incremented L and not yet entered the kernel; under the cooperative scheduler it follows
    /// a deadline wake whose thread has not yet run (T5-b).
    #[test]
    fn a_signal_with_no_waiter_preposts_and_the_next_wait_consumes_it() {
        let mut p = Psynch::default();
        assert_eq!(p.cvsignal(signal(0x100, 0, 0)).unwrap(), Outcome { ret: Ret::Word(PTH_RWS_CV_PBIT), woken: vec![] },
            "nothing woken, and only a fake entry queued: the P bit");
        assert_eq!(p.kwq(CV).unwrap().queue, vec![Kwe { state: KweState::Prepost, lockseq: 0x100, count: 1, thread: None }]);
        assert_eq!(p.cvwait(wait(0x100, 1, 0), 1).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![] },
            "consumed at once: no block");
        assert!(p.is_empty());
    }

    /// `_ksyn_cvsignal_any`'s starvation guard: the only waiter sits below the signal's sequence, so
    /// the signal becomes a broadcast. It wakes that waiter and queues a broadcast entry for the
    /// waiter still on its way, which then returns without blocking.
    #[test]
    fn a_signal_whose_only_waiter_is_below_its_sequence_becomes_a_broadcast() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        // U already counts the waiter at 0x100, and L counts a second that has not reached the kernel.
        assert_eq!(p.cvsignal(signal(0x200, 0, 0x100)).unwrap(),
            Outcome { ret: Ret::Word(PTHRW_INC | PTH_RWS_CV_PBIT), woken: vec![woke(1)] });
        assert_eq!(p.kwq(CV).unwrap().queue, vec![Kwe { state: KweState::Broadcast, lockseq: 0x200, count: 1, thread: None }]);
        assert_eq!(p.cvwait(wait(0x200, 0, 0), 2).unwrap(), Outcome { ret: Ret::Word(0x101), woken: vec![] },
            "the broadcast entry covers it");
        assert!(p.is_empty());
    }

    /// `_psynch_cvwait`'s timeout arithmetic (kern_synch.c:1270-1280) at 24 MHz (plan F6).
    #[test]
    fn the_timeout_decodes_as_kern_synch_does() {
        assert_eq!(timeout_ticks(0, 0), Ok(None), "pthread_cond_wait's {{0, 0}}: no deadline");
        assert_eq!(timeout_ticks(0, 1), Ok(Some(0)), "node's {{0, 1 ns}} is 0 ticks: a deadline already reached");
        assert_eq!(timeout_ticks(0, 5_000_000), Ok(Some(120_000)), "5 ms");
        assert_eq!(timeout_ticks(1, 986_000_000), Ok(Some(47_664_000)), "1.986 s");
        assert_eq!(timeout_ticks(0, 0x4000_0000), Ok(None), "nsec loses its top two bits before the zero test");
        assert_eq!(timeout_ticks(0, 0xffff_ffff_0000_0001), Ok(Some(0)), "the kernel's nsec is 32 bits");
        let e = timeout_ticks(u64::MAX, 0).unwrap_err();
        assert!(e.starts_with("M48: psynch ") && e.contains("sec -1"), "{e}");
    }

    /// R5 and T5-d: every shape no walk measured is refused by value, naming it, and the model is
    /// left as it was. One row per refusal, so deleting any one of them turns this test red.
    #[test]
    fn every_unmeasured_shape_is_refused_by_value_and_changes_nothing() {
        let mut p = Psynch::default();
        p.cvwait(wait(0x100, 1, 0), 1).unwrap();
        let before = p.clone();
        let mut mutexed = wait(0x200, 0, 0);
        mutexed[3] = 0x6000_1000;
        let mut shared = wait(0x200, 0, 0);
        shared[5] = 0x90; // PTHREAD_PROCESS_SHARED | firstfit
        let mut targeted = signal(0x100, 0, 0);
        targeted[3] = 0x1203;
        let cases: [(&str, Result<Outcome, String>); 7] = [
            ("with mutex 0x60001000", p.cvwait(mutexed, 2)),
            ("with flags 0x90", p.cvwait(shared, 2)),
            ("targets thread port 0x1203", p.cvsignal(targeted)),
            ("S 0x200 is not below L 0x200", p.cvwait(wait(0x200, 0x200, 0), 2)),
            ("S 0x300 are out of order", p.cvsignal(signal(0x100, 0x300, 0))),
            ("releases 32 waiters", p.cvbroad(broad(0x100, 0, 0, 0x2000), 4)),
            ("thread 1 already waits at sequence 0x100", p.cvwait(wait(0x100, 1, 0), 2)),
        ];
        for (want, got) in cases {
            let e = got.expect_err(want);
            assert!(e.starts_with("M48: psynch ") && e.contains(want), "{want}: {e}");
        }
        assert_eq!(p, before, "a refusal leaves the model untouched");
    }
}
```

  Run it red; nothing above the module exists yet:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --lib psynch -- --test-threads=1 > $L/t5-red.log 2>&1; echo "exit=$?"; grep -a -E '^error|cannot find' $L/t5-red.log | head
```

  Expected: a nonzero exit with `cannot find type `Psynch`` (and its siblings) in the log.

- [ ] **Step 3: The port.** Write the module above the tests in `crates/retrace-box/src/psynch.rs`:

```rust
//! M48 §3e (P1): psynch condition variables, ported from libpthread-539.100.4's `kern/kern_synch.c`
//! and `kern/synch_internal.h` (plan F1). On this host every guest condvar is psynch and every
//! guest mutex firstfit psynch (plan F2), so `pthread_cond_wait` blocks in `psynch_cvwait` (305) and
//! a signal that finds a waiter issues `psynch_cvsignal` (304) or `psynch_cvbroad` (303).
//!
//! Pure data with no `Box_` access, keyed by the guest cv ADDRESS, the correlation the ulock pair
//! uses. `Box_` owns one (`Box_::psynch`), carries it through every rebuild path in `BoxState`, and
//! does what needs the box: the deadline, blocking the caller, and writing each woken thread's word
//! (`Box_::guest_psynch`, `Box_::cv_timed_out`).
//!
//! **Ported, not invented.** Each function names the kernel function it ports. Left out:
//! - `kw_cvkernelseq`, `kw_lowseq` and `kw_highseq`, which the cv paths write and never read;
//! - `kw_prepost` and `kw_intr`, which belong to the mutex and rwlock paths;
//! - cancellation (`__pthread_testcancel`, the queue scans' cancelled-thread skips): retrace models
//!   none, so no waiter is ever cancelled.
//!
//! A woken waiter's continuation (`psynch_cvcontinue`) runs at its wake (T5-b), and a cv's queue is
//! freed when it empties (T5-c). Every shape no walk measured is refused by value, before anything
//! changes, with an `Err` that starts `M48: psynch ` (R5, T5-d). The box panics with it on record,
//! and replay reports it as a `Divergence`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use retrace_arch::{ECVCLEARED, ECVPREPOST, ETIMEDOUT, PTHRW_COUNT_MASK, PTHRW_COUNT_SHIFT, PTHRW_INC,
                   PTHRW_MAX_READERS, PTH_RWL_MTX_WAIT, PTH_RWS_CV_CBIT, PTH_RWS_CV_MBIT, PTH_RWS_CV_PBIT};

/// `PTHREAD_PSHARED_FLAGS_MASK` and `PTHREAD_PROCESS_SHARED` (kern_synch.c:187-189). A shared cv is
/// keyed by its VM object (`ksyn_findobj`), not its address.
const PSHARED_MASK: u32 = 0x30;
const PSHARED: u32 = 0x10;
/// `_psynch_cvwait` masks `nsec` before testing it (kern_synch.c:1273).
const NSEC_MASK: u32 = 0x3fff_ffff;

/// `is_seqlower` (synch_internal.h:97). Sequence words count in units of `PTHRW_INC` and wrap at
/// 2^32, so order is decided in a half window, and the low byte, which holds flag bits, is ignored.
pub fn is_seqlower(x: u32, y: u32) -> bool {
    let (x, y) = (x & PTHRW_COUNT_MASK, y & PTHRW_COUNT_MASK);
    if x < y { y - x < PTHRW_MAX_READERS / 2 } else { x - y > PTHRW_MAX_READERS / 2 }
}

/// `is_seqlower_eq` (synch_internal.h:109).
pub fn is_seqlower_eq(x: u32, y: u32) -> bool {
    x & PTHRW_COUNT_MASK == y & PTHRW_COUNT_MASK || is_seqlower(x, y)
}

/// `is_seqhigher` (synch_internal.h:119).
pub fn is_seqhigher(x: u32, y: u32) -> bool {
    let (x, y) = (x & PTHRW_COUNT_MASK, y & PTHRW_COUNT_MASK);
    if x > y { x - y < PTHRW_MAX_READERS / 2 } else { y - x > PTHRW_MAX_READERS / 2 }
}

/// `is_seqhigher_eq` (synch_internal.h:131).
pub fn is_seqhigher_eq(x: u32, y: u32) -> bool {
    x & PTHRW_COUNT_MASK == y & PTHRW_COUNT_MASK || is_seqhigher(x, y)
}

/// `diff_genseq` (synch_internal.h:141): how far `x` is ahead of `y`, across the wrap.
pub fn diff_genseq(x: u32, y: u32) -> u32 {
    let (x, y) = (x & PTHRW_COUNT_MASK, y & PTHRW_COUNT_MASK);
    match x.cmp(&y) {
        Ordering::Equal => 0,
        Ordering::Greater => x - y,
        Ordering::Less => (PTHRW_MAX_READERS - y) + x + PTHRW_INC,
    }
}

/// `_psynch_cvwait`'s timeout (kern_synch.c:1270-1280) in guest-clock ticks, or `None` to wait
/// without one, which is `pthread_cond_wait`'s `{0, 0}`. The kernel's `nsec` is 32 bits and loses its
/// top two before the zero test. At 24 MHz (plan F6) a nanosecond count is `ns * 3 / 125` ticks,
/// truncated, so node's `{0, 1}` is 0 ticks: a deadline already reached, which blocks and wakes in
/// the same schedule (Global Constraints). The box adds the clock at the call (T5-g).
pub fn timeout_ticks(sec: u64, nsec: u64) -> Result<Option<u64>, String> {
    let nsec = (nsec as u32) & NSEC_MASK;
    if sec == 0 && nsec == 0 {
        return Ok(None);
    }
    let ns = ((sec as i64) >= 0).then_some(sec)
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|n| n.checked_add(nsec as u64))
        .ok_or_else(|| format!("M48: psynch psynch_cvwait timeout sec {} nsec {nsec:#x}: negative or past \
                                2^64 ns, which no guest was measured to pass", sec as i64))?;
    Ok(Some((ns as u128 * 3 / 125) as u64))
}

/// The SDK's name for psynch syscall `num` (`sys/syscall.h`), for refusals and divergences.
pub fn call_name(num: u64) -> &'static str {
    match num {
        297 => "psynch_rw_longrdlock",
        298 => "psynch_rw_yieldwrlock",
        299 => "psynch_rw_downgrade",
        300 => "psynch_rw_upgrade",
        301 => "psynch_mutexwait",
        302 => "psynch_mutexdrop",
        303 => "psynch_cvbroad",
        304 => "psynch_cvsignal",
        305 => "psynch_cvwait",
        306 => "psynch_rw_rdlock",
        307 => "psynch_rw_wrlock",
        308 => "psynch_rw_unlock",
        309 => "psynch_rw_unlock2",
        312 => "psynch_cvclrprepost",
        _ => "psynch",
    }
}

/// `kwe_state` (synch_internal.h:28).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KweState {
    /// `KWE_THREAD_INWAIT`: a blocked `cvwait`.
    InWait,
    /// `KWE_THREAD_PREPOST`: a signal that found no waiter at or below its sequence.
    Prepost,
    /// `KWE_THREAD_BROADCAST`: a broadcast's claim on waiters not yet in the kernel.
    Broadcast,
}

/// One `ksyn_waitq_element` (kern_internal.h:141), for the fields a cv reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kwe {
    pub state: KweState,
    /// `kwe_lockseq`, count bits only, as every cv path stores it.
    pub lockseq: u32,
    /// `kwe_count`: the signals a prepost still owes.
    pub count: u32,
    /// `kwe_thread` as a thread-table index; `None` for a prepost or a broadcast entry.
    pub thread: Option<usize>,
}

/// One cv's `ksyn_wait_queue` (kern_synch.c:125), for the fields its paths read. A cv uses only
/// `kw_ksynqueues[KSYN_QUEUE_WRITE]`, so there is one queue, kept in TAILQ order, which `SEQFIT`
/// keeps in sequence order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kwq {
    pub lword: u32,
    pub uword: u32,
    pub sword: u32,
    /// `KSYN_KWF_ZEROEDOUT`: L, U and S were cleared at an L == S transition, so the next call's
    /// words replace them outright (`UPDATE_CVKWQ`).
    pub zeroed_out: bool,
    pub queue: Vec<Kwe>,
    /// `kw_fakecount`: the prepost and broadcast entries in `queue`.
    pub fakecount: u32,
}

/// A woken waiter and the word its `cvwait` returns, with carry clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wake {
    pub tid: usize,
    pub word: u32,
}

/// What a call answers its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ret {
    /// The call returns this word at once, with carry clear.
    Word(u32),
    /// The caller blocks (`cvwait` only); its word comes at its wake.
    Block,
}

/// One call's answer, and the waiters it woke, in the order the kernel woke them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub ret: Ret,
    pub woken: Vec<Wake>,
}

/// A deadline wake: the timed-out waiter's errno word, delivered with carry set, and any waiter the
/// balancing L == S released with it. The word is `u64`, as `ETIMEDOUT`, `ECVCLEARED` and
/// `ECVPREPOST` are (Task 2): an errno the stub's caller reads whole (plan F11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimedOut {
    pub word: u64,
    pub woken: Vec<Wake>,
}

/// `psynch_cvcontinue`'s success branch (kern_synch.c:1349-1358): a signalled waiter returns 0,
/// unless the kernel woke it while freeing the queue (`PTH_RWS_CV_MBIT`), when it returns one
/// increment with the C bit.
fn woken_word(psynchretval: u32) -> u32 {
    if psynchretval & PTH_RWS_CV_MBIT != 0 { PTHRW_INC | PTH_RWS_CV_CBIT } else { 0 }
}

fn refuse_shared(call: &str, cv: u64, flags: u32) -> Result<(), String> {
    if flags & PSHARED_MASK == PSHARED {
        return Err(format!("M48: psynch {call} on cv {cv:#x} with flags {flags:#x}: a process-shared cv is keyed \
                            by its VM object (ksyn_findobj), not its address; every measured flags word is 0xa0 \
                            (plan P4)"));
    }
    Ok(())
}

impl Kwq {
    /// `ksyn_wqfind`'s first use of an address (kern_synch.c:1799-1815): L, U and S from the call.
    fn new(lword: u32, uword: u32, sword: u32) -> Kwq {
        Kwq { lword, uword, sword, ..Kwq::default() }
    }

    /// `UPDATE_CVKWQ` (kern_synch.c:290). Its `kw_cvkernelseq` write is dropped: no cv path reads it.
    fn update(&mut self, mgen: u32, ugen: u32, rw_wc: u32) {
        let sinit = rw_wc & PTH_RWS_CV_CBIT != 0;
        if self.zeroed_out {
            (self.lword, self.uword, self.sword, self.zeroed_out) = (mgen, ugen, rw_wc, false);
        } else {
            if is_seqhigher(mgen, self.lword) { self.lword = mgen; }
            if is_seqhigher(ugen, self.uword) { self.uword = ugen; }
            if sinit && is_seqhigher(rw_wc, self.sword) { self.sword = rw_wc; }
        }
    }

    /// The L == S clearing `ksyn_cvupdate_fixup` and the timeout branch share.
    fn clear(&mut self) {
        (self.lword, self.uword, self.sword, self.zeroed_out) = (0, 0, 0, true);
    }

    /// `ksyn_queue_insert` with `SEQFIT` (kern_synch.c:2358), the only fit a cv uses. A second entry
    /// at the first or last sequence is the kernel's `EBUSY`, and a gap it cannot place its `ESRCH`.
    fn insert(&mut self, kwe: Kwe) -> Result<(), String> {
        let seq = kwe.lockseq;
        let at = match (self.queue.first(), self.queue.last()) {
            (Some(f), Some(l)) if seq == f.lockseq || seq == l.lockseq => {
                return Err(format!("an entry at sequence {seq:#x} is already queued (ksyn_queue_insert's EBUSY)"));
            }
            (Some(f), Some(l)) => {
                if is_seqlower(l.lockseq, seq) {
                    self.queue.len()
                } else if is_seqlower(seq, f.lockseq) {
                    0
                } else {
                    self.queue.iter().position(|q| is_seqhigher(q.lockseq, seq)).ok_or_else(|| format!(
                        "no entry above sequence {seq:#x} to insert before (ksyn_queue_insert's ESRCH)"))?
                }
            }
            _ => 0,
        };
        self.queue.insert(at, kwe);
        Ok(())
    }

    /// `ksyn_prepost` (kern_synch.c:907): a fake entry, counted in `fakecount`. The kernel ignores a
    /// failed insert; here it is refused, since the entry would be counted but never queued.
    fn prepost(&mut self, state: KweState, lockseq: u32) -> Result<(), String> {
        self.insert(Kwe { state, lockseq, count: 1, thread: None })?;
        self.fakecount += 1;
        Ok(())
    }

    /// `ksyn_queue_find_cvpreposeq` (kern_synch.c:2482): the first entry at or above `lockseq`,
    /// unless it is a waiter at another sequence.
    fn find_cvpreposeq(&self, lockseq: u32) -> Option<usize> {
        let i = self.queue.iter().position(|k| is_seqhigher_eq(k.lockseq, lockseq))?;
        let k = self.queue[i];
        (k.state != KweState::InWait || k.lockseq == lockseq).then_some(i)
    }

    /// `ksyn_queue_find_signalseq` (kern_synch.c:2505): a prepost or broadcast at or above
    /// `uptoseq`, else the waiter at or above `signalseq`, else the first waiter at or below
    /// `uptoseq`.
    fn find_signalseq(&self, uptoseq: u32, signalseq: u32) -> Option<usize> {
        let mut result = None;
        for (i, q) in self.queue.iter().enumerate() {
            match q.state {
                KweState::Prepost if is_seqhigher(q.lockseq, uptoseq) => return result,
                KweState::Prepost | KweState::Broadcast => {
                    if !is_seqlower(q.lockseq, uptoseq) {
                        return Some(i);
                    }
                }
                KweState::InWait => {
                    if is_seqhigher(q.lockseq, uptoseq) {
                        return result;
                    }
                    if is_seqhigher_eq(q.lockseq, signalseq) {
                        return Some(i);
                    }
                    result = result.or(Some(i));
                }
            }
        }
        result
    }

    /// `_ksyn_cvsignal_any` (kern_synch.c:920).
    fn signal_any(&mut self, uptoseq: u32, signalseq: u32, updatebits: &mut u32, broadcast: &mut bool,
                  woken: &mut Vec<Wake>) -> Result<(), String> {
        let Some(i) = self.find_signalseq(uptoseq, signalseq) else {
            return self.prepost(KweState::Prepost, uptoseq);
        };
        match self.queue[i].state {
            // A waiter below the signal's own sequence: matching it could leave the waiter the signal
            // was meant for with nobody to wake it, so the kernel converts to a broadcast
            // (kern_synch.c:950-960).
            KweState::InWait if is_seqlower(self.queue[i].lockseq, signalseq) => *broadcast = true,
            KweState::InWait => {
                let kwe = self.queue.remove(i);
                woken.push(Wake { tid: kwe.thread.expect("an InWait entry names its thread"), word: woken_word(PTH_RWL_MTX_WAIT) });
                *updatebits += PTHRW_INC;
            }
            KweState::Prepost => self.queue[i].count += 1,
            KweState::Broadcast => {}
        }
        Ok(())
    }

    /// `ksyn_handle_cvbroad` (kern_synch.c:2714): wake every waiter up to `upto` and drop every fake
    /// entry there; then, unless L == S, queue a broadcast entry for waiters not yet in the kernel.
    /// S is read before the caller adds this call's count, as in the kernel.
    fn broadcast(&mut self, upto: u32, updatebits: &mut u32, woken: &mut Vec<Wake>) -> Result<(), String> {
        let mut bits = 0;
        let mut i = 0;
        while i < self.queue.len() {
            let kwe = self.queue[i];
            if is_seqhigher(kwe.lockseq, upto) {
                break;
            }
            self.queue.remove(i);
            match kwe.state {
                KweState::InWait => {
                    woken.push(Wake { tid: kwe.thread.expect("an InWait entry names its thread"), word: woken_word(PTH_RWL_MTX_WAIT) });
                    bits += PTHRW_INC;
                }
                KweState::Prepost | KweState::Broadcast => self.fakecount -= 1,
            }
        }
        if diff_genseq(self.lword, self.sword) != 0 {
            self.prepost(KweState::Broadcast, upto)?;
        }
        *updatebits |= bits;
        Ok(())
    }

    /// `ksyn_queue_free_items` (kern_synch.c:2556): from the head, up to `upto` unless `all`. A
    /// waiter is woken as freed (`PTHRW_INC | PTH_RWS_CV_MBIT | PTH_RWL_MTX_WAIT`); a fake entry is
    /// dropped.
    fn free_items(&mut self, upto: u32, all: bool, woken: &mut Vec<Wake>) {
        while let Some(&kwe) = self.queue.first() {
            if !all && is_seqhigher(kwe.lockseq, upto) {
                break;
            }
            self.queue.remove(0);
            match kwe.state {
                KweState::InWait => woken.push(Wake {
                    tid: kwe.thread.expect("an InWait entry names its thread"),
                    word: woken_word(PTHRW_INC | PTH_RWS_CV_MBIT | PTH_RWL_MTX_WAIT),
                }),
                KweState::Prepost | KweState::Broadcast => self.fakecount -= 1,
            }
        }
    }

    /// `ksyn_cvupdate_fixup` (kern_synch.c:2787): at L == S, free the queue up to L, clear the words
    /// and answer the C bit; with only fake entries left, answer the P bit.
    fn fixup(&mut self, updatebits: &mut u32, woken: &mut Vec<Wake>) {
        if self.lword & PTHRW_COUNT_MASK == self.sword & PTHRW_COUNT_MASK {
            if !self.queue.is_empty() {
                let l = self.lword;
                self.free_items(l, false, woken);
            }
            self.clear();
            *updatebits |= PTH_RWS_CV_CBIT;
        } else if !self.queue.is_empty() && self.fakecount as usize == self.queue.len() {
            *updatebits |= PTH_RWS_CV_PBIT;
        }
    }
}

/// Every cv with a nonempty queue, by guest address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Psynch {
    kwqs: BTreeMap<u64, Kwq>,
}

impl Psynch {
    pub fn is_empty(&self) -> bool { self.kwqs.is_empty() }

    pub fn kwq(&self, cv: u64) -> Option<&Kwq> { self.kwqs.get(&cv) }

    /// `ksyn_wqfind`'s lookup, or a fresh queue from the call's words. A copy: every operation
    /// works on it and stores it back only on success, so a refusal changes nothing.
    fn find(&self, cv: u64, lword: u32, uword: u32, sword: u32) -> Kwq {
        self.kwqs.get(&cv).cloned().unwrap_or_else(|| Kwq::new(lword, uword, sword))
    }

    /// `ksyn_wqrelease` with `qfreenow` (kern_synch.c:1835): an empty queue is freed (T5-c).
    fn store(&mut self, cv: u64, kwq: Kwq) {
        if kwq.queue.is_empty() {
            self.kwqs.remove(&cv);
        } else {
            self.kwqs.insert(cv, kwq);
        }
    }

    /// `_psynch_cvwait` (kern_synch.c:1171) for thread `tid`. `args` is the syscall's: `cv, cvlsgen,
    /// cvugen, mutex, mugen, flags, sec, nsec`; the timeout is the box's (`timeout_ticks`).
    pub fn cvwait(&mut self, args: [u64; 8], tid: usize) -> Result<Outcome, String> {
        let (cv, cvugen, mutex, flags) = (args[0], args[2] as u32, args[3], args[5] as u32);
        let (csgen, cgen) = ((args[1] >> 32) as u32, args[1] as u32);
        refuse_shared("psynch_cvwait", cv, flags)?;
        if mutex != 0 {
            return Err(format!("M48: psynch psynch_cvwait on cv {cv:#x} with mutex {mutex:#x} (mugen {:#x}): the \
                                cv would drop a firstfit mutex that has a kernel waiter (plan F3), which needs the \
                                unmodelled psynch_mutexwait/psynch_mutexdrop pair; every measured cvwait passes \
                                mutex 0 (plan P4)", args[4]));
        }
        let lockseq = cgen & PTHRW_COUNT_MASK;
        if is_seqhigher_eq(csgen, lockseq) {
            return Err(format!("M48: psynch psynch_cvwait on cv {cv:#x}: S {csgen:#x} is not below L {cgen:#x}, \
                                which _psynch_cvwait answers EINVAL; no guest was measured to pass it"));
        }
        let mut kwq = self.find(cv, cgen, cvugen, csgen);
        kwq.update(cgen, cvugen, csgen);
        let mut woken = Vec::new();
        let ret = match kwq.find_cvpreposeq(lockseq) {
            None => {
                kwq.insert(Kwe { state: KweState::InWait, lockseq, count: 1, thread: Some(tid) })
                    .map_err(|e| format!("M48: psynch psynch_cvwait on cv {cv:#x} by thread {tid}: {e}"))?;
                Ret::Block
            }
            Some(i) => {
                let mut updatebits = 0;
                let kwe = kwq.queue[i];
                match kwe.state {
                    KweState::InWait => return Err(format!(
                        "M48: psynch psynch_cvwait on cv {cv:#x} by thread {tid}: thread {} already waits at \
                         sequence {lockseq:#x}, which _psynch_cvwait answers EBUSY",
                        kwe.thread.expect("an InWait entry names its thread"))),
                    // A prepost at our own sequence: consume one of its references.
                    KweState::Prepost if kwe.lockseq == lockseq => {
                        kwq.queue[i].count -= 1;
                        if kwq.queue[i].count == 0 {
                            kwq.queue.remove(i);
                            kwq.fakecount -= 1;
                        }
                    }
                    // A prepost above our sequence can leave its own waiter unmatched, so the kernel
                    // converts it to a broadcast (kern_synch.c:1233-1244).
                    KweState::Prepost => kwq.broadcast(kwe.lockseq, &mut updatebits, &mut woken)
                        .map_err(|e| format!("M48: psynch psynch_cvwait on cv {cv:#x} by thread {tid}: {e}"))?,
                    KweState::Broadcast => {}
                }
                updatebits |= PTHRW_INC;
                kwq.sword = kwq.sword.wrapping_add(PTHRW_INC);
                kwq.fixup(&mut updatebits, &mut woken);
                Ret::Word(updatebits)
            }
        };
        self.store(cv, kwq);
        Ok(Outcome { ret, woken })
    }

    /// `_psynch_cvsignal` (kern_synch.c:1156). `args`: `cv, cvlsgen, cvugen, thread_port, mutex,
    /// mugen, tid, flags`.
    pub fn cvsignal(&mut self, args: [u64; 8]) -> Result<Outcome, String> {
        let (cv, port, flags) = (args[0], args[3] as u32, args[7] as u32);
        refuse_shared("psynch_cvsignal", cv, flags)?;
        if port != 0 {
            return Err(format!("M48: psynch psynch_cvsignal on cv {cv:#x} targets thread port {port:#x} \
                                (pthread_cond_signal_thread_np): the targeted form was never measured (plan P4)"));
        }
        self.signal("psynch_cvsignal", cv, args[1] as u32, args[2] as u32, (args[1] >> 32) as u32, false)
    }

    /// `_psynch_cvbroad` (kern_synch.c:1134). `args`: `cv, cvlsgen, cvudgen, flags, mutex, mugen,
    /// tid`, where `cvudgen` is the old U over the count of waiters being released.
    pub fn cvbroad(&mut self, args: [u64; 8], nthreads: usize) -> Result<Outcome, String> {
        let (cv, flags) = (args[0], args[3] as u32);
        refuse_shared("psynch_cvbroad", cv, flags)?;
        let count = (args[2] as u32) >> PTHRW_COUNT_SHIFT;
        // The kernel's bound is `get_task_threadmax()`, a host value retrace does not model. A
        // guest's count is its unreleased waiters, fewer than its threads, so the guest's own thread
        // count refuses only a count no guest can produce (T5-d).
        if count as usize > nthreads {
            return Err(format!("M48: psynch psynch_cvbroad on cv {cv:#x} releases {count} waiters, more than the \
                                guest's {nthreads} threads (the kernel answers EBUSY above task_threadmax)"));
        }
        self.signal("psynch_cvbroad", cv, args[1] as u32, (args[2] >> 32) as u32, (args[1] >> 32) as u32, true)
    }

    /// `__psynch_cvsignal` (kern_synch.c:1054) with no thread port.
    fn signal(&mut self, call: &str, cv: u64, cgen: u32, cugen: u32, csgen: u32, mut broadcast: bool)
              -> Result<Outcome, String> {
        let uptoseq = cgen & PTHRW_COUNT_MASK;
        let fromseq = (cugen & PTHRW_COUNT_MASK).wrapping_add(PTHRW_INC);
        if is_seqhigher(fromseq, uptoseq) || is_seqhigher(csgen, uptoseq) {
            return Err(format!("M48: psynch {call} on cv {cv:#x}: L {cgen:#x}, U {cugen:#x}, S {csgen:#x} are out \
                                of order, which __psynch_cvsignal answers EINVAL; no guest was measured to pass them"));
        }
        let at = |e: String| format!("M48: psynch {call} on cv {cv:#x}: {e}");
        let mut kwq = self.find(cv, cgen, cugen, csgen);
        kwq.update(cgen, cugen, csgen);
        let (mut updatebits, mut woken) = (0, Vec::new());
        // "No need to signal if the CV is already balanced" (kern_synch.c:1092).
        if !broadcast && diff_genseq(kwq.lword, kwq.sword) != 0 {
            kwq.signal_any(uptoseq, fromseq, &mut updatebits, &mut broadcast, &mut woken).map_err(&at)?;
        }
        if broadcast {
            kwq.broadcast(uptoseq, &mut updatebits, &mut woken).map_err(&at)?;
        }
        kwq.sword = kwq.sword.wrapping_add(updatebits & PTHRW_COUNT_MASK);
        kwq.fixup(&mut updatebits, &mut woken);
        self.store(cv, kwq);
        Ok(Outcome { ret: Ret::Word(updatebits), woken })
    }

    /// `psynch_cvcontinue`'s timeout branch (kern_synch.c:1309-1347) for `tid`'s deadline. The
    /// waiter leaves the queue unsignalled, so it counts itself in S. When that balances L and S the
    /// cv is cleared and the errno carries `ECVCLEARED`; when only fake entries remain it carries
    /// `ECVPREPOST`. libpthread reads both bits (plan F11).
    ///
    /// Panics if `tid` does not wait on `cv`: `wake_due_threads` reaches here only for a thread
    /// blocked on `cv` with a deadline, so that is a box defect, not a guest shape.
    pub fn time_out(&mut self, cv: u64, tid: usize) -> TimedOut {
        let mut kwq = self.kwqs.get(&cv).cloned()
            .unwrap_or_else(|| panic!("M48: psynch deadline of thread {tid} on cv {cv:#x}, which has no queue"));
        let i = kwq.queue.iter().position(|k| k.state == KweState::InWait && k.thread == Some(tid))
            .unwrap_or_else(|| panic!("M48: psynch deadline of thread {tid} on cv {cv:#x}, where it does not wait: {kwq:?}"));
        kwq.queue.remove(i);
        let (mut word, mut woken) = (ETIMEDOUT, Vec::new());
        kwq.sword = kwq.sword.wrapping_add(PTHRW_INC);
        if kwq.lword & PTHRW_COUNT_MASK == kwq.sword & PTHRW_COUNT_MASK {
            word |= ECVCLEARED;
            if !kwq.queue.is_empty() {
                let l = kwq.lword;
                kwq.free_items(l, true, &mut woken);
            }
            kwq.clear();
        } else if !kwq.queue.is_empty() && kwq.fakecount as usize == kwq.queue.len() {
            word |= ECVPREPOST;
        }
        self.store(cv, kwq);
        TimedOut { word, woken }
    }

    /// Test-only (Review Focus 5, T5-f): plant a waiter on `cv` at `lockseq`, the state an earlier
    /// silent divergence would leave, so a test can make replay refuse a call the recording accepted.
    #[doc(hidden)]
    pub fn dbg_plant_waiter(&mut self, cv: u64, lockseq: u32, tid: usize) {
        let lockseq = lockseq & PTHRW_COUNT_MASK;
        let kwq = self.kwqs.entry(cv).or_insert_with(|| Kwq::new(lockseq, 0, 0));
        kwq.insert(Kwe { state: KweState::InWait, lockseq, count: 1, thread: Some(tid) })
            .expect("a planted waiter must fit the queue");
    }
}
```

  Run the module green:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --lib psynch -- --test-threads=1 > $L/t5-unit.log 2>&1; echo "exit=$?"; grep -a -E 'test result|FAILED|panicked' $L/t5-unit.log
```

  Expected: `exit=0` and `test result: ok. 11 passed` for the `psynch::tests` filter. A failing arithmetic row is a porting defect: re-read the cited kernel lines, never adjust the expected value.

- [ ] **Step 4: `BlockReason::Cv`.** In `crates/retrace-box/src/thread.rs`:
  1. Add the variant directly after Task 4's `Kevent`:

```rust
    /// M48 §3e: blocked in `psynch_cvwait` on the condition variable at guest address `addr`. The
    /// key is the address for the reason `Wait`'s is: the kernel keys a cv's wait queue by it, and
    /// so does `psynch.rs`. Woken by a `psynch_cvsignal` or `psynch_cvbroad` the port says reaches
    /// it (`Box_::guest_psynch`), or at `deadline`, a guest-clock value in `Kevent`'s domain, which
    /// is `None` for an untimed wait (`Box_::cv_timed_out`).
    Cv { addr: u64, deadline: Option<u64> },
```

  2. In `BlockReason::deadline`, whose match names every variant (Task 4), give `Cv` the `Kevent` arm, so its body reads:

```rust
        match *self {
            BlockReason::Kevent { deadline, .. } | BlockReason::Cv { deadline, .. } => deadline,
            BlockReason::Join { .. } | BlockReason::Wait { .. } | BlockReason::Sem { .. }
            | BlockReason::Parked => None,
        }
```

  3. In `ThreadTable::wake`, widen the state assert's pattern from `ThreadState::Blocked(BlockReason::Kevent { .. })` to `ThreadState::Blocked(BlockReason::Kevent { .. } | BlockReason::Cv { .. })`. Its message ("not blocked in a timed-wait primitive") and its pending-signal refusal, whose text already names `psynch_cvwait`, stay as Task 4 wrote them.

  `BlockReason` stays `Copy + Eq`: both fields are.

- [ ] **Step 5: The box-level tests, red first.** Create `crates/retrace-box/tests/psynch.rs`:

```rust
//! M48 Task 5, box level: `Box_::guest_psynch` on a static box, with the threads a guest would run
//! switched in by hand (`settle_schedule`), the way `kqmanager.rs` drives the workqueue. The port's
//! arithmetic is `psynch.rs`'s unit tests; these pin what the box adds: who blocks, whose saved
//! context a wake writes, that a refusal changes nothing, and the checkpoint carry. No wait here is
//! timed, because a deadline reads the guest clock and a static box has no commpage; `condvar_e2e`
//! covers deadlines end to end.
use retrace_arch::{PSTATE_C, SYS_PSYNCH_CVBROAD, SYS_PSYNCH_CVSIGNAL, SYS_PSYNCH_CVWAIT};
use retrace_box::thread::{BlockReason, ThreadCtx, ThreadState};
use retrace_box::Box_;

const CV: u64 = 0x1_0000_8000;

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SPINLOOP).unwrap());
    Box_::load(&loaded)
}

/// A static box with `n` threads beside main, spawned as `threads.rs` spawns them.
fn with_threads(n: u64) -> Box_ {
    let mut b = tb();
    for k in 0..n {
        let ctx = ThreadCtx { elr: 0x2000 + k * 0x100, ..ThreadCtx::zeroed() };
        b.threads_mut().spawn(ctx, (0x3020_0000 + k * 0x10_0000, 0x8000));
    }
    b
}

/// `cvwait`'s arguments for an untimed wait with mutex 0 and node's flags (`psynch.rs`'s builder).
fn wait(l: u32, s: u32) -> [u64; 8] { [CV, ((s as u64) << 32) | l as u64, 0, 0, 0, 0xa0, 0, 0] }

/// `cvsignal`'s, with no thread port.
fn signal(l: u32, s: u32, u: u32) -> [u64; 8] { [CV, ((s as u64) << 32) | l as u64, u as u64, 0, 0, 0, 0, 0xa0] }

/// The current thread waits and blocks, as the record arm drives it: the call, then
/// `set_x0_err_and_return(0, false)`, then the switch `run()` makes on its next entry.
fn block_in_cvwait(b: &mut Box_, l: u32, s: u32) {
    let tid = b.threads().current();
    assert_eq!(b.guest_psynch(SYS_PSYNCH_CVWAIT, wait(l, s)), Ok(0));
    b.set_x0_err_and_return(0, false);
    assert_eq!(b.threads().state_of(tid), ThreadState::Blocked(BlockReason::Cv { addr: CV, deadline: None }));
    b.settle_schedule();
}

/// A saved context that already reads 0 with carry clear cannot show whether a wake wrote it, so
/// give it a stale word and carry first.
fn stale(b: &mut Box_, tid: usize) {
    let c = b.threads_mut().ctx_mut(tid);
    c.regs.x[0] = 0xdead;
    c.regs.cpsr |= PSTATE_C;
}

#[test]
fn a_cvsignal_wakes_the_blocked_waiter_and_writes_its_saved_context() {
    let mut b = with_threads(1);
    block_in_cvwait(&mut b, 0x100, 1);
    assert_eq!(b.threads().current(), 1, "the waiter blocked, so the other thread runs");
    stale(&mut b, 0);
    assert_eq!(b.guest_psynch(SYS_PSYNCH_CVSIGNAL, signal(0x100, 0, 0)), Ok(0x101), "T0(M4): the signaller's word");
    assert_eq!(b.threads().state_of(0), ThreadState::Runnable);
    let c = b.threads().ctx_of(0);
    assert_eq!((c.regs.x[0], c.regs.cpsr & PSTATE_C), (0, 0), "the woken waiter reads 0 with carry clear (deliver_wake)");
    assert!(b.dbg_psynch().is_empty(), "L == S clears and frees the cv");
    assert_eq!(b.threads().current(), 1, "a wake does not switch: the signaller keeps the vCPU");
}

#[test]
fn a_cvbroad_wakes_every_waiter_and_each_reads_its_word() {
    let mut b = with_threads(3);
    block_in_cvwait(&mut b, 0x100, 1);
    block_in_cvwait(&mut b, 0x200, 0);
    block_in_cvwait(&mut b, 0x300, 0);
    assert_eq!(b.threads().current(), 3);
    for t in 0..3 { stale(&mut b, t); }
    // cvlsgen: S 0 over L 0x300; cvudgen: the old U 0 over the three being released.
    assert_eq!(b.guest_psynch(SYS_PSYNCH_CVBROAD, [CV, 0x300, 0x300, 0xa0, 0, 0, 0, 0]), Ok(0x301),
        "T0(M4): the broadcaster's word");
    for t in 0..3 {
        assert_eq!(b.threads().state_of(t), ThreadState::Runnable, "thread {t}");
        let c = b.threads().ctx_of(t);
        assert_eq!((c.regs.x[0], c.regs.cpsr & PSTATE_C), (0, 0), "thread {t}'s saved context");
    }
    assert!(b.dbg_psynch().is_empty());
}

/// T5-d and T5-e at the box: the numbers the port does not model, a port refusal reached through
/// the box, and a woken thread's pending signal are each refused by value with nothing changed.
#[test]
fn every_unmodelled_psynch_call_is_refused_by_value_with_nothing_changed() {
    let mut b = with_threads(1);
    block_in_cvwait(&mut b, 0x100, 1);
    let model = b.dbg_psynch().clone();
    for (num, name) in [(301, "psynch_mutexwait"), (302, "psynch_mutexdrop"), (312, "psynch_cvclrprepost"),
                        (306, "psynch_rw_rdlock"), (297, "psynch_rw_longrdlock")] {
        let e = b.guest_psynch(num, [CV, 0x100, 0, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(e.starts_with(&format!("M48: psynch {name} ({num}) is not modelled")), "{e}");
    }
    let mut mutexed = wait(0x200, 0);
    mutexed[3] = 0x6000_1000;
    let e = b.guest_psynch(SYS_PSYNCH_CVWAIT, mutexed).unwrap_err();
    assert!(e.starts_with("M48: psynch psynch_cvwait") && e.contains("with mutex 0x60001000"), "{e}");
    b.threads_mut().pend(0, 30);
    let e = b.guest_psynch(SYS_PSYNCH_CVSIGNAL, signal(0x100, 0, 0)).unwrap_err();
    assert!(e.starts_with("M48: a signal is pending on thread 0"), "{e}");
    assert_eq!(b.dbg_psynch(), &model, "no refusal touched the model");
    assert_eq!(b.threads().state_of(0), ThreadState::Blocked(BlockReason::Cv { addr: CV, deadline: None }));
    assert_eq!(b.threads().state_of(1), ThreadState::Runnable, "the caller never blocked");
}

/// The `psynch` field through `checkpoint`/`from_checkpoint` (Task 4's pattern).
#[test]
fn a_blocked_cvwait_survives_a_checkpoint_and_is_woken_after_restore() {
    let mut b = with_threads(1);
    block_in_cvwait(&mut b, 0x100, 1);
    let model = b.dbg_psynch().clone();
    assert!(!model.is_empty(), "the waiter is queued");
    let st = b.checkpoint();
    drop(b); // one VM per process
    let mut r = Box_::from_checkpoint(&st);
    assert_eq!(r.dbg_psynch(), &model, "the queue is carried (BoxState::psynch)");
    assert_eq!(r.guest_psynch(SYS_PSYNCH_CVSIGNAL, signal(0x100, 0, 0)), Ok(0x101),
        "a restored box without the queue would prepost (the P bit, 0x2) and wake nobody");
    assert_eq!(r.threads().state_of(0), ThreadState::Runnable);
}
```

  Run it red (`guest_psynch`, `dbg_psynch` and `BoxState::psynch` do not exist yet):

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --test psynch -- --test-threads=1 > $L/t5-box-red.log 2>&1; echo "exit=$?"; grep -a -E '^error|no method named' $L/t5-box-red.log | head
```

- [ ] **Step 6: The box.** In `crates/retrace-box/src/lib.rs`:
  1. **The field through every path** (Task 4's pattern, at the six sites Task 10's audit check 5 counts by shape; keep each shape exactly as shown):
     - the `Box_` struct, directly after Task 4's `gkq` field:

```rust
    /// M48 §3e: the guest's psynch condition variables, keyed by guest cv address (`psynch.rs`). Box
    /// state, not trace state: record and replay rebuild it from the guest's own syscalls, and every
    /// rebuild path carries it (`BoxState`).
    psynch: psynch::Psynch,
```

     - `BoxState`, directly after Task 4's `gkq` field:

```rust
    // M48 §3e: carried because a mid-run capture cannot re-derive it: the waits, signals and
    // preposts happened behind the checkpoint. Dropping it would make a seek into a blocked
    // `cvwait` restore a thread blocked on a cv the model has forgotten, so the signal meant to wake
    // it would prepost instead (condvar_e2e's seek test; box test `psynch.rs`).
    pub psynch: psynch::Psynch,
```

     - `checkpoint()`: a line of its own, `            psynch: self.psynch.clone(),`, after Task 4's `gkq: self.gkq.clone(),` line (each field of that literal is one line, as `kq: self.kq.clone(),` is);
     - `from_checkpoint`: likewise `            psynch: state.psynch.clone(),` on its own line after Task 4's `gkq: state.gkq.clone(),`;
     - the three one-line literals (`load_with_pac`, `load_dynamic`, `restore`): append `, psynch: psynch::Psynch::default()` after Task 4's `gkq: gkq::GuestKqueues::default()`, so each literal stays one line. Every snapshot is taken before the guest's first psynch call, so empty is right;
     - `dbg_internal_state`: append ` psynch={:?}` to the format string after Task 4's ` gkq={:?}`, and `self.psynch` to its arguments after `self.gkq`. `restoreparity.rs` compares this string between `load` and `restore`, where both are empty; the e2e seek test compares it between a warm and a cold seek, which is what makes the carry observable.

     These are exactly the shapes Task 10's audit check 5 counts (`decl=2 lit=3 ckpt=1 from=1 dbg=1`).

  2. **`guest_psynch` and `cv_timed_out`**, after Task 4's `deliver_wake`:

```rust
    /// M48 §3e (P1): the psynch condition-variable calls, emulated and never forwarded. Forwarded, a
    /// psynch call acts on the HOST's psynch state keyed by retrace's own addresses, blocking or
    /// waking the recorder. The semantics are `psynch.rs`'s port of libpthread-539.100.4 (plan F1).
    /// This method dispatches by number, gives a timed `cvwait` its deadline on the queue
    /// `schedule_after_block` serves (`BlockReason::Cv`), blocks the caller, and writes each woken
    /// thread's word where that thread will read it (`deliver_wake`).
    ///
    /// Returns the caller's own `x0`, with carry always clear: the word a call answers at once, or 0
    /// for a `cvwait` that blocks, whose real answer comes at its wake. `Err` is the refusal,
    /// `M48: psynch …` (T5-d) or `M48: a signal is pending on thread …` (T5-e), raised before the
    /// model or the thread table changes. Both dispatch arms call this with the same `(num, args)`
    /// (symmetry rule 1), and nothing is recorded (R3): every word is a pure function of box state.
    pub fn guest_psynch(&mut self, num: u64, args: [u64; 8]) -> Result<u64, String> {
        let tid = self.threads.current();
        let mut next = self.psynch.clone();
        let (out, deadline) = match num {
            retrace_arch::SYS_PSYNCH_CVWAIT => {
                // The clock is read only for a timed wait, at the call
                // (kern_synch.c:_psynch_cvwait's clock_absolutetime_interval_to_deadline).
                let ticks = psynch::timeout_ticks(args[6], args[7])?;
                let out = next.cvwait(args, tid)?;
                (out, ticks.map(|t| self.now_guest().saturating_add(t)))
            }
            retrace_arch::SYS_PSYNCH_CVSIGNAL => (next.cvsignal(args)?, None),
            retrace_arch::SYS_PSYNCH_CVBROAD => (next.cvbroad(args, self.threads.len())?, None),
            _ => return Err(format!(
                "M48: psynch {} ({num}) is not modelled: only psynch_cvwait, psynch_cvsignal and \
                 psynch_cvbroad are, the calls the walks measured (plan P4). args {}",
                psynch::call_name(num), Self::fmt_args(args))),
        };
        // T5-e: a woken thread with a signal pending would leave it where assert_no_stranded_signals
        // cannot see it, M18's semaphore posture. Checked for every woken thread before anything
        // changes; `deliver_wake` re-checks each as it wakes it.
        for w in &out.woken {
            let pending = self.threads.pending_of(w.tid);
            if pending != 0 {
                return Err(format!(
                    "M48: a signal is pending on thread {} (set {pending:#x}), which this {} would wake. The \
                     saved context of a thread blocked in psynch_cvwait is unmeasured, so a delivery there \
                     would be a guess; measure it (the blockedctx.rs shape) before allowing this.",
                    w.tid, psynch::call_name(num)));
            }
        }
        self.psynch = next;
        for w in &out.woken {
            self.deliver_wake(w.tid, w.word as u64, false, &[])?;
        }
        match out.ret {
            psynch::Ret::Word(word) => Ok(word as u64),
            psynch::Ret::Block => {
                self.threads.block(thread::BlockReason::Cv { addr: args[0], deadline });
                Ok(0)
            }
        }
    }

    /// M48 §3e: thread `tid`'s timed `psynch_cvwait` on `addr` reached its deadline. The answer is
    /// the port's timeout branch (`Psynch::time_out`): the waiter leaves the queue, S counts it, and
    /// the errno word carries `ECVCLEARED` or `ECVPREPOST` when that balances or empties the queue.
    /// It is delivered with carry set, onto the vCPU when `tid` is the current thread (Review Focus
    /// 2: node's `{0, 1 ns}` waits are woken in the very schedule that blocked them). Below the
    /// trace, so a refusal panics with its text on both sides (R5).
    fn cv_timed_out(&mut self, tid: usize, addr: u64) {
        let out = self.psynch.time_out(addr, tid);
        self.deliver_wake(tid, out.word, true, &[]).unwrap_or_else(|m| panic!("{m}"));
        for w in &out.woken {
            self.deliver_wake(w.tid, w.word as u64, false, &[]).unwrap_or_else(|m| panic!("{m}"));
        }
    }
```

  3. **The deadline wake.** In Task 4's `wake_due_threads`, add the `Cv` arm before its `unreachable!`, qualified the way its `Kevent` arm is (shown here as `thread::`-qualified, which is how the rest of this file names these types):

```rust
            thread::ThreadState::Blocked(thread::BlockReason::Cv { addr, .. }) => self.cv_timed_out(tid, addr),
```

  4. **The accessors**, after `dbg_kq_mut`:

```rust
    /// Test-only (M48): the psynch model, for `tests/psynch.rs`.
    #[doc(hidden)]
    pub fn dbg_psynch(&self) -> &psynch::Psynch { &self.psynch }

    /// Test-only (M48 Review Focus 5, T5-f): the psynch model, writable, so a test can plant the
    /// state an earlier silent divergence would leave.
    #[doc(hidden)]
    pub fn dbg_psynch_mut(&mut self) -> &mut psynch::Psynch { &mut self.psynch }
```

  Run the box's new tests and the modules they touch green:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --test psynch -- --test-threads=1 > $L/t5-box-green.log 2>&1; echo "psynch exit=$?"
cargo test -p retrace-box --lib -- --test-threads=1 > $L/t5-lib.log 2>&1; echo "lib exit=$?"
for t in gkq threads checkpointparity restoreparity kqmanager; do cargo test -p retrace-box --test $t --no-fail-fast -- --test-threads=1 > $L/t5-$t.log 2>&1; echo "$t exit=$?"; done
grep -a -h 'test result' $L/t5-box-green.log $L/t5-lib.log
```

  Expected: every `exit=0`; `psynch` reports `4 passed`.

- [ ] **Step 7: The record arm and its mirror.** In `crates/retrace-core/src/lib.rs`:
  1. **Record.** In `record_box`, insert this arm immediately before the comment line `// M14 Task 7: bsdthread_create is EMULATED, never forwarded — the host would create a`, which follows the M45 `SYS_KEVENT_QOS` arm (and Task 4's `SYS_KEVENT` arm, if it sits there). It is therefore before the generic `Stop::Syscall { num, args } =>` arm, whose `is_psynch` assert (Task 2) is now the backstop:

```rust
            // M48 §3e: every SDK psynch number is EMULATED, never forwarded (see Box_::guest_psynch).
            // Forwarded, a psynch call acts on the HOST's psynch state keyed by retrace's own
            // addresses, blocking or waking the recorder. One arm serves them all: cvwait, cvsignal and
            // cvbroad are the port, and the box refuses every other number by value. This arm may
            // PANIC by design, with the refusal's text, before anything is appended (R5).
            //
            // `writes` is empty and `err` false, deliberately: the call writes no guest memory, the
            // mirror recomputes the caller's word identically, and a woken thread's word is written at
            // its wake into a context both sides rebuild (R3). A blocked cvwait returns 0 here and
            // gets its real answer at the wake (Global Constraints).
            Stop::Syscall { num, args } if retrace_arch::is_psynch(num) => {
                let rc = b.guest_psynch(num, args).unwrap_or_else(|m| panic!("{m}"));
                w.append(&Event::Syscall { num, args, ret: rc, ret1: 0, err: false, writes: vec![], thread })
                    .map_err(|e| format!("append psynch: {e}"))?; count += 1;
                b.set_x0_err_and_return(rc, false);
            }
```

  2. **Replay.** In `ReplaySession::advance`'s `Event::Syscall` chain, insert this mirror immediately before the comment line `// M14 Task 7: the record arm's mirror (symmetry rule 1). Record and`, which follows the M45 `kevent_qos` mirror. It inherits the arm-top `verify_thread` and adds none (§11a item 1):

```rust
                            // M48 §3e: the psynch arm's mirror (symmetry rule 1), in the M45
                            // kevent_qos mirror's shape: the same call with the same arguments, the
                            // word compared, and a refusal reported as a divergence naming the call,
                            // never a panic (Review Focus 5). It inherits the arm-top
                            // `verify_thread` and adds none (§11a item 1).
                            if retrace_arch::is_psynch(num) {
                                let call = retrace_box::psynch::call_name(num);
                                let rc = match self.b.guest_psynch(num, args) {
                                    Ok(rc) => rc,
                                    Err(m) => return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "{call} refused on replay, though the recording accepted it \
                                         — replay diverged before this landmark: {m}") }),
                                };
                                if rc != *ret {
                                    return Err(Divergence { landmark: self.idx, pc,
                                        detail: format!("{call} rc mismatch: replay {rc:#x} != recorded {ret:#x}") });
                                }
                                // Record fixes `ret1: 0, err: false, writes: []`, so a recording
                                // carrying any other is not one this arm produced.
                                if *ret1 != 0 || *err || !writes.is_empty() {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "{call} recorded ret1={ret1:#x} err={err} with {} write(s); the emulation \
                                         records 0, false and none", writes.len()) });
                                }
                                self.b.set_x0_err_and_return(*ret, *err);
                                return self.finish_event();
                            }
```

  3. **The test hook** (T5-f), directly after `ReplaySession::dbg_write_mem`:

```rust
    /// Test-only (M48 Review Focus 5): the box's psynch model, so a test can plant the state an
    /// earlier silent divergence would leave (`Psynch::dbg_plant_waiter`).
    #[doc(hidden)]
    pub fn dbg_psynch_mut(&mut self) -> &mut retrace_box::psynch::Psynch { self.b.dbg_psynch_mut() }
```

  Each side now calls `guest_psynch(num, args)` exactly once, which is what Task 10's audit greps for. Neither comment above writes that call with its leading dot.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo build -p retrace-core > $L/t5-core-build.log 2>&1; echo "exit=$?"
grep -c '\.guest_psynch(' crates/retrace-core/src/lib.rs
```

  Expected: `exit=0`, then `2`.

- [ ] **Step 8: The fixture.** Create `crates/retrace-guest/c/condvar_dyn.c`:

```c
// M48 Task 5: psynch condition variables (spec §3h). libpthread's condvars are psynch on this host
// (plan F2), so every blocking wait here is a psynch_cvwait (305) and every signal that finds a
// waiter a psynch_cvsignal (304) or psynch_cvbroad (303). Signals are issued so that no mutex ever
// has a kernel waiter (plan F3), except in `mutex` mode, which exists to reach one. No mode sleeps:
// usleep is __semwait_signal (334), which retrace does not model. Modes:
//   pingpong     ROUNDS rounds of strict alternation between main and one thread on one cv
//   broadcast    three waiters on one cv, released by one pthread_cond_broadcast
//   timedout     a 5 ms relative wait nobody signals, then the same wait issued raw
//   timedsignal  a 2 s relative wait that the second thread signals first
//   onens        node's shape: a {0, 1 ns} relative wait, then the same wait issued raw
//   mutex        a contended firstfit mutex, which reaches psynch_mutexwait (301); retrace refuses it
// Each mode prints only what every native run prints. timedout and onens also print the cv's three
// sequence words, whose S word shows whether the timeout's errno carried ECVCLEARED.
#include <errno.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define ROUNDS 8
#define CSEQ_OFF 24 // T0(M4): the offset of pthread_cond_t's c_seq[3] (Step 1's addendum pins it)

// libsystem_kernel's raw stub, exported (libpthread declares it in its private header). Its kernel
// prototype is kern_synch.c:_psynch_cvwait.
extern uint32_t __psynch_cvwait(pthread_cond_t *cv, uint64_t cvlsgen, uint32_t cvugen,
                                pthread_mutex_t *mutex, uint64_t mugen, uint32_t flags,
                                int64_t sec, uint32_t nsec);

static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER, m2 = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t c = PTHREAD_COND_INITIALIZER, ready_cv = PTHREAD_COND_INITIALIZER,
                      raw = PTHREAD_COND_INITIALIZER;
static int turn, ready, go, flag, saw[3];

static void *ponger(void *arg) {
    (void)arg;
    for (int j = 0; j < ROUNDS; j++) {
        pthread_mutex_lock(&m);
        while (turn != 1) pthread_cond_wait(&c, &m);
        printf("pong %d\n", j);
        turn = 0;
        pthread_cond_signal(&c);
        pthread_mutex_unlock(&m);
    }
    return NULL;
}

static void *bwaiter(void *arg) {
    int k = (int)(intptr_t)arg;
    pthread_mutex_lock(&m);
    ready++;
    pthread_cond_signal(&ready_cv);
    while (!go) pthread_cond_wait(&c, &m);
    saw[k] = go;
    pthread_mutex_unlock(&m);
    return NULL;
}

static void *signaller(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    flag = 1;
    pthread_cond_signal(&c);
    pthread_mutex_unlock(&m);
    return NULL;
}

static void *locker(void *arg) {
    (void)arg;
    pthread_mutex_lock(&m);
    flag = 1;
    pthread_mutex_unlock(&m);
    return NULL;
}

static void dump(const char *mode, pthread_cond_t *cv) {
    const unsigned char *b = (const unsigned char *)cv + CSEQ_OFF;
    printf("%s c_seq:", mode);
    for (int i = 0; i < 12; i++) printf("%s%02x", i % 4 ? "" : " ", b[i]);
    printf("\n");
}

// A relative timed wait through libpthread, on `c`, which nobody signals. elapsed_ge_timeout is 1
// only if at least the interval passed on the guest's own clock (24 MHz, plan F6).
static void timed(const char *mode, long nsec) {
    struct timespec rel = { 0, nsec };
    uint64_t t0 = mach_absolute_time();
    pthread_mutex_lock(&m);
    int rc = pthread_cond_timedwait_relative_np(&c, &m, &rel);
    pthread_mutex_unlock(&m);
    uint64_t t1 = mach_absolute_time();
    printf("%s rc=%d elapsed_ge_timeout=%d\n", mode, rc, t1 - t0 >= (uint64_t)nsec * 3 / 125);
}

// The first wait on a fresh cv as libpthread would issue it (S carries the C bit, L one waiter,
// mutex 0, node's flags 0xa0), straight to the kernel, so its word and errno print unfiltered.
static void raw_wait(const char *mode, uint32_t nsec) {
    errno = 0;
    uint32_t rv = __psynch_cvwait(&raw, (1ull << 32) | 0x100, 0, NULL, 0, 0xa0, 0, nsec);
    printf("%s raw rv=%#x errno=%#x\n", mode, rv, errno);
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    pthread_t t[3];
    if (!strcmp(mode, "pingpong")) {
        pthread_create(&t[0], NULL, ponger, NULL);
        for (int i = 0; i < ROUNDS; i++) {
            pthread_mutex_lock(&m);
            while (turn != 0) pthread_cond_wait(&c, &m);
            printf("ping %d\n", i);
            turn = 1;
            pthread_cond_signal(&c);
            pthread_mutex_unlock(&m);
        }
        pthread_join(t[0], NULL);
        printf("pingpong done\n");
    } else if (!strcmp(mode, "broadcast")) {
        for (int k = 0; k < 3; k++) pthread_create(&t[k], NULL, bwaiter, (void *)(intptr_t)k);
        pthread_mutex_lock(&m);
        while (ready < 3) pthread_cond_wait(&ready_cv, &m);
        go = 1;
        pthread_mutex_unlock(&m);
        pthread_cond_broadcast(&c); // outside the mutex, so no mutex reaches the kernel (plan F3)
        for (int k = 0; k < 3; k++) pthread_join(t[k], NULL);
        printf("broadcast woke 3: saw %d %d %d\n", saw[0], saw[1], saw[2]);
    } else if (!strcmp(mode, "timedout")) {
        timed("timedout", 5 * 1000 * 1000);
        dump("timedout", &c);
        raw_wait("timedout", 5 * 1000 * 1000);
    } else if (!strcmp(mode, "onens")) {
        timed("onens", 1);
        dump("onens", &c);
        raw_wait("onens", 1);
    } else if (!strcmp(mode, "timedsignal")) {
        struct timespec rel = { 2, 0 };
        pthread_create(&t[0], NULL, signaller, NULL);
        uint64_t t0 = mach_absolute_time();
        pthread_mutex_lock(&m);
        int rc = 0;
        while (!flag && rc == 0) rc = pthread_cond_timedwait_relative_np(&c, &m, &rel);
        pthread_mutex_unlock(&m);
        uint64_t t1 = mach_absolute_time();
        pthread_join(t[0], NULL);
        printf("timedsignal rc=%d flag=%d before_deadline=%d\n", rc, flag, t1 - t0 < 2ull * 24000000);
    } else if (!strcmp(mode, "mutex")) {
        // main holds m while it waits on another cv, so the locker contends for m in the kernel.
        struct timespec rel = { 0, 100 * 1000 * 1000 };
        pthread_mutex_lock(&m);
        pthread_create(&t[0], NULL, locker, NULL);
        pthread_mutex_lock(&m2);
        (void)pthread_cond_timedwait_relative_np(&c, &m2, &rel);
        pthread_mutex_unlock(&m2);
        pthread_mutex_unlock(&m);
        pthread_join(t[0], NULL);
        printf("mutex ok flag=%d\n", flag);
    } else {
        fprintf(stderr, "mode?\n");
        return 2;
    }
    return 0;
}
```

  Wire it:
  - **`crates/retrace-guest/build.rs`:** insert immediately before the line `    // closewrite_dyn: the M37 console-close fixture — closes fd 1 and fd 2, then writes to each;`:

```rust
    // condvar_dyn: the M48 condition-variable fixture — modes pingpong, broadcast, timedout,
    // timedsignal, onens and mutex (see its header). Same recipe as hello_dyn; pthreads live in
    // libSystem, so no -lpthread.
    let src = format!("{}/c/condvar_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/condvar_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang condvar_dyn");
    assert!(status.success(), "condvar_dyn guest build failed");
```

  - **`crates/retrace-guest/src/lib.rs`:** after `pub const FORKFAIL_DYN: …;`:

```rust
/// M48: psynch condition variables by mode — `pingpong`, `broadcast`, `timedout`, `timedsignal`,
/// `onens` and `mutex` (spec §3h, Ruling T5-a; see the source's header).
pub const CONDVAR_DYN: &str = concat!(env!("OUT_DIR"), "/condvar_dyn");
```

  and, after the `forkfail_guest_parses` test:

```rust
    #[test]
    fn condvar_dyn_guest_parses() {
        // M48: proves the build.rs wiring and the path constant; behaviour is condvar_e2e's.
        let l = parse_macho(&std::fs::read(CONDVAR_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

  Check the fixture natively, mode by mode, before any e2e runs. A native output that varies between runs is a fixture defect: fix the fixture, never the test.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-guest condvar_dyn_guest_parses -- --test-threads=1 > $L/t5-guest.log 2>&1; echo "exit=$?"
G=$(ls -t target/debug/build/retrace-guest-*/out/condvar_dyn | head -1)
for mode in pingpong broadcast timedout timedsignal onens mutex; do for n in 1 2 3; do perl -e 'alarm 30; exec @ARGV' "$G" $mode > $L/t5-native-$mode-$n.out 2>&1; echo "$mode run $n exit=$?"; done; cmp $L/t5-native-$mode-1.out $L/t5-native-$mode-2.out && cmp $L/t5-native-$mode-1.out $L/t5-native-$mode-3.out && echo "$mode stable"; done
cat $L/t5-native-timedout-1.out $L/t5-native-onens-1.out
```

  Expected: every run exits 0 and every mode prints `stable`. `timedout` and `onens` show `rc=60 elapsed_ge_timeout=1`, `c_seq: 00010000 01010000 00000000` (L = 0x100, S = 0x101 with its C bit, U = 0, at the `T0(M4)` offset) and `raw rv=0xffffffff errno=0x13c` (`T0(M4)`). If a line differs from these, record the native line in the report: the e2e compares with native, and only its literal checks name these values.

- [ ] **Step 9: `condvar_e2e`.** Create `crates/retrace/tests/condvar_e2e.rs`:

```rust
// M48 gate (spec §3e, §3h, §4; Review Focus 2, 3 and 5). psynch condition variables end to end, on
// `condvar_dyn`. Every assertion is on the guest's own output compared with native's, on the trace,
// or on the recorder's own words, never on an exit code alone: a wait that never blocks, or a wake
// that answers the wrong word, still lets most of these guests exit 0.
mod util;

use retrace_trace::Event;
use std::path::{Path, PathBuf};

/// `condvar_dyn.c`'s `ROUNDS`.
const ROUNDS: usize = 8;

/// The fixture run natively: (exit code, stdout).
fn native(argv: &[&str]) -> (i32, Vec<u8>) {
    let out = std::process::Command::new(retrace_guest::CONDVAR_DYN).args(argv).output().unwrap();
    (out.status.code().unwrap_or(-1), out.stdout)
}

/// Record `argv`; assert exit 0 and native's stdout; replay twice byte-identically.
fn records_and_replays_as_native(argv: &[&str]) -> (String, PathBuf) {
    let (code, want) = native(argv);
    assert_eq!(code, 0, "{argv:?}: native exit");
    let (rec, trace) = util::record_dynamic_args(retrace_guest::CONDVAR_DYN, argv);
    assert_eq!(rec.code, 0, "{argv:?}: record: {}", rec.stderr);
    assert_eq!(String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(&want), "{argv:?}: recorded stdout vs native");
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{argv:?}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{argv:?}: replay {n} stdout");
    }
    (String::from_utf8_lossy(&rec.stdout).into_owned(), trace)
}

/// Every landmark of syscall `num` in `trace`: (landmark index, thread, args, ret).
fn calls(trace: &Path, num: u64) -> Vec<(usize, u32, [u64; 8], u64)> {
    retrace_trace::Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num: n, args, ret, thread, .. } if n == num => Some((i, thread, args, ret)),
        _ => None,
    }).collect()
}

/// §4's pingpong guard: ROUNDS rounds in strict alternation, as native prints them, with both
/// threads blocking in the kernel. Under the cooperative scheduler the counts are forced: main plays
/// ping 0 without waiting, then each side waits once per later round, and every signal but the two
/// that find nobody waiting (main's first, the ponger's last) reaches the kernel and wakes exactly
/// one waiter. A wake-any model passes one round, and a model that never blocks spins forever.
#[test]
fn pingpong_alternates_strictly_with_both_threads_waiting_in_the_kernel() {
    let (out, trace) = records_and_replays_as_native(&["pingpong"]);
    assert!(out.ends_with("ping 7\npong 7\npingpong done\n"), "{out}");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    for t in [0, 1] {
        assert_eq!(waits.iter().filter(|w| w.1 == t).count(), ROUNDS - 1, "thread {t}'s waits: {waits:x?}");
    }
    let signals = calls(&trace, retrace_arch::SYS_PSYNCH_CVSIGNAL);
    assert_eq!(signals.len(), 2 * (ROUNDS - 1), "signals that found a waiter: {signals:x?}");
    assert!(signals.iter().all(|s| s.3 == 0x101),
        "T0(M4): each signal wakes the one waiter and balances L and S: {signals:x?}");
    assert!(calls(&trace, retrace_arch::SYS_PSYNCH_CVBROAD).is_empty());
}

/// One `cvbroad` releases all three waiters, each of which blocked on that cv in the kernel.
#[test]
fn a_broadcast_wakes_all_three_waiters_with_one_call() {
    let (out, trace) = records_and_replays_as_native(&["broadcast"]);
    assert_eq!(out, "broadcast woke 3: saw 1 1 1\n");
    let broads = calls(&trace, retrace_arch::SYS_PSYNCH_CVBROAD);
    assert_eq!(broads.len(), 1, "{broads:x?}");
    let (_, thread, args, ret) = broads[0];
    assert_eq!((thread, ret), (0, 0x301), "main broadcasts; T0(M4): three increments and the C bit");
    let mut waiters: Vec<u32> = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT).into_iter()
        .filter(|w| w.2[0] == args[0]).map(|w| w.1).collect();
    waiters.sort();
    assert_eq!(waiters, [1, 2, 3], "each waiter blocked on the broadcast's cv");
}

/// §4's timedout guard. A 5 ms wait nobody signals blocks, the idle jump reaches its deadline, and
/// the kernel's timeout word comes back. A wait that returns at once fails `elapsed_ge_timeout`;
/// an errno without ECVCLEARED leaves the C bit off S in the c_seq line and fails the raw errno.
#[test]
fn a_timed_wait_that_expires_returns_the_kernels_timeout_word_after_the_idle_jump() {
    let (out, trace) = records_and_replays_as_native(&["timedout"]);
    assert!(out.contains("timedout rc=60 elapsed_ge_timeout=1\n"), "{out}");
    assert!(out.contains("timedout c_seq: 00010000 01010000 00000000\n"), "T0(M4): S = 0x101 at the c_seq offset. {out}");
    assert!(out.contains("timedout raw rv=0xffffffff errno=0x13c\n"), "T0(M4): ETIMEDOUT | ECVCLEARED, carry set. {out}");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 2 && waits.iter().all(|w| w.1 == 0 && w.2[6] == 0 && w.2[7] == 5_000_000 && w.3 == 0),
        "the libpthread wait and the raw one, both blocking (landmark word 0) with the 5 ms interval: {waits:x?}");
}

/// A timed wait signalled first returns 0, and its deadline is gone: had it fired, the idle jump
/// would have carried the guest's clock past 2 s and `before_deadline` would read 0.
#[test]
fn a_timed_wait_signalled_before_its_deadline_returns_zero_and_never_times_out() {
    let (out, trace) = records_and_replays_as_native(&["timedsignal"]);
    assert_eq!(out, "timedsignal rc=0 flag=1 before_deadline=1\n");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 1 && (waits[0].1, waits[0].2[6], waits[0].2[7]) == (0, 2, 0),
        "main's one timed wait, {{2, 0}}: {waits:x?}");
    let signals = calls(&trace, retrace_arch::SYS_PSYNCH_CVSIGNAL);
    assert!(signals.len() == 1 && signals[0].0 > waits[0].0 && signals[0].1 == 1 && signals[0].3 == 0x101,
        "the second thread's signal, after the wait, woke it (T0(M4)): {signals:x?}");
}

/// Review Focus 2. node's shape: `{0, 1 ns}` is 0 ticks, so the wait blocks with its deadline
/// already reached and is woken in the same `schedule_after_block` while it is still the current
/// thread, where `switch_to_thread` returns early. A timeout word written only to the saved context
/// is lost there: the vCPU keeps the landmark's 0 with carry clear, libpthread reads a successful
/// wait, and rc is 0, not 60.
#[test]
fn a_timed_wait_on_the_only_waiter_answers_on_the_vcpu() {
    let (out, trace) = records_and_replays_as_native(&["onens"]);
    assert!(out.contains("onens rc=60 elapsed_ge_timeout=1\n"), "the timeout reached libpthread: {out}");
    assert!(out.contains("onens raw rv=0xffffffff errno=0x13c\n"), "T0(M4): the word and its carry reached the stub: {out}");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 2 && waits.iter().all(|w| w.1 == 0 && w.2[6] == 0 && w.2[7] == 1),
        "two {{0, 1 ns}} waits on the only thread: {waits:x?}");
}

/// Spec §3j restore parity. A checkpoint taken while the ponger is blocked in `cvwait`, continued
/// across main's signal that wakes it, must equal a cold seek past the signal: the queue, the thread
/// table, the woken thread's delivered word and memory.
#[test]
fn a_seek_into_a_blocked_cvwait_matches_a_cold_seek() {
    let (_, trace) = records_and_replays_as_native(&["pingpong"]);
    let w = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT).into_iter().find(|w| w.1 == 1)
        .expect("the ponger's first wait").0;
    let s = calls(&trace, retrace_arch::SYS_PSYNCH_CVSIGNAL).into_iter().find(|s| s.0 > w)
        .expect("main's signal after it").0;
    let at = retrace_core::seek(&trace, w + 1, 0).unwrap();
    assert!(at.dbg_internal_state().contains("InWait"), "the checkpoint holds the queued waiter:\n{}", at.dbg_internal_state());
    let cp = at.checkpoint();
    let warm = {
        let mut r = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        r.advance_to_landmark(s + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (r.current_thread(), r.dbg_regs(), r.dbg_regs_of(1), r.dbg_fp_regs(), r.dbg_internal_state(), r.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, s + 1, 0).unwrap();
    assert!(!cold.dbg_internal_state().contains("InWait"), "past the signal nobody waits:\n{}", cold.dbg_internal_state());
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_regs_of(1), "the woken ponger's saved context, its delivered word included");
    assert_eq!(warm.3, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.4, cold.dbg_internal_state(), "the psynch queue and the clock: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.5).is_none(), "memory: checkpointed vs cold");
}

/// Review Focus 5. A `cvwait` the recording accepted but replay refuses is reachable only after an
/// earlier silent divergence, so replay must name it as a `Divergence`, never panic. The trace
/// cannot carry that divergence, because the arguments are compared before the mirror runs, so the
/// test plants its effect in the box (T5-f): a stale waiter at the ponger's own sequence, which
/// `_psynch_cvwait` answers EBUSY and the port refuses.
#[test]
fn a_cvwait_refused_on_replay_is_a_divergence_naming_it_not_a_panic() {
    let (_, trace) = records_and_replays_as_native(&["pingpong"]);
    let (i, _, args, _) = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT).into_iter().find(|w| w.1 == 1)
        .expect("the ponger's first wait");
    let lockseq = (args[1] as u32) & 0xffff_ff00;
    let mut s = retrace_core::seek(&trace, i, 0).unwrap();
    s.dbg_psynch_mut().dbg_plant_waiter(args[0], lockseq, 7);
    let d = match s.advance() { Err(d) => d, Ok(_) => panic!("the cvwait at landmark {i} replayed over a planted waiter") };
    assert_eq!(d.landmark, i);
    assert!(d.detail.starts_with("psynch_cvwait refused on replay, though the recording accepted it")
            && d.detail.contains(&format!("M48: psynch psynch_cvwait on cv {:#x} by thread 1", args[0]))
            && d.detail.contains(&format!("thread 7 already waits at sequence {lockseq:#x}")), "{}", d.detail);
}

/// Spec §1 part 5 for the port. The mutex pair is out of scope (plan F3), so a contended mutex,
/// which libpthread's firstfit lock takes to `psynch_mutexwait` (301), stops the recorder by value
/// before anything reaches the host. Natively the same program completes.
#[test]
fn the_mutex_pair_is_refused_by_value_and_never_forwarded() {
    let (code, out) = native(&["mutex"]);
    assert_eq!((code, String::from_utf8_lossy(&out).into_owned()), (0, "mutex ok flag=1\n".to_string()), "native completes");
    let (rec, trace) = util::record_dynamic_args(retrace_guest::CONDVAR_DYN, &["mutex"]);
    assert_eq!(rec.code, 101, "the recorder must stop at the refusal. stderr:\n{}", rec.stderr);
    assert!(rec.stderr.contains("M48: psynch psynch_mutexwait (301) is not modelled"), "stderr:\n{}", rec.stderr);
    assert!(!String::from_utf8_lossy(&rec.stdout).contains("mutex ok"), "the guest must not run past the refusal");
    // The refused call appends no landmark; main's timed wait, which let the locker run, did.
    assert!(calls(&trace, 301).is_empty(), "a refused call is never recorded");
    let waits = calls(&trace, retrace_arch::SYS_PSYNCH_CVWAIT);
    assert!(waits.len() == 1 && waits[0].1 == 0, "main's timed wait preceded the refusal: {waits:x?}");
}
```

  Run it:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test condvar_e2e --no-fail-fast -- --test-threads=1 > $L/t5-e2e.log 2>&1; echo "exit=$?"; grep -a -E 'test result|FAILED|panicked' $L/t5-e2e.log
```

  Expected: `exit=0`, `8 passed`. Two failures here are findings to report, never to loosen:
  - **A count in the pingpong test differs.** Its derivation rests on libpthread's signal fast path (`_pthread_psynch_cond_signal` returns without a syscall when L == S or L == U). Read the trace's psynch landmarks in order (`RETRACE_TRACE=1` on a scratch record) and report the measured sequence with the derivation it contradicts.
  - **The `mutex` mode refuses something other than 301** (for example, a ulock operation word through `guest_ulock_wait`'s assert, or a hang the alarm ends). Then libpthread's contended firstfit lock is not the plan-F2 path on this host: report the measured stop. The test's subject, a refusal by value before forwarding, still holds; whether the test is re-pointed or the gap is a Ruling is the controller's call.

- [ ] **Step 10: Green, regression, clippy, commit.** The task adds a `Box_` field to every rebuild path, a `BlockReason` variant every scheduler match sees, and an arm before the generic forward. So the regression set is the box chunk, the core chunk, and every gate that blocks a thread, delivers a signal to one, seeks through a checkpoint or uses libdispatch:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
grep -c '#\[test\]' crates/retrace-box/src/psynch.rs crates/retrace-box/tests/psynch.rs crates/retrace/tests/condvar_e2e.rs
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t5-box.log 2>&1; echo "box exit=$?"
cargo test -p retrace-guest --no-fail-fast -- --test-threads=1 > $L/t5-guestall.log 2>&1; echo "guest exit=$?"
cargo test -p retrace-core --no-fail-fast -- --test-threads=1 > $L/t5-core.log 2>&1; echo "core exit=$?"
for t in condvar_e2e thread_rust_e2e thread_watch_e2e sigthread_e2e thread_oracle blockedctx dispatch_e2e gcdtimer_e2e kqinit_e2e kq_e2e hello_dyn_e2e cpython_e2e cpython_crash_e2e hitorder_e2e checkpoint_seek seek; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t5-$t.log 2>&1; echo "$t exit=$?"; done
grep -a -h -E 'test result|FAILED|panicked' $L/t5-box.log $L/t5-condvar_e2e.log
cargo clippy -p retrace-box -p retrace-core -p retrace-guest -p retrace --all-targets -- -D warnings > $L/t5-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git commit -m "M48 t5: psynch condition variables — kern_synch.c's cvwait/cvsignal/cvbroad ported by guest address, BlockReason::Cv on the deadline queue, the mutex pair refused by value, condvar_dyn and condvar_e2e"
```

  Expected:
  - the `#[test]` counts are `11`, `4` and `8`;
  - every `exit=0`, and clippy is clean;
  - `condvar_e2e` reports `8 passed`.

  `cpython_e2e` and `cpython_crash_e2e` skip with a `SKIPPED` line without Homebrew Python; say so in the report if they did. If a target name above does not exist in `crates/retrace/tests/`, drop it and say so in the report; do not invent one.

- [ ] **Step 11: Controls (on the committed tree).** Apply one at a time, run the named test, record the failing assertion's text, then restore with `git checkout <t5 commit> -- <file>` and confirm `git status --short` is empty. A control that stays green is a finding: report it.
  1. **The checkpoint drops the queue.** In `crates/retrace-box/src/lib.rs`'s `from_checkpoint`, change `psynch: state.psynch.clone(),` to `psynch: psynch::Psynch::default(),`.
     - Run `cargo test -p retrace-box --test psynch a_blocked_cvwait_survives_a_checkpoint_and_is_woken_after_restore -- --test-threads=1`. Expected red: "the queue is carried".
     - Run `cargo test -p retrace --test condvar_e2e a_seek_into_a_blocked_cvwait_matches_a_cold_seek -- --test-threads=1`. Expected red: `warm: diverged at …: psynch_cvsignal rc mismatch: replay 0x2 != recorded 0x101`, the signal preposting on a queue the restore forgot.
  2. **A deadline wake writes only the saved context** (Review Focus 2). In `cv_timed_out`, replace the first `deliver_wake` call with a write that skips the vCPU:

```rust
        self.threads.wake(tid).unwrap_or_else(|m| panic!("{m}"));
        let c = self.threads.ctx_mut(tid);
        c.regs.x[0] = out.word;
        c.regs.cpsr |= retrace_arch::PSTATE_C;
```

     - Run `cargo test -p retrace --test condvar_e2e a_timed_wait_on_the_only_waiter_answers_on_the_vcpu -- --test-threads=1`.
     - Expected red at "recorded stdout vs native": the guest prints `onens rc=0` where native prints `onens rc=60`. The vCPU kept the landmark's 0 with carry clear, and `switch_to_thread`'s early return never loaded the saved context.
  3. **The sequence compare ignores the wrap** (Review Focus 3). In `psynch.rs`, make `is_seqhigher`'s body `x & PTHRW_COUNT_MASK > y & PTHRW_COUNT_MASK`.
     - Run `cargo test -p retrace-box --lib psynch -- --test-threads=1`.
     - Expected red: `the_sequence_window_wraps_as_synch_internal_h_computes` ("one step across the wrap"), and `a_signal_across_the_sequence_wrap_wakes_the_waiter`, whose `cvwait` is refused with `S 0xffffff01 is not below L 0x0`.
  4. **The mirror panics on a refusal** (Review Focus 5). In the psynch mirror in `crates/retrace-core/src/lib.rs`, replace the `Err(m) => return Err(Divergence { … })` arm with `Err(m) => panic!("{m}"),`.
     - Run `cargo test -p retrace --test condvar_e2e a_cvwait_refused_on_replay_is_a_divergence_naming_it_not_a_panic -- --test-threads=1`.
     - Expected red: the test panics inside `advance` with `M48: psynch psynch_cvwait on cv … by thread 1: thread 7 already waits at sequence …`, and no `Divergence` is returned.
  5. **The mutex refusal is gone** (T5-d, plan F3). In `Psynch::cvwait`, delete the `if mutex != 0 { … }` block.
     - Run `cargo test -p retrace-box --lib psynch -- --test-threads=1`.
     - Expected red: `every_unmeasured_shape_is_refused_by_value_and_changes_nothing` at its `with mutex 0x60001000` row, which now blocks the caller.

  Task 10's third control (the psynch record arm's guard set to `false`) is the one that proves the arm, not the generic assert, carries every psynch number. It is not repeated here.

---

### Task 6: JIT write-protect (J1: `retrace-arch`, `retrace-box`, `retrace-core`, fixtures, e2e)

V8 toggles its code space between writable and executable with `pthread_jit_write_protect_np`, which `msr`s one of two commpage words into `S3_6_C15_C1_5` (spec §2c). Under retrace that `msr` is an undefined instruction (EC 0x00, walls.md §1 row 4, §11b item 2), and the following `ic ivau` traps EC 0x18 (row 5). This task:
- emulates the register per thread beside `try_emulate_undef_mrs`, admitting exactly the commpage's `+0x110` and `+0x118` (R1, R2);
- keeps every `MAP_JIT` range in a pure `jit.rs`, and stamps the running thread's view over each range minus its no-access extents (§11b item 4), flipping on a toggle and on a thread switch;
- restamps after `unprotect`, which stamps `ATTR_DATA` unconditionally (Review Focus item 4);
- makes `flush_guest_tlb` step-safe (P9);
- sets SCTLR UCI for every guest (§11b item 3).

Nothing is recorded (R3). The register, the view and the stamps live in `run()`/`step()`/`switch_to_thread` and in box methods both dispatch arms already call (`guest_mmap`, `guest_munmap`, `guest_mprotect`), so symmetry rule 2 holds by construction and there is no new record arm or mirror.

**Rulings made here** (each is ledgered in the task report):
- **T6-a. Admitted `MAP_JIT` protections are exactly `PROT_NONE` and RWX**, on `mmap` and on `mprotect`. Every other value is refused by value, read-only included. This narrows §11a item 5 ("only a read-only `MAP_JIT` page is refused"): what SPRR does to an RW or R-X `MAP_JIT` page is unmeasured, and spec §1 part 5 says an unmeasured shape stops the recorder. V8 uses `PROT_NONE` then RWX (P5); `sprr.c` uses RWX.
- **T6-b. `MAP_JIT` with `MAP_FIXED`, with `MAP_SHARED`, or without `MAP_ANON` is refused by value.** xnu's `mmap` answers each with `EINVAL` (`bsd/kern/kern_mman.c:mmap`, inferred from source, not measured here), and node never issues one (P5: not FIXED). This corrects §3f's "FIXED or not".
- **T6-c. A FIXED mapping (every FIXED path funnels through `place_fixed`) or a `mach_vm_remap` whose source or target overlaps a `MAP_JIT` range is refused by value.** Admitting either would leave the view stamped over pages the new mapping owns, or alias a page whose stamp then stops following the view. No guest was measured doing either (t0 M5).
- **T6-d. A `munmap` of any part of a `MAP_JIT` range is admitted.** It trims or splits the range (§11a item 5), and the released pages go back to `ATTR_DATA`, the identity default every anonymous mapping starts from. Otherwise the next mapping at that address would inherit an `ATTR_CODE` leaf its guest never asked for: M13's `drop_protection` argument, applied to the view.
- **T6-e. A thread is write-enabled iff its register holds the commpage's `+0x110` word.** `0` (R1's initial value) and `+0x118` are both protected. `View::of_sprr` is that rule as a pure function.
- **T6-f. A write by a guest with no SPRR commpage is refused.** A static guest has no commpage. A host whose `+0x110` and `+0x118` are equal cannot tell the two modes apart. In both cases no value is admissible. The `mrs` still answers the thread's value, `0` until written, so libdyld's bit-36 probe (§2d) is untouched.
- **T6-g. `from_checkpoint` carries `jit` and asserts it; it never flushes** (§11a item 3). The stamps ride in `mem`. The restore asserts two things: every stamped page holds the view's attribute, and the view equals the restored current thread's mode. A fresh vCPU's TLB is empty, so a flush would only add the TLBI-stub backing on one side. `restore()` (landmark 0) starts with an empty set and every thread at `0`, which is consistent by construction.
- **T6-h. A signal handler runs in its thread's current mode.** Delivery and `sigreturn` do not touch the register. What xnu does to SPRR across `sendsig` is unmeasured. No fixture and no node walk delivers a signal inside a write-enabled window: node's SIGSEGV handler in the crash demo runs after the addon's fault, outside any toggle. Task 9's census would show one.
- **T6-i. The rewritten SCTLR unit test is renamed** to `sctlr_enables_dc_zva_and_ic_ivau_for_el0_and_nothing_else`, because the old name would now be false. Its count stays one (header amendment in the report).
- **T6-j. Two observability accessors are added:** `Box_::ipa_is_el0_writable` (the twin of `ipa_is_noaccess`, AP `0b01`) and `Box_::dbg_jit`. `dbg_internal_state` also gains ` jit={:?}`, Task 4's pattern, so every seek test's internal-state comparison covers the set. `restoreparity.rs` compares that string between a load box and a restore box, and both hold the default set, so it stays green.

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (the M48 section: `MAP_JIT`, `SprrAccess`, `decode_sprr_access`)
- Modify: `crates/retrace-arch/tests/nodeshapes.rs` (+2)
- Create: `crates/retrace-box/src/jit.rs` (`JitSet`, `View`, `admit_mmap`, `admit_mprotect`; 8 unit tests)
- Modify: `crates/retrace-box/src/thread.rs` (`Thread.sprr`, `ThreadTable::{sprr_of, set_sprr_of}`; +1 unit test)
- Modify: `crates/retrace-box/src/lib.rs`:
  - `pub mod jit;` and the commpage constant;
  - the `jit` field through every path;
  - the SPRR arm in `try_emulate_undef_mrs`, `sprr_write`, `sprr_admitted`, `sync_jit_view`, `restamp_jit`, `unmap_jit`, `assert_jit_stamped`;
  - the `MAP_JIT` hooks in `map_mmap_region` (`guest_mmap`'s body, which the file-backed and replay entries share), `guest_munmap`, `guest_mprotect`, `place_fixed` and `guest_vm_remap`;
  - `switch_to_thread`'s view sync;
  - `flush_guest_tlb`'s `MDSCR` guard;
  - SCTLR UCI and the rewritten `sctlr_dze_tests` test;
  - `ipa_is_el0_writable`, `dbg_jit`, and ` jit={:?}` in `dbg_internal_state`.
- Create: `crates/retrace-box/tests/jit.rs` (5 tests)
- Modify: `crates/retrace-box/tests/checkpointparity.rs` (+1 test; one row in `assert_checkpoint_parity`)
- Modify: `crates/retrace-core/src/lib.rs` (the `MAP_JIT` exemption from the anon-exec warning; nothing else, and no mention of SPRR, which Task 10's audit greps for)
- Create: `crates/retrace-guest/asm/sprrprobe.s`, `crates/retrace-guest/c/jitwp_dyn.c`; modify `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs` (`SPRRPROBE`, `JITWP_DYN`, 2 unit tests)
- Create: `crates/retrace/tests/jitwp_e2e.rs` (8 tests)

**Interfaces:**
- Consumes:
  - t0 §M1 (the `msr` traps EC 0x00; `ic ivau` traps EC 0x18 with ISS op0 1, op1 3, CRn 7, CRm 5, op2 1; the two commpage values; UCI alone needed) and §M5 (the `MAP_JIT` census: one 256 MiB `PROT_NONE` map, flags `0x41842`, one `mprotect(+0x40000, 0xffc0000, RWX)`, a whole-range `munmap`; the SPRR values written; R9's verdict). Step 1's addendum pins them.
  - Task 3's `guest_munmap` (the rounded `start`/`end` and `unmap_range`), which this task adds one call to.
  - Task 4's field-through-every-path pattern (its Interfaces block): the `Box_` struct field and the `BoxState` field after `excl`, `checkpoint()`'s `f: self.f.clone()`, `from_checkpoint`'s `f: state.f.clone()`, `f: <Type>::default()` in each of the three one-line literals (`load_with_pac`, `load_dynamic`, `restore`), and ` f={:?}` in `dbg_internal_state`. Those are the seven `f: ` lines Task 10's audit counts against `kq`'s (Ruling T10-a), plus the format string. `jit` goes after Task 5's `psynch`, which follows Task 4's `gkq`, at every site, and each literal stays one line.
  - Task 4's `BlockReason`/`schedule_after_block` changes are not touched. A switch reaches `sync_jit_view` through `switch_to_thread`, whichever wake made it.
- Produces:
  - `retrace_arch::{MAP_JIT, SprrAccess, decode_sprr_access}`:
    ```rust
    pub const MAP_JIT: u64 = 0x800;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SprrAccess { Read { rt: u32 }, Write { rt: u32 } }
    pub fn decode_sprr_access(insn: u32) -> Option<SprrAccess>;
    ```
  - `retrace_box::jit::{JitSet, View}`:
    ```rust
    pub enum View { Rx, Rw }                       // Default: Rx
    impl View { pub fn of_sprr(sprr: u64, write_enable: Option<u64>) -> View; }
    pub struct JitSet { /* ranges, view */ }       // Clone, Debug, Default, PartialEq, Eq
    impl JitSet {
        pub fn admit_mmap(prot: u64, flags: u64) -> Result<bool, String>;
        pub fn admit_mprotect(&self, ipa: u64, len: u64, prot: u64) -> Result<bool, String>;
        pub fn refuse_overlap(&self, addr: u64, len: u64, what: &str) -> Result<(), String>;
        pub fn add(&mut self, start: u64, len: u64);
        pub fn remove(&mut self, start: u64, end: u64) -> Vec<(u64, u64)>;
        pub fn stamped_extents(&self, noaccess: &[(u64, u64)]) -> Vec<(u64, u64)>;
        pub fn ranges(&self) -> &[(u64, u64)];
        pub fn view(&self) -> View;
        pub fn set_view(&mut self, view: View);
    }
    ```
  - `ThreadTable::{sprr_of(&self, tid: usize) -> u64, set_sprr_of(&mut self, tid: usize, value: u64)}`.
  - `Box_::sprr_write(&mut self, value: u64)`: `value` is the written register value. It is admitted or refused, stored on the current thread, then the view is synced. Task 7 adds its gated `RETRACE_SPRR` line after the admission and before `self.sync_jit_view()`, which is the method's last line.
  - `Box_::{ipa_is_el0_writable(&self, ipa: u64) -> bool, dbg_jit(&self) -> &jit::JitSet}`.
  - The refusal texts, all panics raised identically on record and replay (R5):
    - `M48: SPRR write <value> at pc <pc> …` (an inadmissible value, or no commpage);
    - `M48: MAP_JIT mmap flags <flags> …`, `M48: MAP_JIT mmap prot <prot> …`, `M48: MAP_JIT mprotect … prot <prot> …`, `M48: MAP_JIT range … overlapped by <what> …`.
  - `retrace_guest::{SPRRPROBE, JITWP_DYN}`. `jitwp_dyn` modes are `basic`, `v8`, `twothreads` and `fault`.

- [ ] **Step 1: Controller addendum.** Write `$L/task-6-addendum.md` pinning, from measurements §M1 and §M5:
  - the `msr S3_6_C15_C1_5` trap class (expected EC 0x00; EC 0x18 would move the arm to `try_emulate_timebase`'s path, a Ruling before this task starts);
  - `ic ivau`'s EC and ISS (expected 0x18, op0 1, op1 3, CRn 7, CRm 5, op2 1);
  - the two commpage words (expected `+0x110 = 0x2010002030300000`, `+0x118 = 0x2010002030100000`) and that every SPRR value the walks wrote is one of them;
  - that UCI alone is needed (no `CTR_EL0` read, no `DC CVAU`);
  - the `MAP_JIT` census: every `MAP_JIT` mmap's `prot` and `flags` (expected one, `prot 0`, flags `0x41842`), every `mprotect` over one (expected one, RWX), every `munmap` over one (expected one, whole-range), and any non-`MAP_JIT` exec mmap (expected none);
  - R9's verdict (expected: does not fire).

  A `MAP_JIT` `prot` other than 0 or 7, a FIXED `MAP_JIT`, or an `mprotect` to a value other than 0 or 7 over one would be refused by this task's Rulings T6-a/T6-b. If the census shows one, write a Ruling admitting that measured shape before Step 2. Do not start until the addendum exists.

- [ ] **Step 2: The decode, red first.** Append to `crates/retrace-arch/tests/nodeshapes.rs` (Task 2 created it), adding `decode_sprr_access` and `SprrAccess` to its `use retrace_arch::{…}` line:

```rust
/// M48 §3f, §11b item 2: `S3_6_C15_C1_5` is decoded in both directions, with its register. The
/// words are clang's: `crates/retrace-guest/asm/sprrprobe.s` assembles the first and the third, and
/// `retrace-guest`'s `sprrprobe_guest_parses_and_carries_both_sprr_encodings` re-reads them from the
/// built binary, so a word typed wrongly here fails there.
#[test]
fn the_sprr_decode_reads_s3_6_c15_c1_5_in_both_directions() {
    use SprrAccess::*;
    assert_eq!(decode_sprr_access(0xd53e_f1a0), Some(Read { rt: 0 }));   // mrs x0, S3_6_C15_C1_5 (sprrprobe)
    assert_eq!(decode_sprr_access(0xd53e_f1a9), Some(Read { rt: 9 }));   // mrs x9, … (pthread's read-back)
    assert_eq!(decode_sprr_access(0xd51e_f1a1), Some(Write { rt: 1 }));  // msr S3_6_C15_C1_5, x1 (sprrprobe)
    assert_eq!(decode_sprr_access(0xd51e_f1a0), Some(Write { rt: 0 }));  // msr …, x0 (pthread_jit_write_protect_np)
    assert_eq!(decode_sprr_access(0xd51e_f1bf), Some(Write { rt: 31 })); // msr …, xzr: Rt 31 is XZR here
}

/// M48 §3f: the decode swallows no neighbour. Each of these stays undefined (or keeps its own
/// path) and surfaces as `Stop::Other`, never as an SPRR access.
#[test]
fn the_sprr_decode_refuses_every_neighbour() {
    for (w, what) in [
        (0xd53e_f180, "mrs x0, S3_6_C15_C1_4 (op2 4)"),
        (0xd53e_f1c0, "mrs x0, S3_6_C15_C1_6 (op2 6)"),
        (0xd53e_f2a0, "mrs x0, S3_6_C15_C2_5 (CRm 2)"),
        (0xd53d_f1a0, "mrs x0, S3_5_C15_C1_5 (op1 5)"),
        (0xd51c_f2e0, "msr S3_4_C15_C2_7, x0 (APRR: pthread's commpage +0x10c == 1 path)"),
        (0xd50b_7523, "ic ivau, x3 (a SYS, sys_icache_invalidate's)"),
        (0xd503_42df, "msr daifset, #2 (MSR immediate)"),
        (0xd503_3fdf, "isb"),
    ] {
        assert_eq!(decode_sprr_access(w), None, "{what} ({w:#010x})");
    }
}
```

Run it red; the decode does not exist yet:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-arch --test nodeshapes -- --test-threads=1 > $L/t6-red-arch.log 2>&1; echo "exit=$?"; grep -a -E 'error|cannot find' $L/t6-red-arch.log | head
```

Expected: a non-zero exit, with `cannot find function `decode_sprr_access`` and `cannot find type `SprrAccess``.

- [ ] **Step 3: The decode and `MAP_JIT`.** At the end of the M48 section of `crates/retrace-arch/src/lib.rs` (the one Task 2 opened), add:

```rust
/// M48 §3f: `mmap`'s `MAP_JIT` flag (SDK `sys/mman.h`, `0x0800`): a region V8 writes code into and
/// executes, toggled per thread through `S3_6_C15_C1_5` (§2c). The box keeps every such range
/// (`retrace_box::jit`), and the anonymous-exec warning in `retrace-core` exempts it.
pub const MAP_JIT: u64 = 0x800;

/// M48 §3f: an EL0 access to `S3_6_C15_C1_5`, the SPRR register `pthread_jit_write_protect_np`
/// writes (`pthread-jit-disasm.txt`). HVF exposes neither direction, so both arrive as undefined
/// instructions (EC 0x00; §2d, §11b item 2). `rt` is raw: 31 is XZR in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SprrAccess {
    /// `mrs Xt, S3_6_C15_C1_5`.
    Read { rt: u32 },
    /// `msr S3_6_C15_C1_5, Xt`.
    Write { rt: u32 },
}

/// M48 §3f: decode `insn` as an access to `S3_6_C15_C1_5`, or None. The system-register class is
/// `1101010100 L 1 o0 op1 CRn CRm op2 Rt`; this register is op0 3 (`o0` 1), op1 6, CRn 15, CRm 1,
/// op2 5. `L` 1 is `mrs` (`0xd53ef1a0`) and `L` 0 is `msr` (`0xd51ef1a0`). Every other register and
/// every other instruction is None, so a neighbour keeps failing loud as `Stop::Other`.
pub fn decode_sprr_access(insn: u32) -> Option<SprrAccess> {
    let rt = insn & 0x1f;
    match insn & !0x1f {
        0xd53e_f1a0 => Some(SprrAccess::Read { rt }),
        0xd51e_f1a0 => Some(SprrAccess::Write { rt }),
        _ => None,
    }
}
```

Run `cargo test -p retrace-arch --test nodeshapes -- --test-threads=1` (log `$L/t6-arch.log`): all six tests pass.

- [ ] **Step 4: `Thread.sprr`.** In `crates/retrace-box/src/thread.rs`:
  1. Add the field as the last field of `Thread`:
     ```rust
         /// M48 §3f: this thread's `S3_6_C15_C1_5`, the JIT write-protect register. Per thread, as
         /// native's is (§2c). It starts at 0 for every thread, main included (R1), and is never
         /// inherited: native starts a thread protected even when its creator is write-enabled. Only
         /// `Box_::sprr_write` changes it, with an admitted value.
         pub sprr: u64,
     ```
  2. Initialise it to `0` in both `Thread` literals, `ThreadTable::new`'s and `spawn`'s, after `redirected: false,`.
  3. Add the accessors after `set_altstack_of`:
     ```rust
         /// M48 §3f: `tid`'s `S3_6_C15_C1_5` (R1: 0 until written).
         pub fn sprr_of(&self, tid: usize) -> u64 { self.threads[tid].sprr }

         /// M48 §3f: set `tid`'s register. `Box_::sprr_write` is the only production caller, with an
         /// admitted value, and it syncs the view after.
         pub fn set_sprr_of(&mut self, tid: usize, value: u64) { self.threads[tid].sprr = value; }
     ```
  4. Add to the module's `tests`:
     ```rust
         #[test]
         fn a_spawned_thread_starts_protected_whatever_its_creator_wrote() {
             // M48 R1, §2c (sprr.out): native's main thread starts protected, and so does a thread
             // created while its parent is write-enabled. retrace's protected initial value is 0.
             let mut t = ThreadTable::new(ThreadCtx::zeroed());
             assert_eq!(t.sprr_of(0), 0, "main starts at R1's 0");
             t.set_sprr_of(0, 0x2010_0020_3030_0000); // the probe host's +0x110: write-enabled
             let child = t.spawn(ThreadCtx::zeroed(), (0, 0));
             assert_eq!(t.sprr_of(child), 0, "a child is never write-enabled by inheritance");
             assert_eq!(t.sprr_of(0), 0x2010_0020_3030_0000, "the creator keeps its own value");
         }
     ```

  The field rides `ThreadTable`, which `BoxState` already carries, so `checkpoint`/`from_checkpoint` need nothing more for it.

- [ ] **Step 5: `jit.rs`, pure, with its unit tests.** Create `crates/retrace-box/src/jit.rs`. Its tests are written with the code, because the module is new and pure: they run red against an empty body only by not compiling, which proves nothing.

```rust
//! M48 §3f (J1): the guest's `MAP_JIT` ranges and the stage-1 view stamped over them.
//!
//! Pure data. `Box_` owns one `JitSet`, stamps what [`JitSet::stamped_extents`] returns with the
//! attribute its [`View`] names, and carries it through every rebuild path (`BoxState`). It is rebuilt
//! from the guest's own `mmap`/`mprotect`/`munmap` landmarks and its own `msr`s on both sides, so
//! nothing here is recorded (R3).
//!
//! The view is per process, and it always equals the running thread's mode (§3f "Flips"). Native
//! gives each thread its own `S3_6_C15_C1_5` (§2c); under one vCPU and a cooperative scheduler only
//! the running thread's is ever consulted, so one view that follows it is native's per-thread view.

use crate::subtract_range;

/// `PROT_READ | PROT_WRITE | PROT_EXEC` (SDK `sys/mman.h`).
const PROT_RWX: u64 = 7;
// SDK `sys/mman.h`.
const MAP_SHARED: u64 = 0x1;
const MAP_FIXED: u64 = 0x10;
const MAP_ANON: u64 = 0x1000;

/// The stage-1 view over a `MAP_JIT` range's accessible pages. `Box_::restamp_jit` is the one place
/// a view becomes an attribute: `Rx` is `ATTR_CODE` and `Rw` is `ATTR_DATA`, so no page is ever
/// writable and executable at once, and the W^X invariant holds by construction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// Protected: read and execute. Every native thread starts here (§2c), and so does every guest
    /// thread (R1).
    #[default]
    Rx,
    /// Write-enabled: read and write, never execute.
    Rw,
}

impl View {
    /// The mode of a thread whose `S3_6_C15_C1_5` holds `sprr` (Ruling T6-e). `write_enable` is the
    /// commpage's `+0x110` word, or None for a guest with no SPRR commpage. Only that exact word
    /// write-enables: R1's initial 0 and the commpage's `+0x118` both protect.
    pub fn of_sprr(sprr: u64, write_enable: Option<u64>) -> View {
        if sprr != 0 && Some(sprr) == write_enable { View::Rw } else { View::Rx }
    }
}

/// Every `MAP_JIT` range the guest holds, and the view currently stamped over them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JitSet {
    /// Page-aligned `(start, len)`, sorted by start and disjoint: what the guest mapped `MAP_JIT`
    /// and has not unmapped. A partial `munmap` trims or splits a range (Ruling T6-d).
    ranges: Vec<(u64, u64)>,
    /// What stage 1 holds over every range minus its no-access extents.
    view: View,
}

impl JitSet {
    /// Is an `mmap` with these `prot` and `flags` a `MAP_JIT` one (`Ok(true)`), an ordinary one
    /// (`Ok(false)`), or a `MAP_JIT` shape this model refuses (`Err`, naming the value)? Rulings
    /// T6-a and T6-b. An ordinary RWX anonymous map keeps today's path untouched (§11a item 2).
    pub fn admit_mmap(prot: u64, flags: u64) -> Result<bool, String> {
        if flags & retrace_arch::MAP_JIT == 0 { return Ok(false); }
        if flags & (MAP_FIXED | MAP_SHARED) != 0 || flags & MAP_ANON == 0 {
            return Err(format!(
                "M48: MAP_JIT mmap flags {flags:#x}: MAP_JIT with MAP_FIXED, with MAP_SHARED or \
                 without MAP_ANON is unmodelled. xnu's mmap answers EINVAL (kern_mman.c:mmap), and \
                 no measured guest issues one (Ruling T6-b)"));
        }
        if prot != 0 && prot != PROT_RWX {
            return Err(format!(
                "M48: MAP_JIT mmap prot {prot:#x}: only PROT_NONE (V8's code-range reservation, \
                 t0 M5) and RWX (sprr.c's page) are measured on a MAP_JIT mapping (Ruling T6-a)"));
        }
        Ok(true)
    }

    /// Does an `mprotect(ipa, len, prot)` touch a `MAP_JIT` range (`Ok(true)`, and the caller
    /// restamps the view afterwards), miss every range (`Ok(false)`), or ask one for a protection
    /// this model refuses (`Err`)? Partial overlap is admitted: V8 commits a sub-range of its
    /// reservation (§11a item 5, walls.md §3).
    pub fn admit_mprotect(&self, ipa: u64, len: u64, prot: u64) -> Result<bool, String> {
        let Some(&(s, l)) = self.ranges.iter().find(|&&(s, l)| overlap(ipa, len, s, l)) else {
            return Ok(false);
        };
        if prot != 0 && prot != PROT_RWX {
            return Err(format!(
                "M48: MAP_JIT mprotect [{ipa:#x}, +{len:#x}) prot {prot:#x} over the range \
                 [{s:#x}, {:#x}): only PROT_NONE and RWX are measured on a MAP_JIT page \
                 (Ruling T6-a)", s + l));
        }
        Ok(true)
    }

    /// `Err` naming the range when `[addr, addr+len)` overlaps one (Ruling T6-c). `what` names the
    /// request ("a FIXED mapping", "a mach_vm_remap source", "a mach_vm_remap target").
    pub fn refuse_overlap(&self, addr: u64, len: u64, what: &str) -> Result<(), String> {
        match self.ranges.iter().find(|&&(s, l)| overlap(addr, len, s, l)) {
            None => Ok(()),
            Some(&(s, l)) => Err(format!(
                "M48: MAP_JIT range [{s:#x}, {:#x}) overlapped by {what} at [{addr:#x}, +{len:#x}): \
                 unmodelled, since the view would stay stamped over pages the new mapping owns \
                 (Ruling T6-c)", s + l)),
        }
    }

    /// Add a newly mapped range. `Box_::map_mmap_region` placed it, so it is page-aligned and
    /// overlaps nothing already mapped, ranges included.
    pub fn add(&mut self, start: u64, len: u64) {
        assert!(!self.ranges.iter().any(|&(s, l)| overlap(start, len, s, l)),
            "M48: a new MAP_JIT range [{start:#x}, +{len:#x}) overlaps a live one: {:x?}", self.ranges);
        let at = self.ranges.partition_point(|&(s, _)| s < start);
        self.ranges.insert(at, (start, len));
    }

    /// Remove page-aligned `[start, end)`: a range wholly inside goes, one it cuts is trimmed or
    /// split (Ruling T6-d). Returns the removed pieces, which the caller stamps back to `ATTR_DATA`.
    pub fn remove(&mut self, start: u64, end: u64) -> Vec<(u64, u64)> {
        let gone = self.ranges.iter()
            .filter(|&&(s, l)| overlap(start, end - start, s, l))
            .map(|&(s, l)| { let (a, b) = (s.max(start), (s + l).min(end)); (a, b - a) })
            .collect();
        subtract_range(&mut self.ranges, start, end - start);
        gone
    }

    /// The extents the view is stamped over: every range minus the no-access extents inside it
    /// (§11b item 4). V8 maps its whole code range `PROT_NONE` and commits a sub-range, so the
    /// uncommitted remainder keeps M13's `ATTR_NONE`; stamping it would make it accessible.
    /// `subtract_range` keeps the order, so the result is sorted like the ranges.
    pub fn stamped_extents(&self, noaccess: &[(u64, u64)]) -> Vec<(u64, u64)> {
        let mut out = self.ranges.clone();
        for &(s, l) in noaccess { subtract_range(&mut out, s, l); }
        out
    }

    pub fn ranges(&self) -> &[(u64, u64)] { &self.ranges }
    pub fn view(&self) -> View { self.view }
    pub fn set_view(&mut self, view: View) { self.view = view; }
}

/// `[a, a+alen)` and `[b, b+blen)` share a byte.
fn overlap(a: u64, alen: u64, b: u64, blen: u64) -> bool {
    a < b.saturating_add(blen) && b < a.saturating_add(alen)
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: u64 = 0x4_0000_0000;      // a range base, page-aligned
    const WE: u64 = 0x2010_0020_3030_0000; // the probe host's commpage +0x110 (sprr.out)
    const PR: u64 = 0x2010_0020_3010_0000; // its +0x118
    const V8_FLAGS: u64 = 0x41842;         // MAP_JIT|MAP_ANON|MAP_NORESERVE|MAP_PRIVATE|MAP_UNIX03 (P5)

    #[test]
    fn admit_mmap_takes_prot_none_and_rwx_map_jit_and_passes_the_rest_through() {
        assert_eq!(JitSet::admit_mmap(0, V8_FLAGS), Ok(true), "V8's reservation (P5)");
        assert_eq!(JitSet::admit_mmap(7, 0x1802), Ok(true), "sprr.c's MAP_PRIVATE|MAP_ANON|MAP_JIT RWX page");
        assert_eq!(JitSet::admit_mmap(7, 0x1002), Ok(false), "an RWX map without MAP_JIT keeps today's path");
        assert_eq!(JitSet::admit_mmap(3, 0x1002), Ok(false), "an ordinary data map");
    }

    #[test]
    fn a_map_jit_mmap_with_another_prot_is_refused_by_value() {
        for prot in [1u64, 3, 5] {
            let e = JitSet::admit_mmap(prot, 0x1802).unwrap_err();
            assert!(e.starts_with(&format!("M48: MAP_JIT mmap prot {prot:#x}:")), "{e}");
        }
    }

    #[test]
    fn a_fixed_shared_or_file_map_jit_mmap_is_refused_by_value() {
        for flags in [0x1812u64 /* FIXED */, 0x1801 /* SHARED */, 0x802 /* no MAP_ANON */] {
            let e = JitSet::admit_mmap(7, flags).unwrap_err();
            assert!(e.starts_with(&format!("M48: MAP_JIT mmap flags {flags:#x}:")), "{e}");
        }
    }

    /// Review Focus item 4 (pure half): V8's shape, scaled down. The range is mapped `PROT_NONE`
    /// whole, then `+0x40000..` is committed, so its head stays no-access and only the committed
    /// part carries the view. A no-access extent elsewhere changes nothing, and an interior one splits.
    #[test]
    fn the_stamped_extents_are_the_ranges_minus_their_noaccess_extents() {
        let mut j = JitSet::default();
        j.add(B, 0x100_0000);
        assert!(j.stamped_extents(&[(B, 0x100_0000)]).is_empty(), "all PROT_NONE: nothing stamped");
        let noaccess = [(0x1_0000_0000, 0x4000), (B, 0x4_0000)];
        assert_eq!(j.stamped_extents(&noaccess), vec![(B + 0x4_0000, 0xfc_0000)],
            "the committed sub-range only, V8's mprotect(+0x40000, …, RWX)");
        let punched = [(B, 0x4_0000), (B + 0x10_0000, 0x8000)];
        assert_eq!(j.stamped_extents(&punched),
            vec![(B + 0x4_0000, 0xc_0000), (B + 0x10_8000, 0xef_8000)], "an interior guard splits it");
        assert_eq!(j.stamped_extents(&[]), vec![(B, 0x100_0000)], "no protection: the whole range");
    }

    #[test]
    fn admit_mprotect_admits_none_and_rwx_on_a_range_and_refuses_the_rest() {
        let mut j = JitSet::default();
        j.add(B, 0x10_0000);
        assert_eq!(j.admit_mprotect(B + 0x4_0000, 0xc_0000, 7), Ok(true), "V8's sub-range RWX commit");
        assert_eq!(j.admit_mprotect(B, 0x4000, 0), Ok(true), "a guard page inside the range");
        assert_eq!(j.admit_mprotect(B - 0x4000, 0x8000, 7), Ok(true), "a straddle still touches the range");
        assert_eq!(j.admit_mprotect(B - 0x4000, 0x4000, 1), Ok(false), "outside every range: not ours to judge");
        let e = j.admit_mprotect(B + 0x8000, 0x4000, 5).unwrap_err();
        assert!(e.starts_with(&format!("M48: MAP_JIT mprotect [{:#x}, +0x4000) prot 0x5 ", B + 0x8000)), "{e}");
    }

    #[test]
    fn a_partial_munmap_trims_and_splits_and_returns_what_it_removed() {
        let mut j = JitSet::default();
        j.add(B, 0x4_0000);
        assert_eq!(j.remove(B, B + 0x4000), vec![(B, 0x4000)], "a head trim");
        assert_eq!(j.ranges(), &[(B + 0x4000, 0x3_c000)]);
        assert_eq!(j.remove(B + 0x1_0000, B + 0x1_8000), vec![(B + 0x1_0000, 0x8000)], "an interior punch");
        assert_eq!(j.ranges(), &[(B + 0x4000, 0xc000), (B + 0x1_8000, 0x2_8000)], "split in two, still sorted");
        assert!(j.remove(0x1_0000_0000, 0x1_0000_4000).is_empty(), "a disjoint munmap removes nothing");
        assert_eq!(j.remove(B, B + 0x4_0000), vec![(B + 0x4000, 0xc000), (B + 0x1_8000, 0x2_8000)],
            "V8's whole-range munmap at exit takes both pieces");
        assert!(j.ranges().is_empty());
    }

    #[test]
    fn a_fixed_or_remap_overlap_is_refused_by_value_and_a_disjoint_one_is_not() {
        let mut j = JitSet::default();
        j.add(B, 0x10_0000);
        assert_eq!(j.refuse_overlap(B + 0x10_0000, 0x4000, "a FIXED mapping"), Ok(()), "adjacent above");
        assert_eq!(j.refuse_overlap(B - 0x4000, 0x4000, "a FIXED mapping"), Ok(()), "adjacent below");
        let e = j.refuse_overlap(B + 0xf_c000, 0x8000, "a FIXED mapping").unwrap_err();
        assert!(e.starts_with(&format!("M48: MAP_JIT range [{B:#x}, {:#x}) overlapped by a FIXED mapping at ",
            B + 0x10_0000)), "{e}");
    }

    #[test]
    fn the_mode_is_write_enabled_only_at_the_commpages_write_enable_word() {
        assert_eq!(JitSet::default().view(), View::Rx, "the set starts protected (R1)");
        assert_eq!(View::of_sprr(0, Some(WE)), View::Rx, "R1's initial 0 protects");
        assert_eq!(View::of_sprr(PR, Some(WE)), View::Rx, "+0x118 protects");
        assert_eq!(View::of_sprr(WE, Some(WE)), View::Rw, "+0x110 write-enables");
        assert_eq!(View::of_sprr(WE, None), View::Rx, "no SPRR commpage: nothing write-enables (T6-f)");
        assert_eq!(View::of_sprr(0, Some(0)), View::Rx, "a zero commpage word never write-enables 0");
    }
}
```

Register the module in `crates/retrace-box/src/lib.rs` beside `pub mod kq;`: `pub mod jit;`.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --lib --no-fail-fast -- --test-threads=1 jit:: thread:: > $L/t6-unit.log 2>&1; echo "exit=$?"; grep -a -E '^test |test result' $L/t6-unit.log | tail -25
```

Expected: exit 0; the eight `jit::tests` and every `thread::tests` test pass, `a_spawned_thread_starts_protected_whatever_its_creator_wrote` among them. (`jit::` and `thread::` are libtest name filters; the module paths are `jit::tests::…` and `thread::tests::…`.)

- [ ] **Step 6: The box-level tests, red first.** Create `crates/retrace-box/tests/jit.rs`:

```rust
//! M48 Task 6, box level: the SPRR register, the `MAP_JIT` view over stage 1, and the step-safe
//! flush, driven through `Box_`'s public methods on a static box (the `checkpointparity.rs`
//! pattern). A static guest has no commpage (Ruling T6-f), so `tb()` stages one where
//! `load_dynamic` freezes the host's, holding the three words `pthread_jit_write_protect_np`
//! reads (§2c, `sprr.out`).
use retrace_box::{jit::View, Box_, COMMPAGE_IPA};

const ANON: u64 = 0x1002;         // MAP_ANON | MAP_PRIVATE
const FIXED: u64 = 0x10;          // MAP_FIXED
const JIT_RWX: u64 = 0x1802;      // MAP_JIT | MAP_ANON | MAP_PRIVATE, sprr.c's page
const V8_FLAGS: u64 = 0x41842;    // V8's reservation (P5)
const WE: u64 = 0x2010_0020_3030_0000; // commpage +0x110 on the probe host: write-enable
const PR: u64 = 0x2010_0020_3010_0000; // +0x118: protect

fn tb() -> Box_ {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    assert_eq!(b.guest_mmap(COMMPAGE_IPA, 0x4000, 3, ANON | FIXED), Ok(COMMPAGE_IPA));
    b.poke_guest(COMMPAGE_IPA + 0x10c, &[3]); // SPRR (§2c)
    b.poke_guest(COMMPAGE_IPA + 0x110, &WE.to_le_bytes());
    b.poke_guest(COMMPAGE_IPA + 0x118, &PR.to_le_bytes());
    b
}

/// Review Focus item 4 (box half). V8 maps its code range `PROT_NONE` with `MAP_JIT`, then
/// `mprotect`s a sub-range RWX. `guest_mprotect` routes that through `unprotect`, which stamps
/// `ATTR_DATA` unconditionally: without the view's restamp the committed range would be writable and
/// non-executable under the protected view, and V8's first call into it would fault.
#[test]
fn an_unprotect_inside_a_jit_range_is_restamped_by_the_view_not_left_data() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x10_0000, 0, V8_FLAGS).unwrap();
    assert_eq!(b.dbg_jit().ranges(), &[(a, 0x10_0000)], "the MAP_JIT range is tracked");
    assert!(b.ipa_is_noaccess(a) && b.ipa_is_noaccess(a + 0xf_c000), "mapped PROT_NONE whole");
    b.guest_mprotect(a + 0x4_0000, 0xc_0000, 7); // V8's commit, scaled down (P5)
    assert!(b.ipa_is_noaccess(a + 0x3_c000), "the uncommitted head keeps M13's ATTR_NONE");
    assert!(b.ipa_is_exec(a + 0x4_0000) && b.ipa_is_exec(a + 0xf_c000),
        "protected view: the committed range is ATTR_CODE, not the ATTR_DATA unprotect stamped");
    assert!(!b.ipa_is_el0_writable(a + 0x4_0000), "and not writable");
    b.sprr_write(WE);
    assert_eq!(b.dbg_jit().view(), View::Rw, "the write-enable flipped the view");
    assert!(b.ipa_is_el0_writable(a + 0x4_0000) && !b.ipa_is_exec(a + 0x4_0000), "write-enabled: ATTR_DATA");
    assert!(b.ipa_is_noaccess(a + 0x3_c000), "the view never stamps a no-access page");
    b.sprr_write(PR);
    assert!(b.ipa_is_exec(a + 0x4_0000) && !b.ipa_is_el0_writable(a + 0x4_0000), "protected again");
    assert_eq!(b.threads().sprr_of(0), PR, "the register holds what was written, for the read-back");
}

/// §3f "Flips": the view is the RUNNING thread's mode. A switch to a thread in the other mode
/// flips it, and the switch back restores it, with no write in between.
#[test]
fn the_view_follows_the_running_thread_across_a_switch() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x4000, 7, JIT_RWX).unwrap();
    assert!(b.ipa_is_exec(a), "a new range carries the current view: protected (R1)");
    // A second thread from the live vCPU, the checkpointparity.rs pattern.
    let mut child = b.save_ctx();
    child.regs.sp_el0 -= 0x2000;
    let tid = b.threads_mut().spawn(child, (0, 0));
    b.sprr_write(WE);
    assert!(b.ipa_is_el0_writable(a), "main's own write flips the view");
    b.switch_to_thread(tid);
    assert_eq!(b.threads().sprr_of(tid), 0, "the child was never write-enabled (R1)");
    assert!(b.ipa_is_exec(a) && !b.ipa_is_el0_writable(a), "a switch to a protected thread flips back");
    b.switch_to_thread(0);
    assert!(b.ipa_is_el0_writable(a), "and the switch back restores main's mode");
}

/// P9 (walls.md §1, §4 item 2): a view flip inside `step()` runs `flush_guest_tlb` with
/// `MDSCR_EL1.SS` armed. Before M48 the EL1 stub then took a software-step exception and panicked
/// (`tlbi stub faulted at EL1: EC=SoftStep`).
#[test]
fn a_flush_with_the_step_bits_armed_does_not_step_the_stub() {
    let mut b = tb();
    b.dbg_leak_ss(); // MDSCR_EL1.SS and PSTATE.SS, armed exactly as step() arms them
    let before = b.regs_snapshot();
    b.flush_guest_tlb();
    assert_eq!(b.regs_snapshot(), before, "every register restored, PSTATE.SS included");
    assert_eq!(b.dbg_watch0_hw().2 & 1, 1, "MDSCR_EL1.SS restored for the step that armed it");
}

/// R2: only the commpage's two words are admitted. The refusal names the value and the pc.
#[test]
#[should_panic(expected = "M48: SPRR write 0x1 at pc ")]
fn an_inadmissible_sprr_value_is_refused_by_value() {
    let mut b = tb();
    b.sprr_write(1);
}

/// Ruling T6-d: a munmap of a JIT range trims it, and the released pages go back to `ATTR_DATA`,
/// so a later ordinary mapping there does not inherit an `ATTR_CODE` leaf (M13's argument). Ruling
/// T6-c: a FIXED map over what remains is refused by value before anything changes. This is the
/// one test file that names the `M48: MAP_JIT ` prefix (Task 10's audit, check 9); the other
/// `MAP_JIT` refusals are pinned by `jit.rs`'s unit tests.
#[test]
fn a_munmap_trims_a_jit_range_to_data_pages_and_a_fixed_map_over_one_is_refused() {
    let mut b = tb();
    let a = b.guest_mmap(0, 0x1_0000, 7, JIT_RWX).unwrap();
    assert!(b.ipa_is_exec(a) && b.ipa_is_exec(a + 0xc000), "protected view over the whole range");
    b.guest_munmap(a, 0x4000); // a head trim
    assert_eq!(b.dbg_jit().ranges(), &[(a + 0x4000, 0xc000)]);
    assert!(b.ipa_is_el0_writable(a) && !b.ipa_is_exec(a), "the released page is back to ATTR_DATA");
    assert!(b.ipa_is_exec(a + 0x4000), "the rest keeps the view");
    // The refusal is a panic (R5). `place_fixed` raises it before it touches anything, so the box
    // stays usable; the unwind leaks only the fresh host pages `guest_mmap` allocated.
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.guest_mmap(a + 0x4000, 0x4000, 3, ANON | FIXED)
    })).expect_err("a FIXED map over a MAP_JIT range must be refused");
    let msg = refused.downcast_ref::<String>().expect("a formatted refusal");
    assert!(msg.starts_with(&format!("M48: MAP_JIT range [{:#x}, {:#x}) overlapped by a FIXED mapping at ",
        a + 0x4000, a + 0x1_0000)), "{msg}");
    assert_eq!(b.dbg_jit().ranges(), &[(a + 0x4000, 0xc000)], "a refusal changes nothing");
    assert!(b.ipa_is_exec(a + 0x4000), "and leaves the view stamped");
    b.guest_munmap(a + 0x4000, 0xc000);
    assert!(b.dbg_jit().ranges().is_empty(), "the whole range is gone");
    assert_eq!(b.guest_mmap(a, 0x1_0000, 3, ANON | FIXED), Ok(a));
    assert!(b.ipa_is_el0_writable(a + 0xc000) && !b.ipa_is_exec(a + 0xc000),
        "a data map over the old range is plain data");
}
```

Run red:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --test jit -- --test-threads=1 > $L/t6-red-box.log 2>&1; echo "exit=$?"; grep -a -E 'error|cannot find|no method' $L/t6-red-box.log | head
```

Expected: a compile failure naming `sprr_write`, `dbg_jit` and `ipa_is_el0_writable`, which do not exist yet. The jit module exists from Step 5, so `jit::View` resolves.

- [ ] **Step 7: The box.** In `crates/retrace-box/src/lib.rs`:

  **7a. Imports and constants.**
  - Line 2 becomes `use retrace_arch::{decode_excl, ec_of, Ec, ExclInsn, SprrAccess};`.
  - Below `COMMPAGE_TIMEBASE_OFFSET_IPA`, add:
    ```rust
    /// M48 §2c: the two commpage words `pthread_jit_write_protect_np` loads and `msr`s into
    /// `S3_6_C15_C1_5` (`pthread-jit-disasm.txt`): `+0x110` write-enables the thread's `MAP_JIT`
    /// pages and `+0x118`, eight bytes on, protects them (t0 M1(c)). Frozen with the rest of the
    /// commpage, so record and replay admit the same two values (R2).
    const COMMPAGE_SPRR_IPA: u64 = COMMPAGE_IPA + 0x110;
    ```

  **7b. SCTLR UCI.** Replace the comment above `SCTLR_MMU_ON_BASE` and the constant:
  ```rust
  // base 0x30d00800 + M(1) + C(4) + I(0x1000) + DZE(0x4000) + UCI(0x0400_0000). PAC is NOT in the
  // base: it is per-guest (see below). DZE(14) is SET so EL0 `DC ZVA` executes natively instead of
  // trapping to EL1 with EC 0x18: Apple's `_platform_memset` issues `DC ZVA` above a size threshold,
  // and CPython's allocator hits that threshold at startup (M25-cpython). UCI(26) is SET for the
  // same reason (M48 §11b item 3, R8): `sys_icache_invalidate` issues EL0 `IC IVAU` per 64-byte line
  // after V8 writes JIT code (walls.md §1 row 5, t0 M1(b)). UCT(15) stays CLEAR: t0 M1(b) found no
  // `CTR_EL0` read in it and no `DC CVAU`, and the EC 0x18 exit still fails loud on a guest that
  // needs either.
  const SCTLR_MMU_ON_BASE: u64 = 0x30d0_0800 | 1 | 4 | 0x1000 | 0x4000 | 0x0400_0000;
  ```
  Rewrite the `sctlr_dze_tests` module's comment and its one test (Ruling T6-i):
  ```rust
  // M25-cpython, M48. EL0 `DC ZVA` traps to EL1 (EC 0x18) when SCTLR_EL1.DZE is clear, and EL0
  // `IC IVAU` does when UCI is clear. Apple's `_platform_memset` issues the first above a size
  // threshold (CPython hits it at startup), and `sys_icache_invalidate` the second after V8 writes
  // JIT code. `sctlr_mmu_on` is the one derivation all four SCTLR install sites route through, so
  // pinning both there fixes every guest, not just CPython's or node's.
  #[cfg(test)]
  mod sctlr_dze_tests {
      use super::*;

      // DZE(14) and UCI(26) must be SET. UCT(15) stays CLEAR: t0 M1(b) measured
      // `sys_icache_invalidate` and found no `CTR_EL0` read, so nothing needs it, and the EC 0x18
      // exit is the fail-loud path for a guest that does. "Nothing else" is asserted as the whole
      // value, because a bit set speculatively is the "right conclusion resting on an unmeasured
      // supporting fact" this repo keeps catching.
      #[test]
      fn sctlr_enables_dc_zva_and_ic_ivau_for_el0_and_nothing_else() {
          for pac_enabled in [false, true] {
              let sctlr = sctlr_mmu_on(pac_enabled);
              assert!(sctlr & 0x4000 != 0, "DZE (bit 14) must be SET: {sctlr:#x}");
              assert!(sctlr & 0x0400_0000 != 0, "UCI (bit 26) must be SET: {sctlr:#x}");
              assert!(sctlr & 0x8000 == 0, "UCT (bit 15) must stay CLEAR: {sctlr:#x}");
              assert_eq!(sctlr & !SCTLR_PAC_EN, 0x30d0_0800 | 1 | 4 | 0x1000 | 0x4000 | 0x0400_0000,
                  "the base is M, C, I, DZE and UCI over 0x30d00800, and nothing else: {sctlr:#x}");
          }
      }
  }
  ```

  **7c. The `jit` field through every path** (Task 4's pattern; Task 10's audit counts the seven `jit: ` lines against `kq`'s):
  - In `pub struct Box_`, after Task 5's `psynch` (which follows Task 4's `gkq`, after `excl`):
    ```rust
        /// M48 §3f: every `MAP_JIT` range and the view stamped over them. Box state, rebuilt on both
        /// sides from the guest's own `mmap`/`mprotect`/`munmap` and its own `msr`s, and carried
        /// through every rebuild path (`BoxState`). It holds a `Vec`, but it comes after `vcpu`/`vm`,
        /// so the load-bearing vcpu-before-vm drop order is unaffected.
        jit: jit::JitSet,
    ```
  - In `pub struct BoxState`, after Task 5's `psynch`:
    ```rust
        // M48 §3f: carried because a mid-run capture cannot re-derive it. The MAP_JIT maps and the
        // toggles that set the view happened behind the checkpoint. The stamps ride in `mem`, and
        // `from_checkpoint` asserts that they agree with this set (Ruling T6-g).
        pub jit: jit::JitSet,
    ```
  - In `checkpoint()`, after Task 5's `psynch` line: `jit: self.jit.clone(),`.
  - In `from_checkpoint`'s literal, after Task 5's `psynch` line: `jit: state.jit.clone(),`. Then, after `if state.cache_installed { b.install_cache_pager(); }`, add `b.assert_jit_stamped();`.
  - In each of the three one-line literals (`load_with_pac`, `load_dynamic`, `restore`), append `, jit: jit::JitSet::default()` as the last field, after Task 5's `psynch`. `restore()` rebuilds landmark 0, where no `MAP_JIT` range exists yet and every thread holds 0.
  - In `dbg_internal_state`, append ` jit={:?}` to the format string, after Task 5's ` psynch={:?}`, with `self.jit` as the matching argument.
  - Register the module: `pub mod jit;` after `pub mod kq;` (done in Step 5).

  Every comment written in this step avoids the text ` jit: ` outside these seven lines, so the audit's count stays exact.

  **7d. The register.** Replace `try_emulate_undef_mrs` (its doc comment and body):
  ```rust
      /// Emulate a trapped Apple IMPDEF system-register access that surfaces as an UNDEFINED
      /// instruction (EC=0x00, not the EC=0x18 sysreg-trap path) because HVF does not expose the
      /// register to the guest: `S3_6_C15_C1_5`, the SPRR register, in both directions (M48 §3f,
      /// §11b item 2).
      /// - `mrs Xt` answers the running thread's value (`Thread.sprr`). That is 0 until the thread
      ///   writes it (R1), which is what this arm answered for every read before M48: libdyld probes
      ///   bit 36 and, seeing it clear, takes its normal PAC-authenticated path (§2d).
      /// - `msr S3_6_C15_C1_5, Xt` is [`sprr_write`](Self::sprr_write): admitted by value or refused.
      ///
      /// Either way the instruction is skipped. Deterministic and identical on record and replay (both
      /// re-execute the same instructions), so nothing is recorded (R3). Any other undefined
      /// instruction returns false → surfaced as `Stop::Other` for diagnosis.
      fn try_emulate_undef_mrs(&mut self) -> bool {
          let elr = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
          if self.host_span(elr).is_none() { return false; }
          let insn = u32::from_le_bytes(self.read_guest(elr, 4).try_into().unwrap());
          let Some(access) = retrace_arch::decode_sprr_access(insn) else { return false };
          // Skip it first: a write may run the TLBI stub, which saves and restores this resume state.
          let spsr = self.vcpu.get_sys(sysreg::SPSR_EL1).unwrap();
          self.vcpu.set_reg(reg::PC, elr + 4).unwrap();
          self.vcpu.set_reg(reg::CPSR, spsr).unwrap();
          match access {
              SprrAccess::Read { rt } => {
                  let v = self.threads.sprr_of(self.threads.current());
                  if rt != 31 { self.vcpu.set_reg(reg::x(rt), v).unwrap(); } // x31 = XZR: value discarded
              }
              // x31 = XZR: a write of 0, which R2 refuses by value.
              SprrAccess::Write { rt } => { let v = self.xreg(rt); self.sprr_write(v); }
          }
          true
      }
  ```
  After it, add the register's methods:
  ```rust
      /// M48 §3f: the running thread's `msr S3_6_C15_C1_5, Xt`, emulated below the trace through
      /// `try_emulate_undef_mrs`, from `run()` and `step()` alike. It admits exactly the two words
      /// `pthread_jit_write_protect_np` loads from the commpage (R2): `+0x110` write-enables the
      /// thread's `MAP_JIT` pages and `+0x118` protects them. Anything else, and any write by a guest
      /// with no SPRR commpage (Ruling T6-f), is refused by value and pc, with the same panic on
      /// record and replay (R5). An admitted value is stored on the current thread, so pthread's
      /// read-back (`mrs`, then `brk #1` on a mismatch) sees it, and the view is synced last.
      pub fn sprr_write(&mut self, value: u64) {
          let pc = self.vcpu.get_sys(sysreg::ELR_EL1).unwrap();
          let Some((we, pr)) = self.sprr_admitted() else {
              panic!("M48: SPRR write {value:#x} at pc {pc:#x}: this guest has no SPRR commpage \
                      (+0x110 and +0x118 absent or equal), so no value is admissible (R2, Ruling T6-f)");
          };
          assert!(value == we || value == pr,
              "M48: SPRR write {value:#x} at pc {pc:#x} is neither the commpage's write-enable word \
               {we:#x} (+0x110) nor its protect word {pr:#x} (+0x118) (R2)");
          let tid = self.threads.current();
          self.threads.set_sprr_of(tid, value);
          self.sync_jit_view();
      }

      /// M48 §3f (R2): `(write_enable, protect)`, the commpage's `+0x110` and `+0x118` words. None
      /// when the guest has no commpage (a static guest) or the two words cannot tell the modes apart
      /// (Ruling T6-f).
      fn sprr_admitted(&self) -> Option<(u64, u64)> {
          let b = self.read_guest_checked(COMMPAGE_SPRR_IPA, 16)?;
          let we = u64::from_le_bytes(b[..8].try_into().unwrap());
          let pr = u64::from_le_bytes(b[8..].try_into().unwrap());
          (we != pr).then_some((we, pr))
      }

      /// M48 §3f: the running thread's mode (Ruling T6-e).
      fn jit_mode(&self) -> jit::View {
          jit::View::of_sprr(self.threads.sprr_of(self.threads.current()), self.sprr_admitted().map(|(we, _)| we))
      }

      /// M48 §3f "Flips": make the view the running thread's mode. Called after that thread's own
      /// write and after every thread switch, the only two events that change either side, so the
      /// view always equals the running thread's mode (`assert_jit_stamped` checks it at a restore).
      fn sync_jit_view(&mut self) {
          let mode = self.jit_mode();
          if mode != self.jit.view() { self.restamp_jit(mode); }
      }

      /// M48 §3f: stamp `view` over every `MAP_JIT` range minus its no-access extents (§11b item 4),
      /// record it, and flush the guest TLB once. Called on a flip, when a range is mapped, and after
      /// `guest_mprotect` touches a range. W^X holds by construction: `Rx` is `ATTR_CODE`, `Rw` is
      /// `ATTR_DATA`, never both. The flush is a correctness requirement, as in `protect_none`: the
      /// guest may already hold a translation for these pages.
      fn restamp_jit(&mut self, view: jit::View) {
          let attr = match view { jit::View::Rx => ATTR_CODE, jit::View::Rw => ATTR_DATA };
          let extents = self.jit.stamped_extents(&self.noaccess);
          for &(s, l) in &extents { self.set_region_attr(s, l, attr); }
          self.jit.set_view(view);
          if !extents.is_empty() { self.flush_guest_tlb(); }
      }

      /// M48 §3f (Ruling T6-d): drop page-aligned `[start, end)` from the `MAP_JIT` set and put its
      /// pages back to `ATTR_DATA`, the identity default every anonymous mapping starts from.
      /// Otherwise the next mapping there inherits an `ATTR_CODE` leaf its guest never asked for:
      /// M13's `drop_protection` argument, for the view. `drop_protection` has already reset any
      /// no-access part. Under a write-enabled view the leaves are `ATTR_DATA` already.
      fn unmap_jit(&mut self, start: u64, end: u64) {
          let gone = self.jit.remove(start, end);
          if gone.is_empty() || self.jit.view() == jit::View::Rw { return; }
          for &(s, l) in &gone { self.set_region_attr(s, l, ATTR_DATA); }
          self.flush_guest_tlb();
      }

      /// M48 §3f, Ruling T6-g: a restored box's view must already be what stage 1 holds over every
      /// stamped page, and its current thread's mode. The stamps ride in `mem`, so a mismatch means
      /// `BoxState` lost the set or the view: a restore that replays fine and diverges on a seek
      /// (M24's class). Asserted, never repaired. A fresh vCPU's TLB is empty, so nothing needs a
      /// flush (§11a item 3).
      fn assert_jit_stamped(&self) {
          let (view, mode) = (self.jit.view(), self.jit_mode());
          assert_eq!(view, mode, "M48: a restored MAP_JIT view {view:?} is not thread {}'s mode {mode:?}",
              self.threads.current());
          let attr = match view { jit::View::Rx => ATTR_CODE, jit::View::Rw => ATTR_DATA };
          for (s, l) in self.jit.stamped_extents(&self.noaccess) {
              // Both ends of each extent. `restamp_jit` writes an extent whole, so a lost or stale
              // stamp shows at its ends, and walking all 16 Ki pages of node's code range on every
              // debugger restore would cost one linear backing search per page (`leaf_desc`).
              for p in [s, s + l - GRANULE as u64] {
                  let got = self.leaf_desc(p).map(|d| d & !PT_ADDR & !0x3);
                  assert_eq!(got, Some(attr),
                      "M48: restored MAP_JIT page {p:#x} carries stage-1 attributes {got:x?}, not the {view:?} view's {attr:#x}");
              }
          }
      }
  ```

  **7e. The map hooks.**
  - At the top of `map_mmap_region`, before the exec-alignment `if`:
    ```rust
            // M48 §3f: a MAP_JIT request is admitted by value before anything is placed (Rulings T6-a,
            // T6-b), refused identically on record and replay (R5). Every admitted one is non-FIXED, so
            // it leaves through the tail below, never through the FIXED branch's early return.
            let is_jit = jit::JitSet::admit_mmap(prot, flags).unwrap_or_else(|m| panic!("{m}"));
    ```
  - At its tail, between `if prot == 0 { self.protect_none(ipa, rlen as u64); }` and `Ok(ipa)`:
    ```rust
            // M48 §3f: a new MAP_JIT range carries the current view, the running thread's mode, from
            // its first instruction. A PROT_NONE one (V8's reservation, P5) stamps nothing until an
            // mprotect commits part of it.
            if is_jit {
                self.jit.add(ipa, rlen as u64);
                self.restamp_jit(self.jit.view());
            }
    ```
  - In `place_fixed`, after its `fixed_fits` assert:
    ```rust
            // M48 §3f (Ruling T6-c): every FIXED path funnels through here, and none may land on a
            // MAP_JIT range, whose view would stay stamped over the new mapping's pages.
            if let Err(m) = self.jit.refuse_overlap(addr, rlen as u64, "a FIXED mapping") { panic!("{m}"); }
    ```
  - In `guest_vm_remap`, after its two alignment asserts:
    ```rust
            // M48 §3f (Ruling T6-c): an alias copies one moment's stamp, so an alias of a MAP_JIT page
            // would stop following the toggles, and a target inside a range would be restamped by them.
            for (a, what) in [(src, "a mach_vm_remap source"), (target, "a mach_vm_remap target")] {
                if let Err(m) = self.jit.refuse_overlap(a, size, what) { panic!("{m}"); }
            }
    ```
  - In `guest_munmap` (Task 3's version), replace `if end > start { self.unmap_range(start, end); }` with:
    ```rust
            if end > start {
                // M48 §3f (Ruling T6-d): leave the MAP_JIT set first, so its leaves are back to
                // ATTR_DATA before the backings go.
                self.unmap_jit(start, end);
                self.unmap_range(start, end);
            }
    ```
  - Replace `guest_mprotect`'s body, and append a paragraph to its doc comment:
    ```rust
        /// M48 §3f: an `mprotect` touching a `MAP_JIT` range is admitted by value first (Ruling
        /// T6-a), and the view is restamped after it (Review Focus item 4).
        pub fn guest_mprotect(&mut self, ipa: u64, len: u64, prot: u64) {
            let in_jit = self.jit.admit_mprotect(ipa, len, prot).unwrap_or_else(|m| panic!("{m}"));
            let end = ipa.saturating_add(len);
            if prot == 0 {
                self.protect_none(ipa, len);
            } else if self.noaccess.iter().any(|&(s, l)| ipa < s + l && s < end) {
                self.unprotect(ipa, len);
            } else {
                let _ = self.vm.protect(ipa, len as usize, MemFlags::RWX);
            }
            // Review Focus item 4: `unprotect` stamps ATTR_DATA unconditionally, so V8's RWX commit of
            // its PROT_NONE code range (P5) would leave the range writable and non-executable under the
            // protected view, and V8's first call into it would fault.
            if in_jit { self.restamp_jit(self.jit.view()); }
        }
    ```
    The three branches are today's three early returns, unchanged in what they do.

  **7f. The switch.** In `switch_to_thread`, after `self.load_ctx(&next);`:
  ```rust
          // M48 §3f: the view follows the running thread. Native gives each thread its own SPRR
          // register, and with one vCPU a switch is when the register the hardware consults changes
          // (`jitwp_dyn twothreads`).
          self.sync_jit_view();
  ```

  **7g. The step-safe flush (P9).** Replace `flush_guest_tlb`'s body:
  ```rust
      pub fn flush_guest_tlb(&mut self) {
          self.ensure_tlbi_stub();
          // M48 (P9, walls.md §1): a JIT view flip runs this inside `step()`, with MDSCR_EL1.SS armed,
          // and the EL1 stub then takes a software-step exception (`tlbi stub faulted at EL1:
          // EC=SoftStep`). Clear SS, and MDE with it, for the stub's run, and restore both after.
          // `save_state` restores PSTATE.SS with the rest of the caller's state.
          let mdscr = self.vcpu.get_sys(sysreg::MDSCR_EL1).unwrap();
          self.vcpu.set_sys(sysreg::MDSCR_EL1, mdscr & !(MDSCR_SS | MDSCR_MDE)).unwrap();
          let saved = self.save_state();
          self.vcpu.set_reg(reg::PC, TLBI_STUB_IPA).expect("set PC (tlbi stub)");
          self.vcpu.set_reg(reg::CPSR, TLBI_STUB_CPSR).expect("set CPSR (tlbi stub)");
          self.run_tlbi_stub();
          self.restore_state(&saved);
          self.vcpu.set_sys(sysreg::MDSCR_EL1, mdscr).unwrap();
      }
  ```
  Append to its doc comment: "M48: safe inside `step()`: the step and debug bits are cleared for the stub's run and restored after."

  **7h. The observables.** After `ipa_is_noaccess`:
  ```rust
      /// Test/diagnostic observable (M48): does the stage-1 leaf for `ipa` give EL0 read and write
      /// (`ATTR_DATA`: AP `0b01`)? The twin of [`ipa_is_noaccess`](Self::ipa_is_noaccess), exact for
      /// the same reason: the AP field names each of the four attributes this box installs.
      pub fn ipa_is_el0_writable(&self, ipa: u64) -> bool {
          let Some(leaf) = self.leaf_desc(ipa) else { return false };
          leaf & 0x3 != 0 && leaf & 0xC0 == 0x40
      }
  ```
  After `dbg_kq_mut`:
  ```rust
      /// Test-only (M48): the `MAP_JIT` set and its view, for `checkpointparity.rs` and the box-level
      /// JIT tests, which read its fields rather than `dbg_internal_state`'s string.
      #[doc(hidden)]
      pub fn dbg_jit(&self) -> &jit::JitSet { &self.jit }
  ```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --test jit --no-fail-fast -- --test-threads=1 > $L/t6-box-jit.log 2>&1; echo "exit=$?"; grep -a -E '^test |test result' $L/t6-box-jit.log
cargo test -p retrace-box --lib --no-fail-fast -- --test-threads=1 > $L/t6-box-lib.log 2>&1; echo "exit=$?"; grep -a 'test result' $L/t6-box-lib.log
```

Expected: both exit 0; the five `jit` tests pass, and the lib target passes with `sctlr_enables_dc_zva_and_ic_ivau_for_el0_and_nothing_else` in place of the old name.

- [ ] **Step 8: `checkpointparity.rs`, the M48 tier.** In `crates/retrace-box/tests/checkpointparity.rs`:
  1. In `assert_checkpoint_parity`, capture `let jit = b.dbg_jit().clone();` beside `let kq = b.dbg_kq().clone();`, and after the `dbg_kq` assertion add:
     ```rust
         assert_eq!(r.dbg_jit(), &jit, "{label}: the MAP_JIT set and its view (M48)");
     ```
     `Thread.sprr` needs no row of its own: the existing `format!("{:?}", r.threads())` comparison prints every `Thread` field.
  2. Append the tier:
     ```rust
     /// M48: the write-enabled JIT tier. It stages a commpage (a static guest has none), a V8-shaped
     /// MAP_JIT range (PROT_NONE whole, a sub-range committed RWX), a second thread, and main
     /// write-enabled, so the view is `Rw` and the two threads' registers differ. No other tier makes
     /// `jit` or `Thread.sprr` non-default, so without this one both compare Default == Default. The
     /// restore also runs `assert_jit_stamped` (Ruling T6-g), which fails loud if the stamps carried in
     /// `mem` and the carried view disagree.
     #[test]
     fn a_checkpointed_box_in_a_write_enabled_jit_window_matches_the_box_it_came_from() {
         let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
         let mut b = Box_::load(&loaded);
         let _ = b.run(); // mid-run
         let (we, pr) = (0x2010_0020_3030_0000u64, 0x2010_0020_3010_0000u64); // sprr.out's +0x110, +0x118
         let cp = retrace_box::COMMPAGE_IPA;
         assert_eq!(b.guest_mmap(cp, 0x4000, 3, 0x1012), Ok(cp), "stage a commpage (MAP_ANON|MAP_PRIVATE|MAP_FIXED)");
         b.poke_guest(cp + 0x110, &we.to_le_bytes());
         b.poke_guest(cp + 0x118, &pr.to_le_bytes());
         let a = b.guest_mmap(0, 0x10_0000, 0, 0x41842).unwrap(); // V8's MAP_JIT reservation (P5)
         b.guest_mprotect(a + 0x4_0000, 0xc_0000, 7);
         let mut child = b.save_ctx();
         child.regs.sp_el0 -= 0x2000;
         let tid = b.threads_mut().spawn(child, (0, 0));
         b.sprr_write(we);

         // PRECONDITIONS. Without these the M48 rows compare Default == Default.
         assert_eq!(b.dbg_jit().view(), retrace_box::jit::View::Rw, "precondition: main write-enabled the view");
         assert_eq!(b.dbg_jit().ranges(), &[(a, 0x10_0000)], "precondition: a MAP_JIT range");
         assert_eq!((b.threads().sprr_of(0), b.threads().sprr_of(tid)), (we, 0),
             "precondition: the two threads' registers differ, or an index swap is invisible");
         assert!(b.ipa_is_el0_writable(a + 0x4_0000) && b.ipa_is_noaccess(a),
             "precondition: stamps the restore must agree with, and a no-access head it must not stamp");

         assert_checkpoint_parity(b, "jit");
     }
     ```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-box --test checkpointparity --no-fail-fast -- --test-threads=1 > $L/t6-cpparity.log 2>&1; echo "exit=$?"; grep -a -E '^test |test result' $L/t6-cpparity.log
```

Expected: exit 0, with five tests passing (four existing and the new tier).

- [ ] **Step 9: The fixtures.**

  **9a. `crates/retrace-guest/asm/sprrprobe.s`.** Create it with exactly the text Task 0 Step 3 measured as `$L/t0/sprrprobe.s` (copy that file, then `diff` it against this block; they must match):

```asm
// M48: the SPRR register and EL0 cache maintenance on a static guest (no commpage).
// The mrs must read 0 (R1), the ic ivau must run at EL0 (SCTLR.UCI), and the msr must be refused
// by value, because a static guest has no commpage to admit a value from. exit(2) means the mrs
// read nonzero.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mrs  x0, S3_6_C15_C1_5
    cbnz x0, 1f
    adr  x3, _start
    ic   ivau, x3
    dsb  ish
    isb
    mov  x1, #1
    msr  S3_6_C15_C1_5, x1
    mov  x0, #0
    b    2f
1:  mov  x0, #2
2:  mov  x16, #1
    svc  #0x80
```

  **9b. `crates/retrace-guest/c/jitwp_dyn.c`:**

```c
// M48 Task 6: JIT write-protect (spec §3f, §3h). MAP_JIT pages, toggled per thread through
// pthread_jit_write_protect_np, which writes S3_6_C15_C1_5 (§2c). stdout is line-buffered, so each
// line is its own write(2) and so a landmark: the seek test needs one inside a write-enabled window.
// Modes, from argv[1]:
//   basic       native's sprr.c sequence (docs/sweep-evidence/2026-10-02-m48-static/sprr.c): the
//               commpage words, the register around each toggle, 42 from JIT code, a child's register.
//   v8          V8's shape (P5): a 1 MiB MAP_JIT reservation mapped PROT_NONE, the part from +0x40000
//               mprotected RWX, code written write-enabled and run protected. Then the code page is
//               decommitted (PROT_NONE) and recommitted (RWX) while protected, and called again with no
//               toggle in between: only the view's restamp after `unprotect` makes that call executable
//               (Review Focus item 4). Then a whole munmap.
//   twothreads  thread A stays write-enabled while thread B, protected, runs the page A writes. They
//               hand off through dispatch semaphores, so each hand-off blocks one thread and the
//               cooperative scheduler switches to the other: the view must follow the running thread.
//   fault       a store to a MAP_JIT page while protected (every thread starts so) faults, as native.
// The `JITWP page=` marker names the page; its address differs natively, so the e2e never compares it.
#include <dispatch/dispatch.h>
#include <libkern/OSCacheControl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

static unsigned long long rd(void) { uint64_t v; __asm__ volatile("mrs %0, S3_6_C15_C1_5" : "=r"(v)); return v; }
static unsigned long long cp64(uintptr_t off) { return *(volatile uint64_t *)(0xfffffc000ULL + off); }

static uint32_t *jit_page(void) {
    void *p = mmap(NULL, 0x4000, PROT_READ | PROT_WRITE | PROT_EXEC, MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
    if (p == MAP_FAILED) { puts("mmap failed"); return NULL; }
    printf("JITWP page=%p\n", p);
    return p;
}

// `mov x0, #n; ret` at p, written while write-enabled, then made visible to instruction fetch.
static void emit(uint32_t *p, unsigned n) {
    p[0] = 0xd2800000u | (n << 5);
    p[1] = 0xd65f03c0u;
    sys_icache_invalidate(p, 8);
}

static void *child(void *a) { (void)a; printf("child initial sprr=%#llx\n", rd()); return NULL; }

static int basic(void) {
    printf("commpage +0x10c=%u +0x110=%#llx +0x118=%#llx\n",
           (unsigned)*(volatile uint8_t *)0xfffffc10cULL, cp64(0x110), cp64(0x118));
    printf("main initial sprr=%#llx\n", rd());
    uint32_t *p = jit_page();
    if (!p) return 2;
    pthread_jit_write_protect_np(0); printf("main write-en sprr=%#llx\n", rd());
    p[0] = 0xd2800540u; p[1] = 0xd65f03c0u; // mov x0, #42; ret
    pthread_jit_write_protect_np(1); printf("main protect sprr=%#llx\n", rd());
    sys_icache_invalidate(p, 8);
    printf("jit call=%d\n", ((int (*)(void))p)());
    pthread_jit_write_protect_np(0);
    pthread_t th; pthread_create(&th, NULL, child, NULL); pthread_join(th, NULL);
    printf("main after-child sprr=%#llx\n", rd());
    return 0;
}

static int v8(void) {
    const size_t R = 0x100000, OFF = 0x40000;
    uint8_t *r = mmap(NULL, R, PROT_NONE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE | MAP_JIT, -1, 0);
    if (r == MAP_FAILED) { puts("mmap failed"); return 2; }
    if (mprotect(r + OFF, R - OFF, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) { puts("mprotect failed"); return 2; }
    uint32_t *p = (uint32_t *)(r + OFF);
    pthread_jit_write_protect_np(0);
    p[0] = 0xd2800540u; p[1] = 0xd65f03c0u;
    pthread_jit_write_protect_np(1);
    sys_icache_invalidate(p, 8);
    printf("v8 call=%d\n", ((int (*)(void))p)());
    if (mprotect(p, 0x4000, PROT_NONE) != 0 || mprotect(p, 0x4000, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) {
        puts("recommit failed");
        return 2;
    }
    printf("v8 recommit call=%d\n", ((int (*)(void))p)());
    if (munmap(r, R) != 0) { puts("munmap failed"); return 2; }
    puts("v8 unmapped");
    return 0;
}

static uint32_t *page;
static dispatch_semaphore_t go_a, go_b;

static void *b_main(void *a) {
    (void)a;
    for (int round = 1; round <= 2; round++) {
        printf("B ran %d\n", ((int (*)(void))page)());
        dispatch_semaphore_signal(go_a);
        if (round < 2) dispatch_semaphore_wait(go_b, DISPATCH_TIME_FOREVER);
    }
    return NULL;
}

static int twothreads(void) {
    if (!(page = jit_page())) return 2;
    go_a = dispatch_semaphore_create(0);
    go_b = dispatch_semaphore_create(0);
    pthread_jit_write_protect_np(0); // A is write-enabled from here until after the join
    emit(page, 1);
    pthread_t th; pthread_create(&th, NULL, b_main, NULL);
    dispatch_semaphore_wait(go_a, DISPATCH_TIME_FOREVER); // B runs round 1
    emit(page, 2);
    printf("A sprr=%#llx wrote 2\n", rd());
    dispatch_semaphore_signal(go_b);
    dispatch_semaphore_wait(go_a, DISPATCH_TIME_FOREVER); // B runs round 2
    pthread_join(th, NULL);
    pthread_jit_write_protect_np(1);
    printf("A protect, ran %d\n", ((int (*)(void))page)());
    return 0;
}

static int fault(void) {
    uint32_t *p = jit_page();
    if (!p) return 2;
    *(volatile uint32_t *)p = 0xd2800540u; // protected: natively a fault; J2 would repair it
    puts("UNREACHED");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    const char *mode = argc > 1 ? argv[1] : "basic";
    if (!strcmp(mode, "basic")) return basic();
    if (!strcmp(mode, "v8")) return v8();
    if (!strcmp(mode, "twothreads")) return twothreads();
    if (!strcmp(mode, "fault")) return fault();
    printf("unknown mode %s\n", mode);
    return 2;
}
```

  **9c. Wiring.** In `crates/retrace-guest/build.rs`:
  - after the `scalarprobe` block:
    ```rust
        // M48: the static SPRR / cache-maintenance probe, with no commpage (§11a item 8). Task 0 Step 3
        // measured this exact text on the base binary.
        let src = format!("{}/asm/sprrprobe.s", env!("CARGO_MANIFEST_DIR"));
        let bin = format!("{out}/sprrprobe");
        println!("cargo:rerun-if-changed={src}");
        let status = Command::new("clang")
            .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
            .status().expect("clang sprrprobe");
        assert!(status.success(), "sprrprobe guest build failed");
    ```
  - after the `madv_dyn` block:
    ```rust
        // jitwp_dyn: the M48 JIT write-protect fixture — modes basic, v8, twothreads and fault. Same
        // recipe as madv_dyn.
        let src = format!("{}/c/jitwp_dyn.c", env!("CARGO_MANIFEST_DIR"));
        let bin = format!("{out}/jitwp_dyn");
        println!("cargo:rerun-if-changed={src}");
        let status = Command::new("clang")
            .args(["-arch","arm64","-o",&bin,&src])
            .status().expect("clang jitwp_dyn");
        assert!(status.success(), "jitwp_dyn guest build failed");
    ```

  In `crates/retrace-guest/src/lib.rs`, beside `MADV_DYN`:
  ```rust
  /// M48: a freestanding probe with no commpage: `mrs S3_6_C15_C1_5` must read 0, EL0 `ic ivau` must
  /// run (SCTLR.UCI), and the `msr` must be refused by value (§11a item 8).
  pub const SPRRPROBE: &str = concat!(env!("OUT_DIR"), "/sprrprobe");
  /// M48: `MAP_JIT` write-protect by mode — `basic`, `v8`, `twothreads`, `fault` (spec §3h).
  pub const JITWP_DYN: &str = concat!(env!("OUT_DIR"), "/jitwp_dyn");
  ```
  and to its `tests`:
  ```rust
      #[test]
      fn sprrprobe_guest_parses_and_carries_both_sprr_encodings() {
          // M48: proves the build.rs wiring and the path constant, and pins retrace-arch's SPRR
          // decode to clang's own words: the probe opens with `mrs x0, S3_6_C15_C1_5` and holds one
          // `msr S3_6_C15_C1_5, x1` and one `ic ivau, x3`. Behaviour is jitwp_e2e's.
          let l = parse_macho(&std::fs::read(SPRRPROBE).unwrap());
          let seg = l.segments.iter().find(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64)
              .expect("the entry lies inside a segment");
          let words: Vec<u32> = seg.data[(l.entry - seg.vaddr) as usize..].chunks_exact(4)
              .map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
          use retrace_arch::{decode_sprr_access, SprrAccess};
          assert_eq!(decode_sprr_access(words[0]), Some(SprrAccess::Read { rt: 0 }), "{:#010x}", words[0]);
          assert_eq!(words.iter().filter(|&&w| decode_sprr_access(w) == Some(SprrAccess::Write { rt: 1 })).count(), 1,
              "exactly one msr S3_6_C15_C1_5, x1");
          assert!(words.contains(&0xd50b_7523), "the probe's ic ivau, x3 (nodeshapes.rs's neighbour word)");
      }

      #[test]
      fn jitwp_guest_parses() {
          // M48: proves the build.rs wiring and the path constant; behaviour is jitwp_e2e's.
          let l = parse_macho(&std::fs::read(JITWP_DYN).unwrap());
          assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
      }
  ```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
diff $L/t0/sprrprobe.s crates/retrace-guest/asm/sprrprobe.s; echo "diff=$?"
cargo test -p retrace-guest --lib --no-fail-fast -- --test-threads=1 > $L/t6-guest.log 2>&1; echo "exit=$?"; grep -a -E 'sprrprobe|jitwp|test result' $L/t6-guest.log
G=$(ls -td target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1); echo "guest out dir: $G"
for m in basic v8 twothreads fault; do $G/jitwp_dyn $m; echo "native $m rc=$?"; done > $L/t6-native.out 2>&1; cat $L/t6-native.out
```

Expected: `diff=0`; exit 0 with both new guest tests passing; natively `basic` prints the sprr.c sequence with `jit call=42`, `v8` prints `v8 call=42`, `v8 recommit call=42` and `v8 unmapped`, `twothreads` prints `B ran 1`, `A sprr=<+0x110> wrote 2`, `B ran 2`, `A protect, ran 2`, and `fault` prints its marker and dies of a signal (rc 138 or 139), never printing `UNREACHED`. `$G` is the newest `retrace-guest` build output, the one `cargo test -p retrace-guest` just rebuilt. Record the native outputs in the report.

- [ ] **Step 10: The anon-exec warning's exemption.** In `crates/retrace-core/src/lib.rs`'s anonymous-`mmap` record arm, replace the warning's condition and extend its comment:

```rust
                // Minor (b): an anonymous PROT_EXEC (JIT) mmap would need exec promotion but
                // guest_mmap installs plain RW+non-exec data pages. JIT is out of M2 scope; warn
                // loudly rather than silently hand back a non-exec page the guest can't run.
                // M48 §3f exempts MAP_JIT: the box keeps such a range and stamps the running thread's
                // view over it, so it is executable whenever that thread is protected. Every other
                // anonymous exec map keeps this warning (§11a item 2).
                if args[2] & 0x4 != 0 && args[3] & retrace_arch::MAP_JIT == 0 {
```

This is the task's only `retrace-core` change. It names no SPRR (Task 10's audit requires zero `sprr` mentions there), and it adds no record arm and no mirror: replay's anonymous-`mmap` mirror never warned.

- [ ] **Step 11: `jitwp_e2e`.** Create `crates/retrace/tests/jitwp_e2e.rs`:

```rust
//! M48 Task 6 gate (spec §3f, §3h, §4): JIT write-protect on a repo-owned fixture, so the mechanism
//! is guarded on a machine without node. Every assertion is on the difference J1 makes, never on an
//! exit code a weaker failure also produces:
//! - `basic`: native's register sequence and 42 from JIT code. A flip-on-fault model (J2) returns 42
//!   too, which is why `fault` exists.
//! - `v8`: V8's shape, `PROT_NONE` then an RWX commit inside it (Review Focus item 4).
//! - `twothreads`: B runs the page while A is write-enabled; a process-wide view would fault.
//! - `fault`: a store to a protected page is the crash native takes, where J2 would exit 0.
//! - a seek into a write-enabled window, a single-step across a toggle (P9), and a
//!   `reverse-continue` to the code write, checked by its effect.
//! - `sprrprobe`: the static probe reads 0, runs `ic ivau`, and is refused at its `msr` by value.
mod util;

use retrace_trace::{Event, Reader};
use std::path::{Path, PathBuf};

/// The fixture run natively.
fn native(mode: &str) -> std::process::Output {
    std::process::Command::new(retrace_guest::JITWP_DYN).arg(mode).output().expect("run jitwp_dyn natively")
}

/// Record `mode`, assert exit 0, and replay twice byte-identically.
fn records_and_replays(mode: &str) -> (util::RunOut, PathBuf) {
    let (rec, trace) = util::record_dynamic_args(retrace_guest::JITWP_DYN, &[mode]);
    assert_eq!(rec.code, 0, "{mode}: record: {}", rec.stderr);
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 0, "{mode}: replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "{mode}: replay {n} stdout");
    }
    (rec, trace)
}

/// The page out of the guest's own `JITWP page=0x…` marker.
fn page(stdout: &str) -> u64 {
    let at = stdout.find("JITWP page=0x").unwrap_or_else(|| panic!("no page marker in {stdout:?}")) + 13;
    let rest = &stdout[at..];
    u64::from_str_radix(&rest[..rest.find('\n').unwrap()], 16).unwrap()
}

/// Every line but the page marker, whose address differs natively.
fn without_marker(stdout: &str) -> Vec<String> {
    stdout.lines().filter(|l| !l.starts_with("JITWP page=")).map(str::to_string).collect()
}

/// `(landmark, thread)` of every write to stdout, in order. stdout is line-buffered, so each is one line.
fn stdout_writes(trace: &Path) -> Vec<(usize, u32)> {
    Reader::open(trace).unwrap().into_iter().enumerate().filter_map(|(i, e)| match e {
        Event::Syscall { num, args, thread, .. } if retrace_arch::is_write_syscall(num) && args[0] == 1 => Some((i, thread)),
        _ => None,
    }).collect()
}

/// `basic`'s landmarks: the write of the `main write-en` line, made inside the write-enabled window,
/// and the write of `jit call=42`, made after the protect toggle and the call into the page.
fn basic_landmarks(stdout: &str, trace: &Path) -> (usize, usize) {
    let lines: Vec<&str> = stdout.lines().collect();
    let writes = stdout_writes(trace);
    assert_eq!(writes.len(), lines.len(), "one write per line");
    let we = lines.iter().position(|l| l.starts_with("main write-en sprr=")).expect("the write-enabled line");
    assert!(lines[we + 1].starts_with("main protect sprr=") && lines[we + 2] == "jit call=42", "{lines:?}");
    (writes[we].0, writes[we + 2].0)
}

#[test]
fn basic_runs_natives_jit_sequence_and_replays() {
    let nat = native("basic");
    assert!(nat.status.success(), "native basic: {nat:?}");
    let nat_out = String::from_utf8(nat.stdout).unwrap();
    let (rec, trace) = records_and_replays("basic");
    let out = String::from_utf8(rec.stdout).unwrap();
    // R1's named deviation: native's register starts at the commpage's +0x118, retrace's at 0. Both
    // mean protected (§2c), so every other line, the register around each toggle included, is native's.
    let protect = nat_out.split("+0x118=").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no commpage line in {nat_out:?}")).to_string();
    let want: Vec<String> = without_marker(&nat_out).into_iter().map(|l| {
        for who in ["main initial sprr=", "child initial sprr="] {
            if let Some(v) = l.strip_prefix(who) {
                assert_eq!(v, protect, "native starts every thread protected (§2c): {l}");
                return format!("{who}0");
            }
        }
        l
    }).collect();
    assert_eq!(without_marker(&out), want, "the guest's lines are native's, R1's two lines aside");
    assert!(out.contains("jit call=42\n"), "{out}");
    assert!(!rec.stderr.contains("anon PROT_EXEC mmap"),
        "a MAP_JIT map is exempt from the anon-exec warning (§11a item 2):\n{}", rec.stderr);
    let maps = Reader::open(&trace).unwrap().iter().filter(|e| matches!(e,
        Event::Syscall { num, args, err: false, .. } if *num == retrace_arch::SYS_MMAP && args[3] & retrace_arch::MAP_JIT != 0)).count();
    assert_eq!(maps, 1, "the recording holds the one MAP_JIT mapping");
}

/// Review Focus item 4 (e2e half): V8 maps `PROT_NONE`, then `mprotect`s a sub-range RWX, which
/// `guest_mprotect` routes through `unprotect`, and `unprotect` stamps `ATTR_DATA`. The first call
/// follows two toggles, whose flips restamp the range anyway. The second follows a decommit and
/// recommit made while protected, with no toggle after it, so without the restamp it faults.
#[test]
fn the_v8_shape_none_mapped_then_mprotected_rwx_runs_its_code() {
    let nat = native("v8");
    assert!(nat.status.success(), "native v8: {nat:?}");
    let (rec, trace) = records_and_replays("v8");
    assert_eq!(rec.stdout, nat.stdout, "native's lines");
    assert_eq!(rec.stdout, b"v8 call=42\nv8 recommit call=42\nv8 unmapped\n");
    let evs = Reader::open(&trace).unwrap();
    let (base, len) = evs.iter().find_map(|e| match e {
        Event::Syscall { num, args, ret, err: false, .. }
            if *num == retrace_arch::SYS_MMAP && args[3] & retrace_arch::MAP_JIT != 0 && args[2] == 0 => Some((*ret, args[1])),
        _ => None,
    }).expect("a PROT_NONE MAP_JIT mapping");
    assert!(evs.iter().any(|e| matches!(e, Event::Syscall { num, args, .. }
            if *num == retrace_arch::SYS_MPROTECT && args[2] == 7 && args[0] > base && args[0] < base + len)),
        "an RWX mprotect strictly inside it, V8's commit");
}

/// Spec §4: B runs the page while A is write-enabled. With a process-wide view, B would execute an
/// `ATTR_DATA` page and fault.
#[test]
fn b_runs_the_page_while_a_is_write_enabled() {
    let nat = native("twothreads");
    assert!(nat.status.success(), "native twothreads: {nat:?}");
    let (rec, trace) = records_and_replays("twothreads");
    let out = String::from_utf8(rec.stdout).unwrap();
    assert_eq!(without_marker(&out), without_marker(&String::from_utf8(nat.stdout).unwrap()),
        "native's lines, in native's order, A's write-enabled register included");
    let writes = stdout_writes(&trace);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), writes.len(), "one write per line");
    for (l, &(_, t)) in lines.iter().zip(&writes) {
        let want = if l.starts_with("B ran") { 1 } else { 0 };
        assert_eq!(t, want, "{l:?} was written by thread {t}: B's lines are thread 1's, A's thread 0's");
    }
    assert!(lines.contains(&"B ran 1") && lines.contains(&"B ran 2"), "{out}");
}

/// Spec §4: a store to a protected `MAP_JIT` page is the crash native takes. J2's flip-on-fault
/// would repair it silently and print `UNREACHED`.
#[test]
fn a_store_to_a_protected_jit_page_is_the_recorded_crash_native_takes() {
    use std::os::unix::process::ExitStatusExt;
    let nat = native("fault");
    assert!(nat.status.signal().is_some(), "natively the store kills the process with a signal: {:?}", nat.status);
    assert!(!String::from_utf8_lossy(&nat.stdout).contains("UNREACHED"));
    let (rec, trace) = util::record_dynamic_args(retrace_guest::JITWP_DYN, &["fault"]);
    assert_eq!(rec.code, 139, "a recorded crash exits 139 (M6). stderr: {}", rec.stderr);
    let out = String::from_utf8_lossy(&rec.stdout).into_owned();
    assert!(!out.contains("UNREACHED"), "the store must not succeed:\n{out}");
    let p = page(&out);
    let (esr, far) = Reader::open(&trace).unwrap().iter().find_map(|e| match e {
        Event::Crash { esr, far, .. } => Some((*esr, *far)),
        _ => None,
    }).expect("the protected store ends the recording in an Event::Crash");
    assert_eq!(far, p, "the fault is the store into the MAP_JIT page");
    assert_eq!(esr >> 26, 0x24, "a data abort from EL0: {esr:#x}");
    assert_eq!(esr & 0x3f, 0x0f, "DFSC 0x0f, a permission fault on the ATTR_CODE page, not a translation fault: {esr:#x}");
    assert!(esr & (1 << 6) != 0, "WnR: the access was a write: {esr:#x}");
    assert_eq!(nat.status.signal(), Some(retrace_arch::signal_of_esr(esr).0 as i32),
        "the signal this crash maps to is the one native died of (a permission fault is SIGBUS, M13)");
    for n in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {n}: {}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {n} stdout");
    }
}

/// Spec §3j's restore parity for the JIT: a checkpoint taken inside the write-enabled window (just
/// after the guest printed its write-enabled register), continued across the protect toggle and the
/// call into the page, must equal a cold seek there. A checkpoint that lost the set or the view trips
/// `assert_jit_stamped`, or leaves the page `ATTR_DATA` so the call faults.
#[test]
fn a_seek_into_a_write_enabled_window_matches_a_cold_seek() {
    let (rec, trace) = records_and_replays("basic");
    let out = String::from_utf8(rec.stdout).unwrap();
    let write_enable = out.split("+0x110=").nth(1).and_then(|r| r.split_whitespace().next()).unwrap();
    assert!(out.contains(&format!("main write-en sprr={write_enable}\n")),
        "precondition: the window is write-enabled by the guest's own read-back:\n{out}");
    let (inside, call) = basic_landmarks(&out, &trace);
    let cp = retrace_core::seek(&trace, inside + 1, 0).unwrap().checkpoint();
    let warm = {
        let mut s = retrace_core::ReplaySession::from_checkpoint(&trace, &cp).unwrap();
        s.advance_to_landmark(call + 1).unwrap_or_else(|d| panic!("warm: diverged at {}: {}", d.landmark, d.detail));
        (s.current_thread(), s.dbg_regs(), s.dbg_fp_regs(), s.dbg_internal_state(), s.snapshot().1)
    };
    let cold = retrace_core::seek(&trace, call + 1, 0).unwrap();
    assert_eq!(warm.0, cold.current_thread(), "thread: checkpointed vs cold");
    assert_eq!(warm.1, cold.dbg_regs(), "registers: checkpointed vs cold");
    assert_eq!(warm.2, cold.dbg_fp_regs(), "FP/SIMD: checkpointed vs cold");
    assert_eq!(warm.3, cold.dbg_internal_state(), "internal state: checkpointed vs cold");
    assert!(cold.diff_memory(&warm.4).is_none(), "memory: checkpointed vs cold");
}

/// P9: single-stepping the window that holds the protect toggle runs the view flip, and so
/// `flush_guest_tlb`, inside `step()`. Before M48 that panicked (`tlbi stub faulted at EL1:
/// EC=SoftStep`), which is what `reverse-continue` into JIT code would hit. The window runs from
/// the `main write-en` line's write to the `main protect` line's: the code stores, the protect
/// toggle and its read-back.
#[test]
fn stepping_across_a_toggle_does_not_step_the_tlbi_stub() {
    let (rec, trace) = records_and_replays("basic");
    let out = String::from_utf8(rec.stdout).unwrap();
    let p = page(&out);
    let (inside, _) = basic_landmarks(&out, &trace);
    // `window_len_here` steps the whole window and spends its session (parked at the trap).
    let n = {
        let mut s = retrace_core::seek(&trace, inside + 1, 0).unwrap();
        assert_eq!(s.read_mem(p, 8).unwrap(), [0u8; 8], "precondition: the code is not written yet");
        s.window_len_here().unwrap_or_else(|e| panic!("stepping the protect toggle's window: {e}"))
    };
    assert!(n > 0, "the window holds the stores, the msr and the read-back");
    // A seek to the window's last instruction single-steps every one before it (`step_insns`), the
    // flip included, and leaves a session that can still run.
    let mut s = retrace_core::seek(&trace, inside + 1, n - 1)
        .unwrap_or_else(|e| panic!("a seek stepped across the protect toggle: {e}"));
    assert_eq!(s.read_mem(p, 8).unwrap(), [0x40, 0x05, 0x80, 0xd2, 0xc0, 0x03, 0x5f, 0xd6],
        "the stepped stores landed: mov x0, #42; ret");
    // The stepped flip left the box sound: the call into the page, protected, replays to exit 0.
    loop {
        match s.advance() {
            Ok(retrace_core::Advance::Exited(r)) => {
                assert!(matches!(r.outcome, retrace_core::Outcome::Exit { code: 0 }), "exit after the step");
                break;
            }
            Ok(_) => {}
            Err(d) => panic!("after stepping across the toggle, replay diverged at {}: {}", d.landmark, d.detail),
        }
    }
}

/// Spec §3h: `reverse-continue` to the code write, checked by its effect. The last write to the
/// page's first eight bytes is the `ret` word: before it retires the first word is there and the
/// second is not; one `stepi` later both are. The search single-steps across both toggles (P9).
#[test]
fn reverse_continue_lands_on_the_jit_code_write_by_its_effect() {
    let (rec, trace) = records_and_replays("basic");
    let p = page(&String::from_utf8_lossy(&rec.stdout));
    let (code, out, err) = util::debug_bounded(trace.to_str().unwrap(),
        &format!("continue; watch 0x{p:x} 8; reverse-continue; x 0x{p:x} 8; stepi; x 0x{p:x} 8"), 300);
    assert_eq!(code, Some(0), "debug exited {code:?} (None: killed at the bound). stderr: {err}\nstdout: {out}");
    assert!(out.contains(&format!("hit watch 0x{p:x} (write at ")), "reverse-continue must find the code write:\n{out}");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{p:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps expected:\n{out}");
    assert!(xs[0].ends_with("40 05 80 d2 00 00 00 00"), "before the store only `mov x0, #42` is there:\n{out}");
    assert!(xs[1].ends_with("40 05 80 d2 c0 03 5f d6"), "one stepi later `ret` landed:\n{out}");
}

/// §11a item 8: the static probe reaches its `msr` only if the `mrs` read 0 (R1; otherwise it
/// exits 2) and the `ic ivau` ran at EL0 (SCTLR.UCI; otherwise it stops at EC 0x18). It has no
/// commpage, so the write is refused by value (Ruling T6-f).
#[test]
fn sprrprobe_reads_zero_runs_ic_ivau_and_is_refused_at_its_write_by_value() {
    let (rec, trace) = util::record(retrace_guest::SPRRPROBE);
    let _ = std::fs::remove_file(&trace);
    assert!(!rec.stderr.contains("non-syscall exit"), "the ic ivau must run at EL0, not trap:\n{}", rec.stderr);
    assert!(rec.stderr.contains("M48: SPRR write 0x1 at pc ") && rec.stderr.contains("no SPRR commpage"),
        "the msr must be refused by value (code {}):\n{}", rec.code, rec.stderr);
    assert!(rec.code != 0 && rec.code != 2, "exit 2 would mean the mrs read nonzero; 0 that nothing was refused: {}", rec.code);
}
```

Run it:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test jitwp_e2e --no-fail-fast -- --test-threads=1 > $L/t6-jitwp.log 2>&1; echo "exit=$?"; grep -a -E '^test |test result|panicked' $L/t6-jitwp.log
```

Expected: exit 0, eight tests passing. If `a_store_to_a_protected_jit_page_is_the_recorded_crash_native_takes` finds a DFSC other than `0x0f`, the page was not stamped `ATTR_CODE` at the store (a translation fault means it was unmapped): that is a defect in the mmap hook, not a test to loosen.

- [ ] **Step 12: Green, regression, clippy, commit.** Three changes reach every guest: SCTLR UCI, the `MAP_JIT` checks in every `mmap`/`munmap`/`mprotect`/FIXED path, and the view sync in every thread switch. The step-safe flush reaches every debugger session. So the regression set is the four crates' own targets, the `retrace` bin target, and every e2e gate that maps exec memory, switches threads, single-steps or reverse-continues:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t6-arch-all.log 2>&1; echo "arch exit=$?"
cargo test -p retrace-guest --no-fail-fast -- --test-threads=1 > $L/t6-guest-all.log 2>&1; echo "guest exit=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > $L/t6-box.log 2>&1; echo "box exit=$?"
cargo test -p retrace-core --no-fail-fast -- --test-threads=1 > $L/t6-core.log 2>&1; echo "core exit=$?"
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > $L/t6-bins.log 2>&1; echo "bins exit=$?"
for t in jitwp_e2e hello_dyn_e2e hello_rust_e2e jq_e2e jq_file_e2e cpython_e2e cpython_crash_e2e execmap_e2e tlbiexec_e2e mmapfile_e2e remap_e2e vmremap_e2e protnone_rust_e2e segv_rust_e2e thread_rust_e2e thread_watch_e2e sigthread_e2e dispatch_e2e gcdtimer_e2e kqinit_e2e llsc_e2e hitorder_e2e watchsweep_e2e reverse_debug_e2e checkpoint_seek seek trim_e2e simd_e2e kq_e2e condvar_e2e git_e2e gitprims_e2e apple_walls_e2e gdbserver_e2e lldb_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t6-$t.log 2>&1; echo "$t exit=$?"; done
cargo clippy -p retrace-arch -p retrace-box -p retrace-guest -p retrace-core -p retrace --all-targets -- -D warnings > $L/t6-clippy.log 2>&1; echo "clippy exit=$?"
grep -a -h 'test result' $L/t6-*.log | sort | uniq -c
git add -A && git commit -m "M48 t6: JIT write-protect — S3_6_C15_C1_5 per thread, the MAP_JIT view (ranges minus their no-access extents, restamped after unprotect), a step-safe flush, SCTLR UCI; jitwp_dyn, sprrprobe and jitwp_e2e"
```

Every line must read `exit=0`. The loop runs for tens of minutes (`cpython_crash_e2e`, `git_e2e` and `lldb_e2e` dominate). If it would pass the 10-minute tool ceiling, run it detached (`nohup bash -c '…' > $L/t6-loop.log 2>&1 &`) and poll the log. If a target name above does not exist in `crates/retrace/tests/`, drop it and say so in the report; do not invent one. A test that skips announces `SKIPPED` through `util::announce`; record which ones did.

- [ ] **Step 13: Controls (on the committed tree).** Run each control on its own, record its symptom, then restore with `git checkout <t6 commit> -- <file>` and confirm `git status --short` is empty before the next.
  1. **The restamp after `unprotect` (Review Focus item 4).** Delete `if in_jit { self.restamp_jit(self.jit.view()); }` from `guest_mprotect`. Run `cargo test -p retrace-box --test jit an_unprotect -- --test-threads=1` and `cargo test -p retrace --test jitwp_e2e the_v8_shape -- --test-threads=1`. Both must go red: the first at `protected view: the committed range is ATTR_CODE`, the second at `v8: record:` with exit 139, at the call that follows the recommit (`v8 call=42` is printed, `v8 recommit call=42` is not).
  2. **The step-safe flush (P9).** Restore `flush_guest_tlb`'s pre-M48 body (no `MDSCR` lines). Run `cargo test -p retrace-box --test jit a_flush_with -- --test-threads=1` and `cargo test -p retrace --test jitwp_e2e stepping_across -- --test-threads=1`. Both must go red with `tlbi stub faulted at EL1: EC=SoftStep`.
  3. **The view on a switch.** Delete `self.sync_jit_view();` from `switch_to_thread`. Run `cargo test -p retrace-box --test jit the_view_follows -- --test-threads=1` and `cargo test -p retrace --test jitwp_e2e b_runs_the_page -- --test-threads=1`. The first must go red at `a switch to a protected thread flips back`. The second must go red at `twothreads: record:` with exit 139: B runs under A's write-enabled view, so its call into the page is an instruction abort on an `ATTR_DATA` page.
  4. **The carried set (Ruling T6-g).** In `from_checkpoint`, replace `jit: state.jit.clone(),` with `jit: jit::JitSet::default(),`. Run `cargo test -p retrace-box --test checkpointparity write_enabled_jit -- --test-threads=1` and `cargo test -p retrace --test jitwp_e2e a_seek_into -- --test-threads=1`. Both must go red at `M48: a restored MAP_JIT view Rx is not thread 0's mode Rw`. Then, with that line still replaced, also delete the `b.assert_jit_stamped();` call and re-run the e2e. It must still go red, now as a divergence or a crash at the call into the page, because the view no longer flips on the protect toggle. That proves the seek test does not lean on the assert alone.
  5. **UCI.** Remove `| 0x0400_0000` from `SCTLR_MMU_ON_BASE`. Run `cargo test -p retrace --test jitwp_e2e sprrprobe -- --test-threads=1` and `cargo test -p retrace-box --lib sctlr -- --test-threads=1`. The first must go red with `non-syscall exit` (EC 0x18 at the `ic ivau`), and the second at `UCI (bit 26) must be SET`.
  6. **The SPRR refusal (R2).** Delete the `assert!(value == we || value == pr, …)` in `sprr_write`. Run `cargo test -p retrace-box --test jit an_inadmissible -- --test-threads=1`. It must go red: `test did not panic as expected`.

  A control that stays green is a finding: report it, never paper over it.

---

### Task 7: `node_e2e` — rung 9 un-parked, the JIT witness, the 2-second timer (`retrace-box`, `util`, e2e)

M47 parked `node_prints_one_and_replays` at `kevent` (363). Tasks 1–6 give node everything walls.md §1 measured it needs, so this task deletes the `#[ignore]`, adds the JIT assertion spec §1 part 1 asks for, and adds `node_timer_replays` (§1 part 2, at the 2000 ms of §11b item 12 and P8).

The JIT assertion needs a witness for the SPRR write. The register lives below the trace (R3), so no recording shows a write, and a `--jitless`-equivalent run prints `1` too (spec §4).
- **Ruling T7-a.** The witness is a gated stderr line in `Box_::sprr_write`, `RETRACE_SPRR=1`, the `RETRACE_REGCLAMP` shape: one line per admitted write, nothing when unset.
- Rejected: a trace field (H3), and an in-process replay reading `Thread.sprr` (a second 46 s debug replay and a new `retrace-core` accessor).

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (one gated line in `sprr_write`)
- Modify: `crates/retrace/tests/util/mod.rs` (`record_dynamic_args_env`, `assert_rung_records_and_replays_env` over a shared `assert_rung`, `map_jit_ranges`)
- Rewrite: `crates/retrace/tests/node_e2e.rs` (the `#[ignore]` deleted, the JIT assertion, `node_timer_replays`)

**Interfaces:**
- Consumes:
  - Task 1's `set_simd` (node's threads keep live SIMD state across switches);
  - Task 2's `retrace_arch::SYS_KEVENT` and rows 105/363/303–305, and 3419 forwarded;
  - Task 3's `unmap_range` (V8's trims);
  - Task 4's `Box_::guest_kevent`, `BlockReason::Kevent` with its deadline, the one deadline queue in `schedule_after_block` and `deliver_wake`;
  - Task 5's `Box_::guest_psynch` and `BlockReason::Cv`;
  - Task 6's `Box_::sprr_write(&mut self, value: u64)`, with `value` the admitted register value. It admits the value, calls `self.threads.set_sprr_of(tid, value)`, then ends with `self.sync_jit_view()`, and Step 4's line goes between those two calls.
  - Task 6's `retrace_arch::MAP_JIT`, `JitSet` and the view flip, SCTLR UCI, and the step-safe `flush_guest_tlb`.
- Produces:
  - `node_e2e`'s two tests, un-ignored;
  - the `RETRACE_SPRR` witness line, which Task 9's census also reads;
  - `util::record_dynamic_args_env`, `util::assert_rung_records_and_replays_env` and `util::map_jit_ranges`, which Task 8 uses.

- [ ] **Step 1: Controller addendum.** Write `$L/task-7-addendum.md` pinning, from measurements §M2 and §M5:
  - the `e` and `t2000` walks' record rc and stdout (expected 0 and `1`, 0 and `2`);
  - the `t2000` walk's main-thread timed `kevent` timeout (expected `{1, 986000000}` on kq 10) and its idle jump (the probe's was `0x2d50b80` ticks);
  - the `e` walk's SPRR write count (expected 268) and `MAP_JIT` mapping count (expected 1);
  - every wall §M2 found beyond walls.md §1, with the Ruling that assigned it and the task that cleared it.

  Do not start until it exists.

- [ ] **Step 2: The `util` helpers.** In `crates/retrace/tests/util/mod.rs`, add beside `record_dynamic_args`:

```rust
// M48: `record_dynamic_args` with extra environment set on the RECORDER (see `run_env`), for a
// channel the recorder prints only when asked: node_e2e's `RETRACE_SPRR`.
pub fn record_dynamic_args_env(guest: &str, args: &[&str], env: &[(&str, &str)]) -> (RunOut, std::path::PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(3_000_000);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let trace = std::env::temp_dir().join(format!("retrace-argvenv-{}-{n}.bin", std::process::id()));
    let mut argv = vec!["record-dyn", guest, "-o", trace.to_str().unwrap()];
    if !args.is_empty() { argv.push("--"); argv.extend_from_slice(args); }
    (run_env(&argv, env), trace)
}
```

Replace the body of `assert_rung_records_and_replays`, keeping its doc comment, and add the two functions below it. The assertions move unchanged into `assert_rung`, so every existing caller keeps the same checks and messages (Ruling T7-b):

```rust
pub fn assert_rung_records_and_replays(guest: &str, argv: &[&str], expect_stdout: &[u8]) -> RungOut {
    let (rec, trace) = record_dynamic_args(guest, argv);
    assert_rung(rec, trace, expect_stdout).0
}

/// M48: `assert_rung_records_and_replays` with extra environment on the RECORDER (see `run_env`),
/// returning the recorder's stderr beside the rung output, for a channel the recorder prints only
/// when asked (node_e2e's `RETRACE_SPRR`). The assertions are the same function's.
pub fn assert_rung_records_and_replays_env(guest: &str, argv: &[&str], expect_stdout: &[u8],
                                           env: &[(&str, &str)]) -> (RungOut, String) {
    let (rec, trace) = record_dynamic_args_env(guest, argv, env);
    assert_rung(rec, trace, expect_stdout)
}

/// The rung assertions both entry points above share, moved unchanged from
/// `assert_rung_records_and_replays` at M48. Returns the recorder's stderr too.
fn assert_rung(rec: RunOut, trace: std::path::PathBuf, expect_stdout: &[u8]) -> (RungOut, String) {
    assert_eq!(rec.code, 0,
        "rung guest must reach a clean exit(0); 139 means it CRASHED (M6 records that as a \
         successful recording, which is exactly what this assertion exists to reject). stderr:\n{}",
        rec.stderr);
    assert_eq!(rec.stdout, expect_stdout,
        "rung guest stdout mismatch — did it reach main? got {:?}, want {:?}",
        String::from_utf8_lossy(&rec.stdout), String::from_utf8_lossy(expect_stdout));
    for i in 0..2 {
        let rep = replay(&trace);
        assert_eq!(rep.code, 0, "replay {i} must exit 0. stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged from the recording");
    }
    (RungOut { trace, stdout: rec.stdout }, rec.stderr)
}

/// M48: every `MAP_JIT` mapping a recording made, as `[start, end)`: the `mmap` (197) landmarks
/// whose flags carry `MAP_JIT` (Task 6's `retrace_arch::MAP_JIT`) and that succeeded, at the
/// address they returned. It takes decoded events, so a test over a node trace (526 MB to 1.2 GB,
/// walls.md §2) decodes it once.
pub fn map_jit_ranges(events: &[retrace_trace::Event]) -> Vec<(u64, u64)> {
    events.iter().filter_map(|e| match e {
        retrace_trace::Event::Syscall { num, args, ret, err: false, .. }
            if *num == retrace_arch::SYS_MMAP && args[3] & retrace_arch::MAP_JIT != 0 => Some((*ret, *ret + args[1])),
        _ => None,
    }).collect()
}
```

`retrace-arch` and `retrace-trace` are already dev-dependencies of `retrace`. The module's `#![allow(dead_code)]` covers the binaries that use none of these helpers.

- [ ] **Step 3: Rewrite `node_e2e.rs`, then run it red.** Replace `crates/retrace/tests/node_e2e.rs` with:

```rust
//! M48 rung 9 (spec §1 parts 1–2): Homebrew's node, JIT on, records to exit 0 and replays
//! byte-identically twice: `console.log(1)`, and a 2-second `setTimeout` whose deadline the idle
//! jump reaches on the synthetic clock.
//!
//! M47 took node past AMFI's dyld policy and parked this gate at `kevent` (363) on a guest
//! `kqueue()`, libuv's `EVFILT_USER` probe. M48 models guest kqueues, psynch condition variables,
//! deadline-bounded waits, partial `munmap` and `MAP_JIT` write-protect, and fixes the SIMD
//! restore that made a threaded replay depend on the host (walls.md §1, §4), so node runs to its
//! end. The Cellar path, not the `/opt/homebrew/bin` symlink, is what the probe and the walks
//! measured.
//!
//! Each test asserts the difference M48 makes (CLAUDE.md honest-gate rule 1):
//! - `node_prints_one_and_replays`: the recording holds a `MAP_JIT` mapping and the guest wrote
//!   `S3_6_C15_C1_5`, because a run that never reached V8's code space (`--jitless`) prints 1 too
//!   (spec §4).
//! - `node_timer_replays`: the clock reaches the timer's deadline by the idle jump, exactly, and
//!   main runs the callback only after it, because a `kevent` that returned at once prints 2 too.
//!
//! NOT a repo artifact: without Homebrew's node each test announces its skip. The mechanisms are
//! guarded without node by `kq_e2e`, `condvar_e2e`, `jitwp_e2e`, `simd_e2e` and `trim_e2e`. A node
//! trace is about 526 MB (walls.md §2), so each test deletes its trace after its last assertion;
//! a failing run keeps it for diagnosis.
mod util;
use retrace_trace::Event;

const NODE: &str = "/opt/homebrew/bin/node";
/// 2000 ms, not spec §1 part 2's 10 (§11b item 12, P8): `SYNTH_TSC_STRIDE` carries the clock past a
/// 10 ms deadline before the loop's first poll, so a 10 ms timer never blocks and nothing jumps.
const TIMER_JS: &str = "setTimeout(() => console.log(2), 2000)";
/// One second at the guest's 24 MHz timebase (plan F6).
const ONE_SECOND: u64 = 24_000_000;
/// What one timebase read moves the synthetic clock: `SYNTH_TSC_STRIDE` in `retrace-box`.
const STRIDE: u64 = 0x2400;

/// The Cellar binary behind Homebrew's symlink, or `None` after announcing the skip past libtest's
/// capture (CLAUDE.md: a skipped test must announce itself).
fn node(test: &str) -> Option<String> {
    if !std::path::Path::new(NODE).exists() {
        util::announce(&format!("SKIPPED {test}: {NODE} not installed (`brew install node`). \
            This gate did NOT run — it is not evidence of anything."));
        return None;
    }
    Some(std::fs::canonicalize(NODE).unwrap().to_str().unwrap().to_owned())
}

#[test]
fn node_prints_one_and_replays() {
    let Some(exe) = node("node_prints_one_and_replays") else { return };
    // RETRACE_SPRR on the RECORDER: the register lives below the trace (R3), so `sprr_write`'s line
    // is the only witness of a write, and the one recording carries it (Ruling T7-b).
    let (out, rec_stderr) = util::assert_rung_records_and_replays_env(
        &exe, &["-e", "console.log(1)"], b"1\n", &[("RETRACE_SPRR", "1")]);
    assert_eq!(out.stdout, b"1\n");
    // JIT ran (M48 §1 part 1): V8 reserved its code range MAP_JIT …
    let jit = util::map_jit_ranges(&retrace_trace::Reader::open(&out.trace).unwrap());
    assert!(!jit.is_empty(),
        "the recording holds no MAP_JIT mapping: V8 never reserved its code range, which a \
         --jitless run also prints 1 without (spec §4)");
    // … and toggled its write-protect at least once.
    let writes = rec_stderr.lines().filter(|l| l.starts_with("[M48 SPRR] thread ")).count();
    assert!(writes >= 1,
        "the guest never wrote S3_6_C15_C1_5: V8 never toggled its JIT write-protect (MAP_JIT \
         ranges {jit:x?})");
    let _ = std::fs::remove_file(&out.trace);
}

fn synthetic_tsc(state: &str) -> u64 {
    let v = state.split("synthetic_tsc=0x").nth(1).and_then(|r| r.split_whitespace().next())
        .unwrap_or_else(|| panic!("no synthetic_tsc in {state}"));
    u64::from_str_radix(v, 16).unwrap()
}

/// A relative `struct timespec` (`tv_sec`, `tv_nsec`, 8 bytes each) as 24 MHz ticks, truncating.
fn ticks(ts: &[u8]) -> u64 {
    let sec = u64::from_le_bytes(ts[0..8].try_into().unwrap());
    let nsec = u64::from_le_bytes(ts[8..16].try_into().unwrap());
    sec * ONE_SECOND + nsec * 3 / 125
}

#[test]
fn node_timer_replays() {
    let Some(exe) = node("node_timer_replays") else { return };
    let rung = util::assert_rung_records_and_replays(&exe, &["-e", TIMER_JS], b"2\n");

    // Replay in process, reading the clock at every landmark (gcdtimer_e2e's `the_idle_jump`).
    // K is main's last timed kevent of a second or more before the jump: the wait the timer's
    // deadline ends. J is the landmark across which the clock moves by more than a second, which
    // only the idle jump can do: a window would need 2,604 timebase reads.
    let d = retrace_core::DecodedTrace::load(&rung.trace).unwrap();
    let evs = d.events();
    let mut s = retrace_core::ReplaySession::open_decoded(&d).unwrap();
    let mut k: Option<(usize, u64, u64)> = None; // (landmark, clock before its window, timeout ticks)
    let (j, after_j) = loop {
        let (n, before) = (s.landmark(), synthetic_tsc(&s.dbg_internal_state()));
        match s.advance() {
            Ok(retrace_core::Advance::Exited(_)) => panic!(
                "node exited and no landmark moved the clock by a second: the timer's deadline was \
                 never reached by the idle jump"),
            Ok(_) => {}
            Err(e) => panic!("diverged at landmark {}: {}", e.landmark, e.detail),
        }
        if let Event::Syscall { num, args, thread: 0, .. } = &evs[n] {
            if *num == retrace_arch::SYS_KEVENT && args[4] > 0 && args[5] != 0 {
                let t = ticks(&s.read_mem(args[5], 16).expect("main's kevent timeout is readable"));
                if t >= ONE_SECOND { k = Some((n, before, t)); }
            }
        }
        let after = synthetic_tsc(&s.dbg_internal_state());
        if after - before > ONE_SECOND { break (n, after); }
    };
    let (kn, before_k, t) = k.unwrap_or_else(|| panic!(
        "the clock jumped at landmark {j} with no timed kevent of a second or more on main before it"));
    // The jump lands on K's deadline (M48 §3d: the one idle jump goes to the earliest deadline):
    // the clock at K's call plus K's timeout. The clock at the call is the clock before K's window
    // plus that window's own timebase reads, a whole number of STRIDEs.
    let excess = after_j.checked_sub(before_k + t).unwrap_or_else(|| panic!(
        "the jump at landmark {j} stopped short of main's kevent deadline (landmark {kn}: clock \
         {before_k:#x} before its window, timeout {t} ticks; clock after the jump {after_j:#x})"));
    assert!(excess.is_multiple_of(STRIDE) && excess < ONE_SECOND,
        "the jump at landmark {j} must land exactly on main's kevent deadline (landmark {kn}, \
         timeout {t} ticks): {excess:#x} ticks past it is not K's own window's timebase reads");
    assert_eq!(s.current_thread(), 0, "the jump at landmark {j} must wake main, whose deadline it reached");
    let write = evs.iter().position(|e| matches!(e, Event::Syscall { num, args, .. }
        if (*num == retrace_arch::SYS_WRITE || *num == retrace_arch::SYS_WRITE_NOCANCEL) && args[0] == 1))
        .expect("the callback's write of `2` to stdout");
    assert!(write > j,
        "the callback wrote stdout at landmark {write}, before the clock reached its deadline at {j}");
    let _ = std::fs::remove_file(&rung.trace);
}
```

Compile first, then run the target detached. Nothing else may build while it runs:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test node_e2e --no-run > $L/t7-red-build.log 2>&1; echo "build exit=$?"
nohup bash -c 'cargo test -p retrace --test node_e2e --no-fail-fast -- --test-threads=1; echo "exit=$?"' > $L/t7-red.log 2>&1 &
echo "pid=$!"
```

Poll with `grep -a -E '^exit=|^test .* (ok|FAILED)$|SKIPP' $L/t7-red.log` until an `exit=` line appears. Expected:
- `node_prints_one_and_replays` FAILS at `the guest never wrote S3_6_C15_C1_5`, the witness line not existing yet, with every earlier assertion passing. It is the first gate to record node end to end.
- `node_timer_replays` passes.
- A failure at any other assertion is a finding to diagnose before Step 4. A wall in the rung assertion is H5 or a Ruling, exactly as t0 Step 6 classes one.

- [ ] **Step 4: The witness line, then green.** In `Box_::sprr_write`, between `self.threads.set_sprr_of(tid, value);` and its last line, `self.sync_jit_view();`, add:

```rust
        // M48 Task 7: the witness node_e2e reads (spec §1 part 1). The register is below the trace
        // (R3), so no recording shows a write, and a run that never reached V8's code space prints
        // 1 as well. `RETRACE_REGCLAMP`'s shape: one line per admitted write, nothing when unset.
        if std::env::var_os("RETRACE_SPRR").is_some() {
            eprintln!("[M48 SPRR] thread {} wrote {:#x}", self.threads.current(), value);
        }
```

`value` is `sprr_write`'s parameter, already admitted by the time this line runs. The line's text is what the test and Task 9's census match.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test node_e2e --no-run > $L/t7-build.log 2>&1; echo "build exit=$?"
nohup bash -c 'cargo test -p retrace --test node_e2e --no-fail-fast -- --test-threads=1; echo "exit=$?"' > $L/t7-node_e2e.log 2>&1 &
echo "pid=$!"
```

Poll as in Step 3. Expect both tests `ok`, `exit=0` and no `SKIPPED` line. Record the target's `finished in` time from the `test result:` line; Task 8 Step 6 reports it beside its own.

- [ ] **Step 5: Regression, clippy, commit.** The rung helper's body moved, so run every target that calls it, plus the SPRR arm's own gates:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
for t in rung argv_e2e atfdcwd_e2e closewrite_e2e cpython_e2e dup2_e2e dupfd_e2e dupkind_e2e fdtable_e2e hello_rust_e2e jq_e2e jq_file_e2e pipe_e2e stdio_e2e sysbin_e2e vmalign_e2e vmremap_e2e jitwp_e2e simd_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t7-$t.log 2>&1; echo "$t exit=$?"; done
cargo test -p retrace-box --test jit --no-fail-fast -- --test-threads=1 > $L/t7-box-jit.log 2>&1; echo "box jit exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t7-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git commit -m "M48 t7: node_e2e un-parked — rung 9 prints 1 with a MAP_JIT mapping and an SPRR write (RETRACE_SPRR), and a 2-second setTimeout reached by the idle jump"
```

- [ ] **Step 6: Controls (on the committed tree).** Each runs one test detached (`nohup bash -c 'cargo test -p retrace --test node_e2e <name> --no-fail-fast -- --test-threads=1; echo "exit=$?"' > $L/t7-control-<n>.log 2>&1 &`) and is polled as in Step 3.
  1. **A run that never reaches V8's code space.** In `node_prints_one_and_replays`, change the argv to `&["--jitless", "-e", "console.log(1)"]`. Run it and confirm it fails at `the recording holds no MAP_JIT mapping`, with the rung assertion passing: a jitless node prints 1 and replays.
     - If it fails at the rung assertion instead, `--jitless` meets a wall of its own. Then record `--jitless` once by hand with `RETRACE_TRACE=1` and `RETRACE_SPRR=1` exported, count its `MAP_JIT` mmaps and `[M48 SPRR]` lines, and report both counts as the control's result.
  2. **A timer the clock's stride passes.** Change `TIMER_JS`'s `2000` to `10`. Run `node_timer_replays` and confirm it fails at `no landmark moved the clock by a second`, with its rung assertion passing (P8).
  3. **No witness.** Delete Step 4's `if` block. Run `node_prints_one_and_replays` and confirm it fails at `the guest never wrote S3_6_C15_C1_5`.

  Restore after each with `git checkout <t7 commit> -- crates/retrace/tests/node_e2e.rs crates/retrace-box/src/lib.rs`, confirm `git status --short` is empty, and record each symptom.

---

### Task 8: The crash demo, rung 10 (`retrace-guest/node/`, `build.rs`, `node_crash_e2e`)

Spec §3g (D1) and §1 part 3, following `cpython_crash_e2e`. The demo is the probe's (P7), measured end to end.
- **Ruling T8-a.** `crash.js` reads `crash.json` from `__dirname`, which is spec §3g's command line (`crash.js <addon path>`) and `crash.py`'s convention. The probe passed it as `argv[3]`. Every other line is the probe's measured text.
- **Ruling T8-b.** The debug session runs through `util::debug_bounded` (M42), never unbounded, with a bound Step 6 checks against measurement.
- **Ruling T8-c.** Three assertions beyond spec §1 part 3's five, each a measured fact from P7:
  - the marker's `opt=` equals native's, so TurboFan compiled `store` under retrace exactly as natively;
  - node's own `SIGSEGV` handler is delivered the fault before the terminal crash;
  - the cell holds the warm-up value `2` before the store.

**Files:**
- Create: `crates/retrace-guest/node/crash_addon.c`, `crates/retrace-guest/node/crash.js`, `crates/retrace-guest/node/crash.json`
- Modify: `crates/retrace-guest/build.rs` (the addon, built only where `node_api.h` exists)
- Modify: `crates/retrace-guest/src/lib.rs` (`CRASH_JS`, `CRASH_JS_JSON`, `NODE_CRASH_ADDON`; the `node_crash_demo_is_wired` unit test and a local `announce`)
- Create: `crates/retrace/tests/node_crash_e2e.rs`

**Interfaces:**
- Consumes:
  - t0 §M7 (the native marker, the store's location, the session's cost);
  - Task 6's step-safe `flush_guest_tlb`, because the `reverse-continue` single-steps across JIT toggles (P9);
  - Task 6's view flip, because the store executes in a `MAP_JIT` range under the protected view;
  - Tasks 1–5, through node's run;
  - Task 7's `util::map_jit_ranges`;
  - the existing `util::record_dynamic_args`, `util::replay`, `util::debug_bounded` and `util::announce`.
- Produces:
  - the demo files and the three constants (Task 9's walk records the same script);
  - `node_crash_e2e`;
  - the measured debug-build durations of both node targets, the crash trace's size and the debug session's peak RSS, for Task 11's docs and the header's timing line.

- [ ] **Step 1: Controller addendum.** Write `$L/task-8-addendum.md` pinning, from measurements §M7:
  - the native marker line, including `opt=` (expected `opt=101001`);
  - the addon build's result with `cc -bundle -undefined dynamic_lookup`;
  - that the hit's pc landed inside the `MAP_JIT` range, and the cell's two values (expected `2`, then `0x4000dead0000`);
  - the release session's time and peak RSS (P10: 13.6 s, 4.2 GB).

  If t0 ruled a lever change (§M7: `--no-concurrent-recompilation` first), the addendum carries the changed `crash.js` text and the test's argv, and they replace Step 2's and Step 4's. Do not start until it exists.

- [ ] **Step 2: The demo.** Create `crates/retrace-guest/node/crash_addon.c`:

```c
// M48 rung 10 (spec §3g, D1): the N-API addon of node's crash demo. addressOf(ab) returns an
// ArrayBuffer's backing-store address as a BigInt; deref(ab) reads the store's first 8 bytes as a
// pointer and loads through it, which faults at the pointer crash.js stored. It uses no external
// buffers, which the V8 sandbox may forbid. build.rs builds it against Homebrew's node_api.h with
// `-bundle -undefined dynamic_lookup`: the N-API symbols resolve from node when node loads it.
#include <node_api.h>
#include <stdint.h>

static napi_value address_of(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value argv[1];
    napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
    void *data = NULL;
    size_t len = 0;
    napi_get_arraybuffer_info(env, argv[0], &data, &len);
    napi_value r;
    napi_create_bigint_uint64(env, (uint64_t)(uintptr_t)data, &r);
    return r;
}

static napi_value deref(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value argv[1];
    napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
    void *data = NULL;
    size_t len = 0;
    napi_get_arraybuffer_info(env, argv[0], &data, &len);
    volatile uint64_t *p = *(volatile uint64_t *volatile *)data;
    uint64_t v = *p; // the fault: p is the computed target, which is never mapped
    napi_value r;
    napi_create_bigint_uint64(env, v, &r);
    return r;
}

NAPI_MODULE_INIT() {
    napi_property_descriptor d[] = {
        { "addressOf", NULL, address_of, NULL, NULL, NULL, napi_default, NULL },
        { "deref", NULL, deref, NULL, NULL, NULL, napi_default, NULL },
    };
    napi_define_properties(env, exports, 2, d);
    return exports;
}
```

Create `crates/retrace-guest/node/crash.js`:

```js
// M48 rung 10 (spec §3g, D1): node's crash demo, rung 8's shape in JavaScript. The target is
// COMPUTED from crash.json, never a literal here. TurboFan-compiled code stores it into an
// ArrayBuffer's backing store, and the addon loads through it, which faults. The marker line
// reveals the cell (the M6 marker convention), so a test DISCOVERS it from the recording.
//
// Run as `node --allow-natives-syntax crash.js <addon>`. The natives force `store` through
// TurboFan synchronously on main (R6). Left to its heuristics, V8 compiles on a worker thread,
// which the cooperative scheduler runs only when main blocks, and main never blocks here.
//
// 0x4000_DEAD_0000 has bit 46 set (an L1 slot that is never mapped, below 2^47), the FAR crashy.c
// and crash.py use, so the load is a level-1 translation fault at exactly the target.
const addon = require(process.argv[2]);
const fs = require('fs');
const path = require('path');
const rows = JSON.parse(fs.readFileSync(path.join(__dirname, 'crash.json'), 'utf8')).rows;
const target = BigInt(rows[0].value) + BigInt(rows[1].value);
const ab = new ArrayBuffer(8);
const cell = addon.addressOf(ab);
function store(view, v) { view[0] = v; }
const view = new BigUint64Array(ab);
%PrepareFunctionForOptimization(store);
store(view, 1n); store(view, 2n);
%OptimizeFunctionOnNextCall(store);
store(view, target);
console.log(`CRASHJS cell=0x${cell.toString(16)} target=0x${target.toString(16)} rows=${rows.length} opt=${%GetOptimizationStatus(store).toString(2)}`);
addon.deref(ab);
console.log('UNREACHED');
```

Create `crates/retrace-guest/node/crash.json`:

```json
{"rows": [{"name": "base", "value": "0x400000000000"}, {"name": "offset", "value": "0xdead0000"}]}
```

- [ ] **Step 3: The build, the constants and the unit test.** Append to `main()` in `crates/retrace-guest/build.rs`, after the last fixture:

```rust
    // M48 Task 8 (spec §3g, D1): the N-API addon of node's crash demo, built only where Homebrew's
    // node headers are. Elsewhere it is skipped with a warning and `NODE_CRASH_ADDON` is `None`,
    // so the crate builds clean and `node_crash_e2e` announces its skip. The header is registered
    // for re-runs only when it exists: cargo re-runs a build script on every build while a
    // rerun-if-changed path is missing, which would rebuild every fixture each time. So a machine
    // that installs node later rebuilds the addon at the next change to this crate.
    let napi = "/opt/homebrew/include/node/node_api.h";
    let src = format!("{}/node/crash_addon.c", env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed={src}");
    if std::path::Path::new(napi).exists() {
        println!("cargo:rerun-if-changed={napi}");
        let bin = format!("{out}/crash_addon.node");
        let status = Command::new("clang")
            .args(["-arch", "arm64", "-bundle", "-undefined", "dynamic_lookup",
                   "-I", "/opt/homebrew/include/node", "-o", &bin, &src])
            .status().expect("clang crash_addon");
        assert!(status.success(), "the node crash addon failed to build against {napi}");
        println!("cargo:rustc-env=RETRACE_NODE_CRASH_ADDON={bin}");
    } else {
        println!("cargo:warning=SKIPPED the node crash addon: {napi} not found (`brew install node`); \
                  NODE_CRASH_ADDON is None and node_crash_e2e will announce its skip");
    }
```

Add to `crates/retrace-guest/src/lib.rs`, beside `CRASH_PY`:

```rust
/// M48 (spec §3g, D1): node's crash demo. `crash.js` reads `crash.json` beside it, computes the
/// target, has TurboFan compile a store of it synchronously (`--allow-natives-syntax`, R6), prints
/// the `CRASHJS` marker and loads through the pointer with the addon. node reads both at run time,
/// so these are repo paths, like `CRASH_PY`.
pub const CRASH_JS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/node/crash.js");
pub const CRASH_JS_JSON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/node/crash.json");
/// M48: the demo's N-API addon (`node/crash_addon.c`). `build.rs` builds it only where
/// `/opt/homebrew/include/node/node_api.h` exists; elsewhere it is `None`, and `node_crash_e2e`
/// announces its skip.
pub const NODE_CRASH_ADDON: Option<&str> = option_env!("RETRACE_NODE_CRASH_ADDON");
```

Add to the `tests` module, beside `crash_py_fixture_is_wired`. The `announce` helper is the same body as `util::announce`, which this crate cannot reach (CLAUDE.md's rule for a test in another crate):

```rust
    /// One line past libtest's capture: `util::announce`'s body (CLAUDE.md, honest-gate rule 2).
    fn announce(line: &str) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "{line}");
    }

    #[test]
    fn node_crash_demo_is_wired() {
        // M48 Task 8: the repo paths point at the demo, the data file carries the target the tests
        // assert by name, and the addon constant is Some exactly when node's headers exist, so a
        // build that silently failed to make the addon cannot pass as a machine without node.
        let js = std::fs::read_to_string(CRASH_JS).unwrap();
        assert!(js.contains("%OptimizeFunctionOnNextCall(store)"), "crash.js must force TurboFan on `store` (R6)");
        assert!(js.contains("CRASHJS cell="), "crash.js must print the M6-style marker");
        assert!(js.contains("'crash.json'"), "crash.js must read the data file beside it");
        let json = std::fs::read_to_string(CRASH_JS_JSON).unwrap();
        assert!(json.contains("\"0x400000000000\"") && json.contains("\"0xdead0000\""),
                "crash.json must carry base 0x400000000000 + offset 0xdead0000");
        let napi = std::path::Path::new("/opt/homebrew/include/node/node_api.h").exists();
        match NODE_CRASH_ADDON {
            Some(p) => {
                assert!(napi, "an addon was built though node_api.h is missing");
                let b = std::fs::read(p).unwrap();
                // mach_header_64: MH_MAGIC_64, CPU_TYPE_ARM64, and filetype MH_BUNDLE (8).
                assert_eq!(b[0..4], 0xfeed_facfu32.to_le_bytes(), "{p} is not a 64-bit Mach-O");
                assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 0x0100_000c, "{p} is not arm64");
                assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 8, "{p} is not a bundle");
            }
            None => {
                assert!(!napi, "node_api.h exists but build.rs built no addon");
                announce("SKIPPED node_crash_demo_is_wired's addon half: /opt/homebrew/include/node/node_api.h \
                          not found (`brew install node`). The addon was NOT checked.");
            }
        }
    }
```

Run it: the first run, before the constants exist, fails to compile, which is its red. Then add them and run again:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-guest --lib --no-fail-fast -- --test-threads=1 > $L/t8-guest.log 2>&1; echo "exit=$?"; grep -a -E 'node_crash_demo_is_wired|test result:|SKIPP' $L/t8-guest.log
```

Expect `node_crash_demo_is_wired ... ok`, 35 tests in the lib target (28 at M47, plus Tasks 1 and 3–6's six, plus this one), and no `SKIPPED` line on this host.

- [ ] **Step 4: `node_crash_e2e`.** Create `crates/retrace/tests/node_crash_e2e.rs`:

```rust
//! M48 rung 10 (spec §1 part 3, §3g, D1): the real node runs a repo-owned script whose optimized
//! JavaScript stores a computed bad pointer, and an N-API addon loads through it. The run records,
//! replays bit-for-bit, and a scripted debug session reverse-continues from the crash into V8's
//! JIT code, to the store.
//!
//! cpython_crash_e2e's structure and its four assertions, each on the difference rung 10 makes
//! (CLAUDE.md honest-gate rule 1), plus the fifth that is this rung's own:
//!   1. the script RAN: its marker is in the recorded stdout, `UNREACHED` is not, and the marker's
//!      `opt=` equals native's, so TurboFan compiled `store` exactly as it does natively;
//!   2. the crash IS the deref: node's own SIGSEGV handler is delivered the fault first (it
//!      re-raises), then the terminal Event::Crash has far == the computed target, a level-1
//!      translation DFSC, and the marker's thread;
//!   3. replay agrees, twice;
//!   4. `watch <cell>; reverse-continue` from the crash lands on the store, by its effect: the cell
//!      holds the warm-up value 2 before it and the target one stepi later;
//!   5. the store's pc lies inside a MAP_JIT mapping of the recording. An interpreter store passes
//!      1–4 too (spec §4).
//! The cell and the MAP_JIT ranges are DISCOVERED from the recording (the M6 marker convention).
//!
//! Neither node nor its headers are repo artifacts, so the test announces a skip naming what is
//! missing. kq_e2e, condvar_e2e, jitwp_e2e, simd_e2e and trim_e2e guard node's mechanisms without
//! it. The crash demo's trace is 1.2 GB (walls.md §2), so the test deletes it after its last
//! assertion; a failing run keeps it for diagnosis.
mod util;
use retrace_trace::Event;

const NODE: &str = "/opt/homebrew/bin/node";
const TARGET: u64 = 0x4000_DEAD_0000; // crash.json: 0x400000000000 + 0xdead0000
/// The debug session's bound, so a hang fails the gate rather than stalling it (M42). t0 M7
/// measured the session at 14 s on a release build; Task 8 measured the debug build's, and this
/// is at least twice that.
const DEBUG_SECS: u64 = 1200;

fn marker(stdout: &str) -> &str {
    stdout.lines().find(|l| l.starts_with("CRASHJS cell=0x"))
        .unwrap_or_else(|| panic!("missing the `CRASHJS cell=0x` marker in stdout:\n{stdout}"))
}

/// The value of `key` (`opt=`, `cell=0x`, …) in the marker line, up to the next space.
fn field<'a>(marker: &'a str, key: &str) -> &'a str {
    marker.split(' ').find_map(|w| w.strip_prefix(key))
        .unwrap_or_else(|| panic!("no `{key}` in the marker `{marker}`"))
}

#[test]
fn node_crashes_in_a_real_script_and_reverse_debugs_into_its_jit_code() {
    if !std::path::Path::new(NODE).exists() {
        util::announce(&format!("SKIPPED node_crashes_in_a_real_script…: {NODE} not installed \
            (`brew install node`). This gate did NOT run — it is not evidence of anything."));
        return;
    }
    let Some(addon) = retrace_guest::NODE_CRASH_ADDON else {
        util::announce("SKIPPED node_crashes_in_a_real_script…: the crash addon was not built \
            (/opt/homebrew/include/node/node_api.h not found at build time). This gate did NOT run \
            — it is not evidence of anything.");
        return;
    };
    let exe = std::fs::canonicalize(NODE).unwrap();
    let exe = exe.to_str().unwrap();
    let args = ["--allow-natives-syntax", retrace_guest::CRASH_JS, addon];

    let native = std::process::Command::new(exe).args(args).output().unwrap();
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    assert!(!native_out.contains("UNREACHED"), "natively the deref must fault:\n{native_out}");
    let native_opt = field(marker(&native_out), "opt=").to_owned();

    let (rec, trace) = util::record_dynamic_args(exe, &args);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();

    // 1. The script ran, and TurboFan compiled `store` as it does natively.
    assert!(stdout.contains("CRASHJS cell=0x"),
        "marker line missing (record exit {}). stdout:\n{stdout}\nstderr:\n{}", rec.code, rec.stderr);
    let m = marker(&stdout);
    assert_eq!(field(m, "target="), format!("{TARGET:#x}"), "the target is computed from crash.json");
    assert_eq!(field(m, "rows="), "2");
    assert_eq!(field(m, "opt="), native_opt, "TurboFan must compile `store` under retrace as natively");
    assert!(!stdout.contains("UNREACHED"), "the deref must not return. stdout:\n{stdout}");
    let cell = u64::from_str_radix(field(m, "cell=0x"), 16).expect("cell hex");

    // 2. The crash is the deref, after node's own handler saw it; and 5's ranges, from one decode.
    let evs = retrace_trace::Reader::open(&trace).unwrap();
    let jit = util::map_jit_ranges(&evs);
    let (mut marker_thread, mut delivery, mut crash) = (None, None, None);
    for (i, e) in evs.iter().enumerate() {
        match e {
            Event::Syscall { num, args, thread, .. }
                if (*num == retrace_arch::SYS_WRITE || *num == retrace_arch::SYS_WRITE_NOCANCEL) && args[0] == 1 =>
                marker_thread = Some(*thread),
            Event::SignalDelivery { sig: 11, si_addr, thread, .. } => delivery = Some((i, *si_addr, *thread)),
            Event::Crash { esr, far, thread, .. } => crash = Some((i, *esr, *far, *thread)),
            _ => {}
        }
    }
    drop(evs);
    let mthread = marker_thread.expect("a write to stdout (the marker) before the crash");
    let (ci, esr, far, cthread) = crash.expect("the trace ends in an Event::Crash");
    assert_eq!(far, TARGET, "the crash FAR must be the computed target (esr={esr:#x})");
    assert_eq!(esr & 0x3f, 0x05, "DFSC must be a level-1 translation fault (esr={esr:#x})");
    assert_eq!(cthread, mthread, "the deref must run on the marker's thread");
    let (di, si_addr, dthread) = delivery
        .expect("node's own SIGSEGV handler must be delivered the fault before the terminal crash (walls.md §2)");
    assert!(di < ci && si_addr == TARGET && dthread == cthread,
        "node's handler must see this fault first: delivery #{di} si_addr {si_addr:#x} thread \
         {dthread}, crash #{ci} thread {cthread}");
    assert_eq!(rec.code, 139, "a recorded crash exits 139 (M6). stderr:\n{}", rec.stderr);

    // 3. Replay agrees, twice.
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {i} must reproduce the crash. stderr:\n{}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout must be byte-identical");
        assert!(!rp.stderr.contains("DIVERGENCE"), "replay {i} diverged:\n{}", rp.stderr);
    }

    // 4. THE demo: run to the crash, watch the cell the addon read the pointer from, run BACKWARD
    //    to its last writer, and prove it by effect.
    let ts = trace.to_str().unwrap();
    let script = format!("continue; watch 0x{cell:x} 8; reverse-continue; x 0x{cell:x} 8; stepi; x 0x{cell:x} 8");
    let (code, out, err) = util::debug_bounded(ts, &script, DEBUG_SECS);
    assert_eq!(code, Some(0), "the debug session failed or hit its {DEBUG_SECS} s bound. stderr:\n{err}\nstdout:\n{out}");
    assert!(out.contains("guest crashed: pc="), "continue must park at the crash:\n{out}");
    let hit = out.lines().find(|l| l.starts_with(&format!("hit watch 0x{cell:x} (write at 0x")))
        .unwrap_or_else(|| panic!("reverse-continue must find a writer:\n{out}"));
    let pc_hex = hit.split("(write at 0x").nth(1).and_then(|r| r.split(')').next()).expect("the writer's pc");
    let pc = u64::from_str_radix(pc_hex, 16).expect("the writer's pc is hex");
    let hex = |v: u64| v.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{cell:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps expected:\n{out}");
    assert!(xs[0].contains(&hex(2)), "before the store the cell holds the warm-up value 2:\n{out}");
    assert!(xs[1].contains(&hex(TARGET)), "after one stepi the store of the target retired:\n{out}");

    // 5. The store is JIT code.
    assert!(jit.iter().any(|&(s, e)| (s..e).contains(&pc)),
        "the store's pc {pc:#x} must lie inside a MAP_JIT mapping {jit:x?}: an interpreter or \
         builtin store passes 1–4 too (spec §4)");
    let _ = std::fs::remove_file(&trace);
}
```

- [ ] **Step 5: Run it, detached.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test node_crash_e2e --no-run > $L/t8-build.log 2>&1; echo "build exit=$?"
nohup bash -c 'cargo test -p retrace --test node_crash_e2e --no-fail-fast -- --test-threads=1; echo "exit=$?"' > $L/t8-node_crash_e2e.log 2>&1 &
echo "pid=$!"
```

Poll with `grep -a -E '^exit=|^test .* (ok|FAILED)$|SKIPP' $L/t8-node_crash_e2e.log` until `exit=` appears. Expect `ok`, `exit=0` and no `SKIPPED`. Record the `finished in` time.

A failure is diagnosed against t0 §M7 before anything else:
- **The pc outside every `MAP_JIT` range** is the lever question §M7 names; it gets a Ruling, `--no-concurrent-recompilation` first.
- **A `SoftStep` panic** is Task 6's `flush_guest_tlb` guard (P9).
- **A bound kill** goes to Step 6's measurement.

- [ ] **Step 6: Measure the debug build's cost.** The test deletes its trace, so measure by hand on the same debug binary. Write `$L/t8-measure.sh`:

```bash
#!/bin/bash
# M48 Task 8: the debug build's cost for the crash demo, outside the test (which deletes its
# trace): record, one replay and the debug session, each under /usr/bin/time -l, and the trace size.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
L=$W/.superpowers/sdd/2026-10-02-retrace-m48-node
B=/private/tmp/claude-501/m48-t8-retrace
T=/private/tmp/claude-501/m48-t8-crash.bin
NODE=$(realpath /opt/homebrew/bin/node)
cd "$W" || exit 2
ADDON=$(ls -t target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/crash_addon.node | head -1)
cp target/aarch64-apple-darwin/debug/retrace "$B" && codesign -s - -f --entitlements retrace.entitlements "$B" || exit 2
perl -e 'alarm 900; exec @ARGV' "$B" > /dev/null 2>&1; echo "warm-up (codesign validation) rc=$?"
echo "binary sha256=$(shasum -a 256 $B | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) addon=$ADDON"
/usr/bin/time -l perl -e 'alarm 1800; exec @ARGV' $B record-dyn "$NODE" -o $T -- --allow-natives-syntax $W/crates/retrace-guest/node/crash.js "$W/$ADDON" < /dev/null 2> $L/t8-measure-rec.err | cat > $L/t8-measure-rec.out
echo "record rc=${PIPESTATUS[0]} trace=$(stat -f %z $T)"
/usr/bin/time -l perl -e 'alarm 1800; exec @ARGV' $B replay $T < /dev/null > $L/t8-measure-rp.out 2> $L/t8-measure-rp.err
echo "replay rc=$?"
CELL=$(grep -a -o 'cell=0x[0-9a-f]*' $L/t8-measure-rec.out | head -1 | cut -d= -f2)
/usr/bin/time -l perl -e 'alarm 1800; exec @ARGV' $B debug $T --script "continue; watch $CELL 8; reverse-continue; x $CELL 8; stepi; x $CELL 8" > $L/t8-measure-dbg.out 2> $L/t8-measure-dbg.err
echo "debug rc=$?"
grep -a -E ' real |maximum resident' $L/t8-measure-rec.err $L/t8-measure-rp.err $L/t8-measure-dbg.err
cat $L/t8-measure-dbg.out
rm -f $T
echo "measure done"
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
nohup bash $L/t8-measure.sh > $L/t8-measure.log 2>&1 &
echo "pid=$!"
```

Poll until `measure done` appears. Then:
1. **The report** carries:
   - each phase's `real` time and `maximum resident set size`;
   - the trace's size;
   - the `finished in` times of `node_e2e` (Task 7 Step 4) and `node_crash_e2e` (Step 5).

   These are the measured values behind the header's "about 6 min" and "about 8 min" and P10's figures on the debug build.
2. **Conditional, only if the debug session's `real` exceeds 600 s:** raise `DEBUG_SECS` to twice the measured time, rounded up to a multiple of 100, and update its doc comment's last sentence to the measured figure. If it is 600 s or less, change nothing.

- [ ] **Step 7: Regression, clippy, commit.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-guest --no-fail-fast -- --test-threads=1 > $L/t8-guest-all.log 2>&1; echo "guest exit=$?"
for t in skiplines cpython_crash_e2e; do cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > $L/t8-$t.log 2>&1; echo "$t exit=$?"; done
cargo clippy --workspace --all-targets -- -D warnings > $L/t8-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git status --short
git commit -m "M48 t8: the crash demo, rung 10 — crash.js, its N-API addon and data file; node_crash_e2e reverse-continues from the crash into V8's JIT code"
```

`git status --short` before the commit must list only the four new files under `crates/retrace-guest/node/` and `crates/retrace/tests/`, and the two modified `retrace-guest` files. A built `crash_addon.node` lives in `OUT_DIR`, never in the tree.

- [ ] **Step 8: Controls (on the committed tree).**
  1. **An interpreted store.** Delete the `%PrepareFunctionForOptimization(store);` and `%OptimizeFunctionOnNextCall(store);` lines from `crash.js`. `store` then runs three times in Ignition, below V8's feedback-allocation count, so the store is a bytecode handler's in node's own text.
     - Run `node_crash_e2e` detached and confirm it fails at assertion 5, `must lie inside a MAP_JIT mapping`, with 1–4 passing. The native `opt=` moves with the script, so assertion 1 still holds.
     - A pass is a finding, since the store would then be in baseline (Sparkplug) code. Report the pc and the ranges.
  2. **A silently skipped addon.** In `build.rs`, change the `napi` path to `/opt/homebrew/include/node/node_api_missing.h`. Run `cargo test -p retrace-guest --lib node_crash_demo_is_wired -- --test-threads=1` and confirm it fails at `node_api.h exists but build.rs built no addon`.

  Restore with `git checkout <t8 commit> -- crates/retrace-guest/node/crash.js crates/retrace-guest/build.rs`, confirm `git status --short` is empty, and record both symptoms.

---

### Task 9: The walk, the bench and the sweep (spec §3i)

The final binary re-walks every node command t0 walked. A release bench adds node to the performance table (spec §3k). The parked gates run once with `--ignored`, so a wall M48 moved is seen. The Apple sweep re-runs on an idle host.

The changes that could move a sweep row:
- the SCTLR UCI bit and the `kevent` row (spec §3i);
- the psynch rows;
- the SIMD restore and the partial `munmap`, which change behaviour for every guest.

Expect 49/54, or 50/54 on `dddiagnose`'s coin flip. Any other move is a finding.

- **Ruling T9-a.** The sweep refuses to start at a 1-minute load of 3 or more. After three refusals spanning at least 30 minutes, the controller rules whether to run it with the load noted (M47's T7-a convention for the headline figure).
- **Ruling T9-b.** The bench re-measures every row, not only node's, so the README's table moves to one date.

**Files:**
- Modify: `tools/bench.py` (a `node -e 'console.log(1)'` workload)
- Create: `docs/sweep-evidence/2026-10-02-m48/` (the scripts below, the walks' outputs and censuses, the bench, the parked-gate results, the sweep, the controls, `README.md`)
- Conditional: `crates/retrace/tests/apple_walls_e2e.rs`, only if Step 2 measures a parked wall moved (Step 7)

**Interfaces:**
- Consumes:
  - t0's walk conventions (`walk.sh`, `census.sh`) and its §M2–§M6 figures, which this walk is compared with;
  - the t0 base binary `/private/tmp/claude-501/m48-base-retrace`, which the controls attribute rows against;
  - Task 7's `RETRACE_SPRR` line;
  - Task 8's `crash.js` and its addon recipe;
  - M47's sweep log `docs/sweep-evidence/2026-09-30-m47/sweep.log`, the row baseline.
- Produces:
  - the evidence directory, whose `walk-*.census` files Task 10's census check reads;
  - the bench table and the sweep tally, for Task 11's docs.

Every script below lives in the evidence directory (`E`). Every build runs first, in Step 1, so no cargo runs while anything is recorded, benched or swept (M45 T3-a).

- [ ] **Step 1: The builds.** Write `$E/t9-build.sh` and run it in the foreground (it takes a few minutes):

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
mkdir -p $E
cat > $E/t9-build.sh <<'SH'
#!/bin/bash
# M48 Task 9: every cargo build this task needs, FIRST, so no cargo runs while anything is
# recorded, benched or swept (M45 T3-a). The debug binary is the sweep's (the gates' build), the
# release one the walk's and the bench's. Each signed copy runs once untimed, because the first run
# of a freshly signed binary can stall for minutes in codesign validation.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
L=$W/.superpowers/sdd/2026-10-02-retrace-m48-node
cd "$W" || exit 2
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') commit=$(git rev-parse --short HEAD)"
cargo build -p retrace > $L/t9-build-debug.log 2>&1; echo "debug build exit=$?"
cargo build --release -p retrace > $L/t9-build-release.log 2>&1; echo "release build exit=$?"
cargo test -p retrace --no-run > $L/t9-build-tests.log 2>&1; echo "test build exit=$?"
for v in debug release; do
  b=/private/tmp/claude-501/m48-t9-$v-retrace
  cp target/aarch64-apple-darwin/$v/retrace $b && codesign -s - -f --entitlements retrace.entitlements $b; echo "$v sign=$?"
  perl -e 'alarm 900; exec @ARGV' $b > /dev/null 2>&1; echo "$v warm-up rc=$? (2 is the usage exit)"
  shasum -a 256 $b
done
clang -arch arm64 -bundle -undefined dynamic_lookup -I /opt/homebrew/include/node \
  -o /private/tmp/claude-501/m48-t9-crash_addon.node crates/retrace-guest/node/crash_addon.c; echo "addon=$?"
echo "end $(date '+%Y-%m-%d %H:%M:%S %Z')"
SH
bash $E/t9-build.sh 2>&1 | tee $E/t9-build.out
```

Every `exit=` must be 0, each warm-up rc 2, and `addon=0`.

- [ ] **Step 2: The parked gates.** Write `$E/t9-parked.sh`:

```bash
#!/bin/bash
# M48 Task 9: every parked gate, run with --ignored on the final tree, so a wall M48 moved is seen
# (CLAUDE.md, honest-gate discipline). Each must still fail, at the wall its #[ignore] reason names.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
L=$W/.superpowers/sdd/2026-10-02-retrace-m48-node
cd "$W" || exit 2
echo "#[ignore] lines: $(git grep -c -E '^\s*#\[ignore' -- crates | awk -F: '{s+=$2} END {print s}')"
cargo test -p retrace --test apple_walls_e2e --no-fail-fast -- --ignored --test-threads=1 > $L/t9-parked-apple.log 2>&1; echo "apple_walls_e2e exit=$?"
cargo test -p retrace --test stackoverflow_rust_e2e --no-fail-fast -- --ignored --test-threads=1 > $L/t9-parked-stackoverflow.log 2>&1; echo "stackoverflow_rust_e2e exit=$?"
cargo test -p retrace --test symbols_e2e --no-fail-fast -- --ignored --test-threads=1 > $L/t9-parked-symbols.log 2>&1; echo "symbols_e2e exit=$?"
grep -a -h -E '^test .* (ok|FAILED)$' $L/t9-parked-*.log
grep -a -h -A3 -E "panicked at|^---- " $L/t9-parked-*.log | cut -c1-300
SH
```

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
bash $E/t9-parked.sh > $E/parked.txt 2>&1; tail -60 $E/parked.txt
```

Expect `#[ignore] lines: 9` and nine `FAILED` lines. The tests are `csh_records_and_replays`, `tcsh_records_and_replays`, `automationmodetool_records_and_replays`, `desdp_records_and_replays`, `dyld_info_records_and_replays`, `flex_records_and_replays`, `dddiagnose_records_and_replays`, the `stackoverflow_rust_e2e` test and `cache_symbol_e2e`.

For each, compare the failure text with the wall its `#[ignore]` reason names:
- `wait4` (7) for `csh` and `tcsh`;
- `kevent_id` (375) for `automationmodetool`;
- exec-in-place for the three M44 rows;
- the I/O Kit main port for `dddiagnose`;
- the blocked-signal wall for the stack overflow;
- the shared-cache symbol wall for `cache_symbol_e2e`.

Write one row per test into the evidence README's parked-gates table: test, the reason's wall, the observed failure, and same or moved. A `dddiagnose` pass is its known coin flip: re-run that one test twice more and record all three outcomes.

- [ ] **Step 3: The walk.** Write `$E/t9-walk.sh`, `$E/t9-census.sh` and `$E/t9-walks.sh`:

```bash
#!/bin/bash
# M48 Task 9 (spec §3i): one node walk on the final release binary, as t0's walk.sh without the
# probe: record under RETRACE_TRACE and RETRACE_SPRR, then replay twice. stdout goes through a pipe,
# as a test harness's does. Usage: t9-walk.sh <tag> -- <node args…>
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
B=/private/tmp/claude-501/m48-t9-release-retrace
NODE=$(realpath /opt/homebrew/bin/node)
tag=$1; shift 2
T=/private/tmp/claude-501/m48-t9-$tag.bin
export RETRACE_TRACE=1
export RETRACE_SPRR=1
s=$(date +%s)
perl -e 'alarm 900; exec @ARGV' $B record-dyn "$NODE" -o $T -- "$@" < /dev/null 2> $E/walk-$tag.err | cat > $E/walk-$tag.out
rc=${PIPESTATUS[0]}
secs=$(( $(date +%s) - s ))
unset RETRACE_TRACE RETRACE_SPRR
echo "$tag record rc=$rc secs=$secs trace=$(stat -f %z $T 2>/dev/null) traps=$(grep -ac '^\[trap\]' $E/walk-$tag.err)" | tee $E/walk-$tag.status
for i in 1 2; do
  s=$(date +%s)
  perl -e 'alarm 900; exec @ARGV' $B replay $T < /dev/null > $E/walk-$tag.rp$i.out 2> $E/walk-$tag.rp$i.err
  r=$?
  echo "$tag replay$i rc=$r secs=$(( $(date +%s) - s )) same_stdout=$(cmp -s $E/walk-$tag.out $E/walk-$tag.rp$i.out && echo yes || echo no)" | tee -a $E/walk-$tag.status
done
```

```bash
#!/bin/bash
# M48 Task 9: one walk's census from its traced stderr: every distinct syscall number (Task 10
# reads the `nums=` line), the counts M48's subsystems are measured by, the MAP_JIT mmaps, and
# the SPRR writes by thread and by value. Usage: t9-census.sh <tag>
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
f=$E/walk-$1.err
echo "walk=$1 traps=$(grep -ac '^\[trap\]' $f)"
echo "nums=$(grep -a '^\[trap\] num=' $f | sed 's/^\[trap\] num=\([-0-9]*\) .*/\1/' | sort -n -u | tr '\n' ' ')"
for n in 363 303 304 305 297 298 299 300 301 302 306 307 308 309 312 360 361 105 32 73 74 75; do
  echo "num $n: $(grep -ac "^\[trap\] num=$n " $f)"
done
perl -ne 'if (/^\[trap\] num=197 .*args=\[([^\]]*)\]/) { my @a = split /,/, $1; if (hex($a[3]) & 0x800) { $n++; print "map_jit len=$a[1] prot=$a[2] flags=$a[3]\n" } } END { print "map_jit=", ($n // 0), "\n" }' $f
echo "sprr writes=$(grep -ac '^\[M48 SPRR\] ' $f)"
grep -a '^\[M48 SPRR\] ' $f | awk '{print $4}' | sort -n | uniq -c | awk '{print "sprr thread " $2 ": " $1}'
grep -a '^\[M48 SPRR\] ' $f | awk '{print $6}' | sort | uniq -c | awk '{print "sprr value " $2 ": " $1}'
echo "msgh_ids=$(grep -a '^\[mach_msg2\]' $f | grep -a -o 'msgh_id=[0-9]*' | sort -u | tr '\n' ' ')"
```

```bash
#!/bin/bash
# M48 Task 9: the five walks t0 made, their censuses, and the crash demo's debug session on the
# release binary, one after another. Traces are deleted at the end; none is evidence.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
B=/private/tmp/claude-501/m48-t9-release-retrace
cd "$W" || exit 2
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') commit=$(git rev-parse --short HEAD) load=$(sysctl -n vm.loadavg) sha256=$(shasum -a 256 $B | cut -d' ' -f1)"
bash $E/t9-walk.sh e -- -e 'console.log(1)'
bash $E/t9-walk.sh t10 -- -e 'setTimeout(() => console.log(2), 10)'
bash $E/t9-walk.sh t2000 -- -e 'setTimeout(() => console.log(2), 2000)'
bash $E/t9-walk.sh natives -- --allow-natives-syntax -e 'function f(a,v){a[0]=v} const a=new BigUint64Array(1); %PrepareFunctionForOptimization(f); f(a,1n); f(a,2n); %OptimizeFunctionOnNextCall(f); f(a,3n); console.log(String(a[0]))'
bash $E/t9-walk.sh crash -- --allow-natives-syntax $W/crates/retrace-guest/node/crash.js /private/tmp/claude-501/m48-t9-crash_addon.node
for t in e t10 t2000 natives crash; do bash $E/t9-census.sh $t > $E/walk-$t.census 2>&1; done
CELL=$(grep -a -o 'cell=0x[0-9a-f]*' $E/walk-crash.out | head -1 | cut -d= -f2)
/usr/bin/time -l perl -e 'alarm 900; exec @ARGV' $B debug /private/tmp/claude-501/m48-t9-crash.bin --script "continue; watch $CELL 8; reverse-continue; x $CELL 8; stepi; x $CELL 8" > $E/walk-crash.dbg.out 2> $E/walk-crash.dbg.err
echo "debug rc=$?"
rm -f /private/tmp/claude-501/m48-t9-e.bin /private/tmp/claude-501/m48-t9-t10.bin /private/tmp/claude-501/m48-t9-t2000.bin /private/tmp/claude-501/m48-t9-natives.bin /private/tmp/claude-501/m48-t9-crash.bin
echo "walks done $(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)"
```

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
nohup bash $E/t9-walks.sh > $E/walks.log 2>&1 &
echo "pid=$!"
```

Poll `$E/walks.log` until `walks done` appears. Then `cat $E/walks.log $E/walk-*.census $E/walk-crash.dbg.out`. The traced stderr of a whole node walk is about 190 KB (the probe's `w1.err`), so every `walk-*.err` is committed whole.

**What the walk must show.**

The status lines:

| Walk | Record rc | Prints | Each replay |
|---|---|---|---|
| `e` | 0 | `1` | rc 0, `same_stdout=yes` |
| `t10` | 0 | `2` | rc 0, `same_stdout=yes` |
| `t2000` | 0 | `2` | rc 0, `same_stdout=yes` |
| `natives` | 0 | `3` | rc 0, `same_stdout=yes` |
| `crash` | 139 | the `CRASHJS` marker, not `UNREACHED` | rc 139, `same_stdout=yes` |

The censuses, compared with measurements §M2–§M6 row by row:
- **psynch:** only 303, 304 and 305 among the psynch numbers.
- **`MAP_JIT`:** `map_jit=1` with `len=0x10000000 prot=0x0 flags=0x41842`.
- **SPRR:** every SPRR value is commpage `+0x110` or `+0x118`'s (§M1(c)).
- **Threads:** `num 360` is 6.
- **No socket:** `num 32: 0`.
- **kevent:** its counts within t0's walk-to-walk range.

The crash session:
- the hit is a `hit watch` line;
- the cell reads `02 00 …` and then the target;
- its `real` and `maximum resident set size` are recorded.

A record that stops anywhere else is a new wall:
- a row or a small model is a Ruling and a step in the task that owns its subsystem, then this step re-runs;
- a new subsystem is H5.

- [ ] **Step 4: The bench.** In `tools/bench.py`:
  - add `NODE = os.path.realpath("/opt/homebrew/bin/node")` below `PY`;
  - append `("node -e 'console.log(1)'", NODE, ["-e", "console.log(1)"]),` as the last entry of `workloads`;
  - in the docstring, change "(`jq`, `python@3.14`)" to "(`jq`, `python@3.14`, `node`)".

  `realpath` of a missing path returns the path unchanged, so the existing `os.path.exists` check prints the `SKIPPED` line.

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
nohup bash -c 'echo "start $(date "+%Y-%m-%d %H:%M:%S %Z") commit=$(git rev-parse --short HEAD) load=$(sysctl -n vm.loadavg)"; python3 tools/bench.py --runs 5 --retrace target/aarch64-apple-darwin/release/retrace; echo "bench exit=$?"; echo "end $(date "+%Y-%m-%d %H:%M:%S %Z") load=$(sysctl -n vm.loadavg)"' > $E/bench.txt 2>&1 &
echo "pid=$!"
```

Poll until `bench exit=` appears. Every row must be timed, with no `FAILED` and no `SKIPPED` on this host.

The bench runs node with stdout on `/dev/null`, a path the walks (a pipe) did not take. If the node row prints `FAILED`, that is a finding:
- record the failing phase from a by-hand run with stdout on `/dev/null`;
- name it in the evidence README and in Task 11's Known limits;
- leave node out of the README's table.

- [ ] **Step 5: The sweep.** Write `$E/t9-sweep.sh`:

```bash
#!/bin/bash
# M48 Task 9 (spec §3i): the full Apple corpus on a signed scratch copy of this task's debug binary,
# detached, with no cargo running in this worktree (t9-build.sh ran every build first). M47's
# t6-sweep.sh shape, gated on an idle host (Ruling T9-a): it refuses to start at a 1-minute load
# of 3 or more.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
B=/private/tmp/claude-501/m48-t9-debug-retrace
cd "$W" || exit 2
load1=$(sysctl -n vm.loadavg | awk '{print $2}')
if awk -v l="$load1" 'BEGIN { exit !(l >= 3) }'; then echo "REFUSED: 1-minute load $load1 >= 3 at $(date '+%Y-%m-%d %H:%M:%S %Z')"; exit 3; fi
export RETRACE_SWEEP_KEEP=$E/sweep
( while :; do echo "$(date '+%H:%M:%S') $(sysctl -n vm.loadavg)"; sleep 30; done ) > $E/sweep-load.txt 2>&1 &
sampler=$!
{
  echo "pidstart=$$"
  echo "binary=$B sha256=$(shasum -a 256 "$B" | cut -d' ' -f1) commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo "load-start: $(uptime) vm.loadavg=$(sysctl -n vm.loadavg)"
  tools/apple-sweep.sh "$B"
  echo "SWEEP_EXIT=$?"
  echo "load-end: $(uptime) vm.loadavg=$(sysctl -n vm.loadavg) date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
} > "$E/sweep.log" 2>&1
kill $sampler 2>/dev/null
echo "wrapper done"
```

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
nohup bash $E/t9-sweep.sh > $E/sweep-wrapper.log 2>&1 &
echo "pid=$!"
```

Poll `$E/sweep-wrapper.log` until it reads `wrapper done` or `REFUSED`. On `REFUSED`, wait and re-run it, appending each attempt's line to `$E/sweep-refusals.txt`. After three refusals spanning at least 30 minutes, stop and hand Ruling T9-a's question to the controller.

When it ran: `grep -a -E '^TALLY|^SWEEP_EXIT|^FAIL' $E/sweep.log`.

- [ ] **Step 6: The row diff and the controls.** Write `$E/t9-rowdiff.sh`. It is M47's `t6-rowdiff.sh` with:
  - `W` set to the m48 worktree;
  - `E` set to this directory;
  - `S` set to `/private/tmp/claude-501`;
  - `A` set to `/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-09-30-m47/sweep.log`;
  - `B` set to `$E/sweep.log`;
  - the temp names `$S/m48base.$$` and `$S/m48rows.$$`;
  - the header comment naming M48 Task 9 and M47's `a8a1ecd` sweep as the baseline.

  Its body is otherwise identical: copy it from that file and change only those lines. Run it with `bash $E/t9-rowdiff.sh > $E/t9-rowdiff.out 2>&1`.

  Write `$E/moved-rows.txt`:
  - a `#` comment line naming the source (`rowdiff-norm.txt`);
  - one path per line for every row whose normalised `ROW` differs from M47's;
  - always `/usr/bin/dddiagnose`.

Write `$E/t9-controls.sh`. It is M47's `t6-controls.sh` with these changes:

| Variable | Value |
|---|---|
| `W` | the m48 worktree |
| `E` | this directory |
| `S` | `/private/tmp/claude-501/m48-t9-ctl` |
| `BASE` | `/private/tmp/claude-501/m48-base-retrace` |
| `T6` | renamed `T9`, set to `/private/tmp/claude-501/m48-t9-debug-retrace` |

The header comment says the same `TRACE_MAGIC` (`RT\x00\x0b`) is on both binaries. Its `run` function and the rounds are unchanged:
- `r1`/`r2` over the moved rows, alternating base and t9;
- `d3`–`d6` over `dddiagnose` alone.

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
mkdir -p /private/tmp/claude-501/m48-t9-ctl
nohup bash $E/t9-controls.sh > $E/controls.txt 2>&1 &
echo "pid=$!"
```

Poll until the `end` line appears. Attribute each moved row:
- **M48's:** it differs on both t9 runs and on neither base run. A row that goes PASS → FAIL this way is a regression M48 caused. Stop and report it to the controller before Task 10; it is fixed under a Ruling, and if the fix round fails, that is H1.
- **Host state:** both outcomes occur on both binaries.
- **Unattributed:** neither.

`dddiagnose` gets six samples per binary.

- [ ] **Step 7: Wall bookkeeping (conditional).**
  - **A parked gate whose failure moved to another wall** (Step 2): rewrite its `#[ignore]` reason in place, in the reason's existing form. Name the new wall, its landmark, number and caller, and this directory as evidence. This is CLAUDE.md's honest-gate discipline; the count does not change. Then run `cargo test -p retrace --test apple_walls_e2e --no-run` to confirm it compiles.
  - **A parked gate that passed all three runs**, `dddiagnose` aside: do not un-ignore it here. Report it to the controller. Un-parking changes the tally (9 → 8 ignored) and needs a Ruling that names what M48 cleared, with a base-binary control.
  - **Any new `#[ignore]`** is H2: halt.
  - **None of these:** change nothing.

- [ ] **Step 8: The evidence README; commit.** Write `$E/README.md` with these sections:
  1. **Header.** The run dates, the host (chip, macOS build, kernel), the three binaries with their sha256 (debug, release, base) and the commit.
  2. **Files.** For each file: the command that produced it, the binary, and the date.
  3. **The walk.** The table above, filled from `walk-*.status`, beside t0's §M2 figures. Then each census against §M3–§M6, one row per count. Then the crash session's hit line, the two cell values, `real` and peak RSS.
  4. **The bench.** `bench.txt`'s table, the start and end loads, and any finding from Step 4.
  5. **The parked gates.** Step 2's table.
  6. **The sweep.** `TALLY`, the start and end loads, `rowdiff.txt`, `rowdiff-norm.txt`, `moved-rows.txt`, and each moved row's attribution from `controls.txt`.
  7. **Verdicts.**
     - whether any new wall was met (H5);
     - whether any row moved because of M48;
     - whether the headline stays 49 of 54;
     - every finding, each named, never smoothed.

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
git add tools/bench.py docs/sweep-evidence/2026-10-02-m48 ':(exclude)docs/sweep-evidence/2026-10-02-m48/*.bin' ':(exclude)docs/sweep-evidence/2026-10-02-m48/sweep/*.bin'
git diff --cached --name-only | grep -a -c '\.bin$'
git status --short | grep -a -v '\.bin$'
git commit -m "M48 t9: the walk — node e/t10/t2000/natives/crash on the final binary, the parked gates re-run, the bench with a node row, the Apple sweep and its controls"
```

The `.bin` count must print 0. If Step 7 rewrote a reason, add `crates/retrace/tests/apple_walls_e2e.rs` to the `git add` and name the row in the message.

---

### Task 10: The audit and the oracle controls (spec §3j, as corrected by §11a item 1)

The `kevent` and psynch mirrors sit inside the `Event::Syscall` chain, after its arm-top `verify_thread` (Global Constraints), so that one call guards them, and the count stays seven. Its proof is a retag test over each new landmark, plus deleting the arm-top call (§11a item 1).

The audit checks the spec's §3j list as greps over the committed tree:
- symmetry rule 1, arm by arm;
- rule 2 for the JIT;
- every new `Box_` field through every path;
- a seek test for each new state;
- the generic arm's asserts.

It adds four more checks:
- spec §1 part 5's census;
- H3;
- the Review Focus pins;
- a test for every refusal prefix.

**Ruling T10-a.** Spec §3j names "all three construction sites". Task 4's field-through-every-path pattern names six:
1. the `Box_` struct;
2. `BoxState`;
3. `checkpoint()`;
4. `from_checkpoint`;
5. the three literal constructors (`load_with_pac`, `load_dynamic`, `restore`);
6. `dbg_internal_state`'s format string.

The check greps each site with its own pattern, so a function parameter named `kq: u64` (Task 4's `kevent_timed_out`) or `jit: &JitSet` does not count as a field site. It runs every pattern on M46's `kq` first as their control. On the M47 tree, `kq` reads `decl=2 lit=3 ckpt=1 from=1 dbg=1`, and `excl`, which is not in the format string, reads `decl=2 lit=0 ckpt=1 from=1 dbg=0`.

**Files:**
- Modify: `crates/retrace/tests/thread_oracle.rs` (+2 tests, one helper)
- Create: `docs/sweep-evidence/2026-10-02-m48/t10-audit.sh`, `t10-audit.txt`
- Conditional: `crates/retrace-arch/tests/census.rs`, only if Step 3 finds a walk number missing from `CENSUS`

**Interfaces:**
- Consumes:
  - Task 2's `retrace_arch::SYS_KEVENT`, `SYS_PSYNCH_CVWAIT` and `is_psynch`, and its two generic-arm assert messages;
  - Task 4's `KQ_DYN` in mode `wake`, the `kevent` record arm and mirror calling `Box_::guest_kevent`, `Box_::note_fd_effects` at its three call sites (Ruling K6) and the `gkq` field;
  - Task 5's `CONDVAR_DYN` in mode `pingpong`, the psynch arm and mirror calling `Box_::guest_psynch`, and the `psynch` field;
  - Task 6's `jit` field and its `checkpointparity` test;
  - Tasks 4–6's Review Focus tests by name, and their seek tests in `kq_e2e`, `condvar_e2e` and `jitwp_e2e`;
  - the six refusal prefixes;
  - Task 9's `walk-*.census`.
- Produces:
  - two thread-oracle tests;
  - the audit's verdicts (`t10-audit.txt`), which Task 11's final review and docs cite.

- [ ] **Step 1: The two retag tests.** Append to `crates/retrace/tests/thread_oracle.rs`:

```rust
/// M48 Task 10 (spec §3j as corrected by §11a item 1). The `kevent` mirror sits inside the
/// `Syscall` chain after the arm-top `verify_thread`, so that one call is what guards it, and a
/// `kevent` landmark retagged to another live thread must diverge as the schedule. `kq_dyn wake`:
/// thread A blocks in `kevent` and thread B triggers it, so two live threads issue syscalls and the
/// retag target is real.
#[test]
fn a_wrong_thread_on_a_kevent_landmark_is_a_divergence() {
    retag_fixture_and_expect_divergence(retrace_guest::KQ_DYN, &["wake"], retrace_arch::SYS_KEVENT);
}

/// M48 Task 10: the same for `psynch_cvwait`, on `condvar_dyn pingpong`, whose two threads each wait.
#[test]
fn a_wrong_thread_on_a_psynch_cvwait_landmark_is_a_divergence() {
    retag_fixture_and_expect_divergence(retrace_guest::CONDVAR_DYN, &["pingpong"], retrace_arch::SYS_PSYNCH_CVWAIT);
}

/// `retag_and_expect_divergence` over any fixture and argv: retag the first `Syscall` landmark for
/// `num_wanted` to another thread id that genuinely appears in the same trace, and assert replay
/// refuses it as the schedule. The reasoning for a live id, not a constant, is
/// `a_wrong_thread_on_replay_is_a_divergence`'s.
fn retag_fixture_and_expect_divergence(guest: &str, argv: &[&str], num_wanted: u64) {
    let (rec, trace) = util::record_dynamic_args(guest, argv);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let mut events = retrace_trace::Reader::open(&trace).unwrap();
    let mut ids: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::Syscall { thread, .. } => Some(*thread), _ => None }).collect();
    ids.sort_unstable(); ids.dedup();
    assert!(ids.len() >= 2, "{guest} {argv:?} must schedule two threads that issue syscalls; got {ids:?}");

    let i = events.iter().position(|e| matches!(e, Event::Syscall { num, .. } if *num == num_wanted))
        .unwrap_or_else(|| panic!("no Syscall landmark for num {num_wanted} — {guest} {argv:?} no \
                                   longer reaches the mirror this test exists to cover"));
    let orig = match &events[i] { Event::Syscall { thread, .. } => *thread, _ => unreachable!() };
    let other = *ids.iter().find(|&&t| t != orig).expect("a genuinely live second id, not a constant");
    if let Event::Syscall { thread, .. } = &mut events[i] { *thread = other; }

    let mut w = retrace_trace::Writer::create(&trace).unwrap();
    for e in &events { w.append(e).unwrap(); }
    drop(w);

    let rep = util::replay(&trace);
    assert_eq!(rep.code, 3, "CLI exit 3 is the Divergence convention; stderr:\n{}", rep.stderr);
    assert!(rep.stderr.contains("the schedule diverged"),
        "the divergence must be the THREAD oracle's, not merely some divergence; stderr:\n{}", rep.stderr);
}
```

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace --test thread_oracle --no-fail-fast -- --test-threads=1 > $L/t10-thread_oracle.log 2>&1; echo "exit=$?"; grep -a -E '^test .* (ok|FAILED)$|test result:' $L/t10-thread_oracle.log
```

Expect 9 passed: the oracle exists, so these pass at once, and Step 6 is their red.

- [ ] **Step 2: The audit script.** Write `docs/sweep-evidence/2026-10-02-m48/t10-audit.sh`:

```bash
#!/bin/bash
# M48 Task 10 (spec §3j, §6, §1 part 5): the structural audit as greps over the committed tree.
# Each check prints its evidence and a VERDICT line; a FINDING is reported, never smoothed.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
E=$W/docs/sweep-evidence/2026-10-02-m48
S=/private/tmp/claude-501
cd "$W" || exit 2
BASE=e6caa65
C=crates/retrace-core/src/lib.rs
X=crates/retrace-box/src/lib.rs
v() { if [ "$1" = "$2" ]; then echo "VERDICT $3: ok ($1)"; else echo "VERDICT $3: FINDING (got $1, want $2)"; fi; }
echo "audit of $(git rev-parse --short HEAD) on $(date '+%Y-%m-%d %H:%M:%S %Z')"

echo "== 1. No trace-format change (H3)"
v "$(git diff $BASE..HEAD -- crates/retrace-trace | wc -l | tr -d ' ')" 0 "crates/retrace-trace diff lines since $BASE"
v "$(grep -c -F 'pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x0b";' crates/retrace-trace/src/lib.rs)" 1 "TRACE_MAGIC is RT\\x00\\x0b"

echo "== 2. Symmetry rule 1: guest_kevent and guest_psynch once in record_box and once in ReplaySession::advance, the record call before the generic arm; note_fd_effects twice in record_box (Task 4 Ruling K6) and once in advance"
ra=$(grep -n 'pub fn advance(&mut self)' $C | head -1 | cut -d: -f1)
g=$(grep -n 'reached the generic forward arm' $C | head -1 | cut -d: -f1)
echo "ReplaySession::advance starts at line $ra; the generic arm's first assert is at line $g"
for f in guest_kevent guest_psynch; do
  grep -n -A2 "\.$f(" $C
  set -- $(grep -n "\.$f(" $C | cut -d: -f1)
  if [ "$#" = 2 ] && [ "$1" -lt "$ra" ] && [ "$2" -gt "$ra" ]; then echo "VERDICT $f sides: ok (record $1, replay $2)"; else echo "VERDICT $f sides: FINDING (lines $*)"; fi
done
# Task 4 Ruling K6: the record-side console-close arm calls note_fd_effects too, because replay
# finishes that landmark through the generic mirror. So: two record calls (the console-close arm,
# then the generic arm), both before `advance`, and one replay call (the generic mirror) inside it.
grep -n -A2 "\.note_fd_effects(" $C
set -- $(grep -n "\.note_fd_effects(" $C | cut -d: -f1)
if [ "$#" = 3 ] && [ "$1" -lt "$ra" ] && [ "$2" -lt "$ra" ] && [ "$3" -gt "$ra" ]; then echo "VERDICT note_fd_effects sides: ok (record $1 and $2, replay $3)"; else echo "VERDICT note_fd_effects sides: FINDING (lines $*)"; fi
for f in guest_kevent guest_psynch; do
  r=$(grep -n "\.$f(" $C | head -1 | cut -d: -f1)
  if [ "$r" -lt "$g" ]; then echo "VERDICT $f before the generic arm: ok ($r < $g)"; else echo "VERDICT $f before the generic arm: FINDING ($r >= $g)"; fi
done
echo "(compare the argument lists printed above by eye: the same Box_ method with the same arguments on both sides)"

echo "== 3. The generic forward arm asserts every new number"
grep -n 'reached the generic forward arm' $C
v "$(grep -c 'kevent (363) reached the generic forward arm' $C)" 1 "the kevent assert"
v "$(grep -c 'psynch syscall {num} reached the generic forward arm' $C)" 1 "the psynch assert"

echo "== 4. Symmetry rule 2: the SPRR register and the JIT view live below the trace"
grep -n -i 'sprr\|jit' $C
v "$(grep -i 'sprr' $C | grep -c -v -E '^\s*(//|\*)')" 0 "SPRR in retrace-core code (comments aside)"

echo "== 5. Every new Box_ field at Task 4's six sites (Ruling T10-a); kq, M46's field, is the patterns' control"
for f in kq gkq psynch jit; do
  d=$(grep -c -E "^\s*(pub )?$f: [A-Za-z_:<>]+," $X)
  l=$(grep -c -E "[ ,{]$f: [A-Za-z_:]*::default\(\)" $X)
  c=$(grep -c -E "^\s*$f: self\.$f\.clone\(\)," $X)
  r=$(grep -c -E "^\s*$f: state\.$f\.clone\(\)," $X)
  s=$(grep -c -E "[ \"]$f=\{:\?\}" $X)
  v "decl=$d lit=$l ckpt=$c from=$r dbg=$s" "decl=2 lit=3 ckpt=1 from=1 dbg=1" "$f: Box_ and BoxState, the three literals, checkpoint(), from_checkpoint, dbg_internal_state"
done

echo "== 6. verify_thread keeps seven call sites (§11a item 1)"
v "$(grep -c 'self\.verify_thread(' $C)" 7 "verify_thread call sites"

echo "== 7. Restore parity: each new state has a seek test, and checkpointparity has M48's"
for t in kq_e2e condvar_e2e jitwp_e2e; do
  grep -n -E 'fn [a-z_]*seek[a-z_]*\(' crates/retrace/tests/$t.rs
  n=$(grep -c -E 'fn [a-z_]*seek[a-z_]*\(' crates/retrace/tests/$t.rs)
  if [ "$n" -ge 1 ]; then echo "VERDICT $t seek tests: ok ($n)"; else echo "VERDICT $t seek tests: FINDING (none)"; fi
done
grep -n 'M48' crates/retrace-box/tests/checkpointparity.rs | head -5

echo "== 8. Review Focus: every pinned test is defined exactly once"
for n in an_event_list_aliasing_the_change_list_is_read_before_it_is_written \
         the_runtime_detection_probe_returns_natives_one_event \
         a_wake_of_the_current_thread_writes_the_vcpu \
         a_timeout_on_the_only_thread_answers_on_the_vcpu \
         a_timed_wait_on_the_only_waiter_answers_on_the_vcpu \
         the_sequence_window_wraps_as_synch_internal_h_computes \
         a_signal_across_the_sequence_wrap_wakes_the_waiter \
         the_stamped_extents_are_the_ranges_minus_their_noaccess_extents \
         an_unprotect_inside_a_jit_range_is_restamped_by_the_view_not_left_data \
         the_v8_shape_none_mapped_then_mprotected_rwx_runs_its_code \
         a_kevent_refused_on_replay_is_a_divergence_naming_it_not_a_panic \
         a_cvwait_refused_on_replay_is_a_divergence_naming_it_not_a_panic; do
  v "$(git grep -c -E "fn $n\(" -- crates | awk -F: '{s+=$2} END {print s+0}')" 1 "$n"
done

echo "== 9. Every refusal prefix has a test file that names it"
for p in 'M48: kevent ' 'M48: pipe ' 'M48: psynch ' 'M48: SPRR ' 'M48: MAP_JIT ' 'M48: a signal is pending on thread '; do
  s=$(git grep -c -F "$p" -- 'crates/*/src/*.rs' | awk -F: '{s+=$2} END {print s+0}')
  t=$(git grep -l -F "$p" -- 'crates/*/tests/*.rs' | wc -l | tr -d ' ')
  echo "prefix [$p]: source lines $s, test files $t"
  if [ "$t" -ge 1 ]; then echo "VERDICT [$p] tested: ok"; else echo "VERDICT [$p] tested: FINDING (no test file names it; check the src unit tests by hand)"; fi
done

echo "== 10. Spec §1 part 5: every syscall number the final node walks reached is in CENSUS"
perl -0ne 'if (/CENSUS: &\[i64\] = &\[(.*?)\];/s) { print "$_\n" for ($1 =~ /-?\d+/g) }' crates/retrace-arch/tests/census.rs | sort -u > $S/m48-t10-census.txt
grep -a -h '^nums=' $E/walk-*.census | sed 's/^nums=//' | tr ' ' '\n' | grep -v '^$' | sort -u > $S/m48-t10-walknums.txt
echo "walk numbers: $(wc -l < $S/m48-t10-walknums.txt | tr -d ' '); CENSUS: $(wc -l < $S/m48-t10-census.txt | tr -d ' ')"
comm -23 $S/m48-t10-walknums.txt $S/m48-t10-census.txt | sed 's/^/MISSING /'
v "$(comm -23 $S/m48-t10-walknums.txt $S/m48-t10-census.txt | wc -l | tr -d ' ')" 0 "walk numbers missing from CENSUS"
echo "psynch numbers the walks reached (only 303, 304 and 305 are modelled):"
grep -a -h -E '^num (29[7-9]|30[0-9]|312): [1-9]' $E/walk-*.census | sort | uniq -c

echo "== 11. Nine #[ignore] lines, none new (H2)"
v "$(git grep -c -E '^\s*#\[ignore' -- crates | awk -F: '{s+=$2} END {print s}')" 9 "#[ignore] lines"
```

```bash
export E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
bash $E/t10-audit.sh > $E/t10-audit.txt 2>&1; grep -a 'VERDICT' $E/t10-audit.txt
```

**Expected:** every `VERDICT` reads `ok`, and check 10's psynch list shows only `num 303`, `num 304` and `num 305`. Quote every verdict in the task report.

**A `FINDING`:**
- is read against its printed evidence before anything else;
- if it is a miscount, an extra `jit: ` binding in a function signature for instance, is recorded as one, line by line;
- if it is a real asymmetry, a missing field site or a missing test, is reported to the controller, who assigns its fix to the task that owns the code.

Check 2's argument lists are compared by eye, and the report quotes both lines of each pair.

- [ ] **Step 3: The census (conditional).** Only if check 10 printed `MISSING` lines, for each missing number `n`:
  1. Confirm it has a row: `cargo test -p retrace-arch --test census` passes with `n` added.
  2. Add `n` to `CENSUS` in sorted position.
  3. Append one sentence to the M48 doc paragraph: "Task 10's census of the final node walks (`docs/sweep-evidence/2026-10-02-m48/walk-*.census`) added <the numbers>, rows they already had."
  4. Re-run `bash $E/t10-audit.sh > $E/t10-audit.txt 2>&1`.

  A missing number with no row cannot occur: the walk would have stopped at the M33 panic. If one does, it is a finding. With no `MISSING` line, change nothing.

- [ ] **Step 4: Clippy, commit.**

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
cargo test -p retrace-arch --no-fail-fast -- --test-threads=1 > $L/t10-arch.log 2>&1; echo "arch exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > $L/t10-clippy.log 2>&1; echo "clippy exit=$?"
git add -A && git status --short
git commit -m "M48 t10: the audit — thread_oracle retags a kevent and a psynch_cvwait landmark; the structural audit's verdicts (t10-audit.txt)"
```

- [ ] **Step 5: Controls (on the committed tree).**
  1. **The arm-top oracle.** In `crates/retrace-core/src/lib.rs`, delete the `self.verify_thread(*rthread, pc)?;` directly below the comment that begins `// M15 Task 4: the thread oracle`.
     - Run `cargo test -p retrace --test thread_oracle a_wrong_thread_on_a -- --test-threads=1`.
     - Confirm both new tests fail: replay accepts the retagged trace (exit 0), or reports some other divergence without `the schedule diverged`.
     - This is the control §11a item 1 assigns: it proves the arm-top call is what guards each new mirror.
  2. **No `kevent` arm.** In `record_box`, replace the `kevent` record arm's guard (the condition that selects 363) with `false`.
     - Run `cargo test -p retrace --test kq_e2e the_runtime_detection_probe_returns_natives_one_event -- --test-threads=1`.
     - Confirm it fails with `kevent (363) reached the generic forward arm` in the recorder's stderr: §1 part 5's assert is live.
  3. **No psynch arm.** Replace the psynch record arm's guard with `false`.
     - Run `cargo test -p retrace --test condvar_e2e a_timed_wait_on_the_only_waiter_answers_on_the_vcpu -- --test-threads=1`.
     - Confirm it fails with `psynch syscall 30` and `reached the generic forward arm` in the recorder's stderr.

  Restore after each with `git checkout <t10 commit> -- crates/retrace-core/src/lib.rs`, confirm `git status --short` is empty, and record each symptom.

---

### Task 11: The close (spec §3k, §6)

1. Predict the gate from source.
2. Run it chunked and detached.
3. Tally and reconcile file by file against M47's close.
4. Write the docs: append the status log, edit `current-state.md` in place, and make the README's and CLAUDE.md's edits.
5. The final review, at most one fix wave, and a scoped re-review.
6. Merge `--no-ff` from the main checkout, with tree-hash identity, the ledger copied before the worktree is removed, and the worktree and branch removed.
7. Then the milestone's one outward-facing action, `git push origin main`, only on a green gate and a clean final review (the operator's 2026-10-02 grant).

**Files:**
- Create: `.superpowers/sdd/2026-10-02-retrace-m48-node/{predict,gate,tally}.sh` (the ledger, git-excluded)
- Modify: `docs/status-log.md` (append), `docs/current-state.md` (edit in place), `README.md`, `CLAUDE.md`

**Interfaces:**
- Consumes:
  - every task's commit and report;
  - t0's measurements file;
  - Task 7's and Task 8's measured durations;
  - Task 9's evidence (walk, bench, sweep, parked gates);
  - Task 10's audit;
  - M47's close: 950/0/10 over 160, 958 `#[test]`, its `predict.sh`, `gate.sh` and `tally.sh` in the main checkout's `.superpowers/sdd/2026-09-30-retrace-m47-gitwrite/`.
- Produces:
  - the gate's tally;
  - the docs at the merge commit;
  - the merge on `main`, pushed;
  - the ledger in the main checkout.

- [ ] **Step 1: Predict.** Write `$L/predict.sh`. It is M47's with:
  - the `cd` set to the m48 worktree;
  - `BASE=e6caa65` (M47's merge, whose code the M48 spec commit `a7365da` carries unchanged);
  - `F=/private/tmp/claude-501/m48-predict-files.txt`;
  - its header comment naming M48 Task 11 and that base.

  The body is otherwise M47's verbatim.

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
bash $L/predict.sh > $L/predict.txt 2>&1; cat $L/predict.txt
```

Expected, exactly (the header's +115), in the script's sorted order:

```
DELTA crates/hv-sys/tests/simd.rs: 1 -> 2 (1)
DELTA crates/retrace-arch/tests/nodeshapes.rs: 0 -> 6 (6)
DELTA crates/retrace-box/src/gkq.rs: 0 -> 11 (11)
DELTA crates/retrace-box/src/jit.rs: 0 -> 8 (8)
DELTA crates/retrace-box/src/psynch.rs: 0 -> 11 (11)
DELTA crates/retrace-box/src/thread.rs: 10 -> 13 (3)
DELTA crates/retrace-box/tests/checkpointparity.rs: 4 -> 5 (1)
DELTA crates/retrace-box/tests/gkq.rs: 0 -> 10 (10)
DELTA crates/retrace-box/tests/jit.rs: 0 -> 5 (5)
DELTA crates/retrace-box/tests/psynch.rs: 0 -> 4 (4)
DELTA crates/retrace-box/tests/simdctx.rs: 0 -> 3 (3)
DELTA crates/retrace-box/tests/trim.rs: 0 -> 6 (6)
DELTA crates/retrace-core/src/machmsg.rs: 34 -> 35 (1)
DELTA crates/retrace-guest/src/lib.rs: 28 -> 35 (7)
DELTA crates/retrace/tests/condvar_e2e.rs: 0 -> 8 (8)
DELTA crates/retrace/tests/jitwp_e2e.rs: 0 -> 8 (8)
DELTA crates/retrace/tests/kq_e2e.rs: 0 -> 12 (12)
DELTA crates/retrace/tests/node_crash_e2e.rs: 0 -> 1 (1)
DELTA crates/retrace/tests/node_e2e.rs: 1 -> 2 (1)
DELTA crates/retrace/tests/simd_e2e.rs: 0 -> 3 (3)
DELTA crates/retrace/tests/thread_oracle.rs: 7 -> 9 (2)
DELTA crates/retrace/tests/trim_e2e.rs: 0 -> 3 (3)
TOTAL #[test]: 958 -> 1073 (115)
TARGETS crates/retrace-arch/tests: 6 -> 7
TARGETS crates/retrace-box/tests: 44 -> 49
TARGETS crates/retrace/tests: 83 -> 89
```

Reconcile every difference file by file before the gate. A file whose count differs carries the reason in its task's addendum or report: a t0 wall's own tests (Task 0's addendum), or a fix round's guard. The prediction becomes the measured total, written in `$L/predict-reconciled.md` as one line per differing file with its reason.

With the census pair counted twice, as since M45, the gate must report passed + ignored = TOTAL + 2. That is 1075 at the planned TOTAL, over 160 + 12 = 172 binaries, and the ignored count must be 9.

- [ ] **Step 2: The gate, detached.** Write `$L/gate.sh`:

```bash
#!/bin/bash
# M48 close (Task 11 Step 2): the chunked gate, M47's retargeted. Every chunk runs --no-fail-fast;
# each chunk's exit code is captured before any pipe and lands in gate-summary.txt. Read the logs,
# never this script's own exit status. retrace-box runs as a whole package, so `Doc-tests
# retrace_box` is in the box chunk (CLAUDE.md's second mouth); no library crate is split per
# target, so no --doc chunk is owed. stdin is /dev/null for every chunk, so nothing a test spawns
# can read the target list.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node || exit 1
D=.superpowers/sdd/2026-10-02-retrace-m48-node
S=$D/gate-summary.txt
: > "$S"
run() {
  name=$1; shift
  "$@" < /dev/null > "$D/gate-$name.log" 2>&1
  echo "$name exit=$?" >> "$S"
}
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') commit=$(git rev-parse --short HEAD) load=$(sysctl -n vm.loadavg)" >> "$S"
run ws cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1
run box cargo test -p retrace-box --no-fail-fast -- --test-threads=1
run bins cargo test -p retrace --bins --no-fail-fast -- --test-threads=1
ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' > "$D/gate-targets.txt"
while read -r n; do
  run "e2e-$n" cargo test -p retrace --test "$n" --no-fail-fast -- --test-threads=1
done < "$D/gate-targets.txt"
run clippy cargo clippy --workspace --all-targets -- -D warnings
echo "end $(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)" >> "$S"
echo DONE >> "$S"
```

Write `$L/tally.sh`, M47's with `D` set to this ledger. Then run the gate on the last code commit, with nothing else building in the worktree, and never edit `crates/` while it runs (M46):

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
git status --short; git rev-parse --short HEAD
nohup bash $L/gate.sh > $L/gate-wrapper.log 2>&1 &
echo "pid=$!"
```

It takes over an hour: M47's took 54 minutes, and the node targets add about 14 on the debug build. Poll `tail -3 $L/gate-summary.txt` until `DONE` appears. Then:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
grep -a -c ' exit=0$' $L/gate-summary.txt; grep -a ' exit=' $L/gate-summary.txt | grep -a -v ' exit=0$'
bash $L/tally.sh | tee $L/tally.txt
grep -a -l 'SKIPP' $L/gate-*.log
git grep -c -E '^\s*#\[ignore' -- crates | awk -F: '{s+=$2} END {print "ignore lines=" s}'
```

Expected:
- **93** `exit=0` lines (ws, box, bins, 89 `e2e`, clippy), and no other `exit=` line;
- **`tally.txt`:**
  - `ws 240/0/0 over 31`;
  - `box 433/0/0 over 51`;
  - `bins 32/0/0 over 1`;
  - `e2e 361/0/9 over 89`;
  - `e2e logs: 89`;
  - `all 1066/0/9 over 172`;
  - or the reconciled figures from Step 1;
- no `SKIPP` in any gate log, since node, jq, Homebrew Python, Xcode's git and `/usr/bin/lldb` are all present on this host. A skip is named in the docs, and is never counted as a run;
- `ignore lines=9`.

**A red chunk** is diagnosed from its log, fixed once (`M48 close fix: <what>`), and its chunk and clippy re-run green. A second red is H1: stop, write it into the ledger, and report.

**A count off the reconciled prediction** is reconciled file by file before the docs are written.

- [ ] **Step 3: The docs.** Edit on the branch, after the gate. Every figure comes from a log, an addendum or an evidence file named here, never from memory.

**`docs/status-log.md`: append** `## M48-node: real node, JIT on, recorded, replayed and reverse-debugged into its JIT code`, in M47's section shape. It carries, in order:

1. **An opening paragraph.**
   - The v1 bar: `python3` since M26/M39, `git` since M47, and `node` now, so the bar is closed.
   - **What landed:** one bullet per task 1–10, each with its commit hashes from `git log --oneline a7365da..HEAD`.
   - **Outcome:**
     - node v25.6.1's two runs, from `node_e2e`;
     - rung 10's reverse-continue into a `MAP_JIT` range, from `node_crash_e2e`;
     - the sweep tally and its verdict, from Task 9's README;
     - the bench's node row.
   - **Operator decisions:**
     - the 2026-10-02 brainstorm choices (node with JIT in one milestone, the rung-8 demo, absorb small and halt on a new subsystem, K1/P1/J1/D1);
     - the 2026-10-02 autonomy grant (merge and push at the close on a green gate and a clean final review);
     - the controller's rulings §11 records: the SIMD fix as a task (§11b item 1), partial `munmap` absorbed (§11b item 5).
2. **The milestone's numbers, measured from source at the last code commit.**
   - **`#[test]` attributes:** +115 (or the reconciled figure), 958 → 1073, per file as `predict.txt` lists them.
   - **New test binaries:** twelve, 160 → 172.
   - **Rows:** five `arg_kinds` rows (363, 303, 304, 305, 105), so `CENSUS` goes 120 → 125 plus Task 10's additions, and `EXPECTED_DIFFS` 37 → 40.
   - **One forward:** 3419.
   - **New fixtures:** `simd_dyn`, `trim_dyn`, `kq_dyn`, `condvar_dyn`, `jitwp_dyn` and `sprrprobe`, and the node demo's three files.
   - **Generic-arm asserts:** two new ones (`kevent`, `is_psynch`).
   - **`verify_thread`:** 7 → 7.
   - **`TRACE_MAGIC`:** unchanged at `RT\x00\x0b`.
   - **`#[ignore]` lines:** 10 → 9.
   - **Commits:** every commit after the spec (`a7365da`) and the plan.
3. **Forward pointers** to earlier sections, by this file's line numbers (`grep -n` them when writing). Each earlier section is left as it was written.
   - M47's "`kevent` on a guest `kqueue()`, node's next wall, and the V8 JIT behind it" (`:15604`) is paid.
   - M47's "Timed waits" (`:15601`), and M46's (`:15099`), are partly paid. `kevent`'s timeout and `psynch_cvwait`'s deadline are modelled on one deadline queue. `sleep()`, `__semwait_signal` (334), `setitimer` (83) and `__ulock_wait2` (544) stay owed.
   - Any M47 owed item Task 9's sweep or controls re-measured.
4. **Subsections, in this order:**
   - `### What t0 measured` (M1–M9 from the measurements file, each with its decision);
   - `### The SIMD ABI (Task 1, <hash>)`;
   - `### The rows (Task 2, <hash>)`;
   - `### Partial munmap (Task 3, <hash>)`;
   - `### The deadline queue and guest kqueues (Task 4, <hashes>)`;
   - `### psynch condition variables (Task 5, <hashes>)`;
   - `### JIT write-protect (Task 6, <hashes>)`;
   - `### node (Task 7, <hash>)`, with its controls' symptoms;
   - `### The crash demo (Task 8, <hash>)`, with Step 6's measured costs and the controls;
   - `### The walk (Task 9, <hash>)`;
   - `### The sweep (Task 9)`;
   - `### The audit (Task 10, <hash>)`, quoting its verdicts and the three controls;
   - `### The gate`: the tally table, the reconciliation against `predict.txt`, the run's dates and loads, and the skip check;
   - `### The final review`: its findings by severity and the fix wave's outcome;
   - `### What measurement changed`: every spec claim a measurement corrected, §11's items included;
   - `### Rulings`:
     - the spec's R1–R9, with R9's verdict;
     - the plan's rulings: T7-a, T7-b, T8-a, T8-b, T8-c, T9-a, T9-b, T10-a, and every Ruling the addenda and the ledger recorded, each with its cost if wrong;
   - `### Named weakness`, written as measured:
     - node's two gates skip without Homebrew node, so `kq_e2e`, `condvar_e2e`, `jitwp_e2e`, `simd_e2e` and `trim_e2e` are the repo-owned guards;
     - the SPRR witness is an env-gated line, not trace data;
     - the passed-deadline semantics are the probe's measurement;
     - the pipe byte accounting is not on node's path (P3) and is guarded by `kq_dyn pipe` alone;
   - `### What stays owed`. Spec §7's not-done list:
     - process creation;
     - workloops and `kevent_id`;
     - `kevent64` or `kevent_qos` on a guest kqueue;
     - readiness on any fd that is not a guest pipe, so no network and no interactive node;
     - the psynch mutex and rwlock calls and `cvclrprepost`;
     - lldb-driven node sessions;
     - WebAssembly;
     - performance work beyond R9's measurement.

     Then:
     - the timed waits above;
     - lazy `PROT_NONE` backing (the 526 MB and 1.2 GB traces, 4.2 GB RSS);
     - the parked review minors;
     - M47's owed items, by reference to its section.

**`docs/current-state.md`: edit in place** (anchors are the text as it stands at M47):
- **"Guest breadth".**
  - Change "and since M47 Xcode's `git` for its local workflow." to "since M47 Xcode's `git` for its local workflow, and since M48 Homebrew's `node` with its JIT on."
  - In the rung table, insert after rung 8's row:
    - `| 9 | Homebrew **node** (v25.6.1), JIT on | `-e 'console.log(1)'`, and a 2-second `setTimeout` whose deadline the idle jump reaches; V8's `MAP_JIT` code range and its per-thread write-protect are modelled (below) |`
    - `| 10 | real node on a real script that crashes | `crash.js`: an N-API addon and a TurboFan-compiled store of a computed pointer; recorded, replayed, and reverse-continued from the crash into V8's JIT code, to the store |`
- **The Apple-sweep paragraph.** After M47's sentences, add M48's run:
  - date, commit, loads and `TALLY`;
  - each moved row with its attribution, from Task 9's README;
  - whether the headline figure stays 49, by M47's T7-a convention: it moves only on a row M48 caused.
- **Capabilities.** Add a bullet after M47's `git` bullet, headed "**Homebrew `node` records and replays with its JIT on; guest kqueues, psynch condition variables, deadline-bounded waits, partial `munmap` and `MAP_JIT` write-protect are modelled; every SIMD restore installs its value.** Since M48, which makes `node` the third of the vision spec's three workloads." It describes, each with the test that guards it:
  - the hv-sys `set_simd` fix (`simd_e2e`);
  - the backing split (`trim_e2e`);
  - guest kqueues: `EVFILT_USER`, a guest pipe's readiness by byte accounting (R4), the never-ready answer to `EVFILT_READ` on the inherited stdout, timeouts, `EV_ONESHOT` (`kq_e2e`);
  - the deadline queue and its one idle jump (R7);
  - psynch `cvwait`/`cvsignal`/`cvbroad` ported from libpthread-539.100.4 (`condvar_e2e`);
  - the per-thread SPRR register with R1's start value, the two admitted commpage values (R2), the view over `MAP_JIT` ranges minus their no-access extents, SCTLR UCI, and the step-safe `flush_guest_tlb` (`jitwp_e2e`);
  - `RETRACE_SPRR`.

  It closes with "Nothing new is recorded and `TRACE_MAGIC` did not move; `verify_thread` keeps its seven call sites".
- **"Gate".** Replace the paragraph that begins "**Gate:** 950 passed" with M48's. It names:
  - the figures from `tally.txt` and the commit;
  - the dates and loads from `gate-summary.txt`;
  - the 89 targets;
  - the fix wave's re-runs, from Step 5.

  Replace "ten gates are parked" with nine, and remove `node_e2e`'s parked clause, saying M48 un-parked it. Replace the M46→M47 reconciliation table with an M47→M48 table: one row per `DELTA` line, each test named by what it asserts, from the test files. Update the sentence after it: +115 or the reconciled figure, twelve new binaries named, 1073 attributes.
- **Usage.** After the `RETRACE_TRACE` paragraph, add: "`RETRACE_SPRR=1` on a `record`/`record-dyn` run prints one `[M48 SPRR] thread <n> wrote <value>` line per guest write of `S3_6_C15_C1_5`. The register is below the trace, so nothing in a recording shows a write; `node_e2e`'s JIT assertion reads this line."
- **Performance.**
  - Replace the table, its date line and its machine line with Task 9's `bench.txt` (the node row included).
  - Change the start-up bullet to give node's start-up, measured, and why: V8 reserves about 640 MB that retrace backs and snapshots in full.
  - Change the last paragraph's "the table has not been re-measured since" to say M48 re-measured it on that date.
- **Known limits.**
  - Replace the "**Homebrew `node` stops at `kevent` (363)**" bullet with one on node's costs and fidelity:
    - the debug-build times from Task 8 Step 6;
    - the trace sizes from Task 9's `walk-*.status`;
    - the 4.2 GB peak RSS of one `reverse-continue` session (P10), as lazy `PROT_NONE` backing is not modelled;
    - the cooperative scheduler's effect on V8: background compilation and GC helpers run only when main blocks, which is fidelity, not determinism;
    - rung 10 needing `--allow-natives-syntax` (R6);
    - R1's named deviation (the register reads 0 before a thread's first write, where native reads `+0x118`'s value);
    - the measured toggle cost (P5, Task 9's census);
    - the origin of node's `{0, 1 ns}` waits from t0 M4.
  - In the paragraph naming "`kevent` (363), `kevent64` (369) and `kevent_id` (375) have **no row**", drop 363 from it and add "`kevent` (363) is modelled on a guest kqueue since M48 (What works today)".
  - In the M46 paragraph, replace "`kevent`, `kevent64` or `kevent_qos` on a guest `kqueue()` descriptor, `sleep()` and every other timed wait" with "`kevent64` or `kevent_qos` on a guest `kqueue()` descriptor (M48 models `kevent` on one), and every timed wait but `kevent`'s timeout and `psynch_cvwait`'s deadline (M48), `sleep()` among them".
  - Add one bullet listing every M48 refusal by its message prefix and what it refuses. Build it by reading each `"M48: ` message in the source (`git grep -n '"M48: ' -- crates`), and name the test that pins each one.
  - Change "**Ten gates are parked**" to nine, and drop `node_e2e`'s clause from that bullet.
- **Testing.** Add one paragraph:
  - the node targets run for minutes on the debug build (Task 8 Step 6's two `finished in` times);
  - they write up to 1.2 GB into the temp dir and delete it on a pass;
  - a gate run needs that much free disk and about 4.2 GB of RAM for `node_crash_e2e`'s debug session.

**`README.md`:**
- In the intro paragraph, insert "Homebrew's `node` with its JIT on, " after "the CPython interpreter, ".
- Replace the "**Threads and signals.**" bullet with: "- **Threads, timers and signals.** Multi-threaded guests (`std::thread`, pthreads with their condition variables, GCD's global queues, GCD timers on the uptime clock, and kqueues with timeouts) record and replay; every timed wait runs on a synthetic clock, so a 2-second timer costs no wall-clock time. Signals reach the thread they were sent to, through the handler the program installed."
- After the "**Crashes.**" bullet, add: "- **JIT code.** node's V8 compiles JavaScript at run time; retrace models Apple's per-thread JIT write-protect, so a recording can be reverse-continued into JIT-compiled code, to the store that wrote a bad pointer."
- In "**Command-line programs only.**", append: "kqueue readiness is modelled for a program's own pipes, so nothing that waits on a socket, a terminal or an inherited descriptor runs (no network, no interactive node)."
- Update the sweep figure's sentences with M48's, as `current-state.md` states them.
- In "**Traces are large…**", replace "Tens to hundreds of MiB, uncompressed, and" with "Tens of MiB to over a GiB, uncompressed (node's V8 reserves address space retrace backs in full: about <the `e` walk's trace size, in MiB, from `walk-e.status`> for `console.log(1)`), and".
- In "## Performance", replace the table and its date line from Task 9's `bench.txt`, and keep the README's columns. Add node's start-up to the paragraph below the table.

**`CLAUDE.md`:**
- **The gates list.** Replace "`node_e2e` (M47: node parked at its measured wall; skips loudly without Homebrew node)" with the clauses below. Check each against its file's test names, and drop any clause the file does not test.
  - `simd_e2e` (M48: callee-saved SIMD registers across a thread switch and a `sigreturn`, and a threaded replay independent of the host environment — the hv-sys `set_simd` ABI fix)
  - `trim_e2e` (M48: V8's aligned-reservation trim, a partial `munmap` that splits its backing)
  - `kq_e2e` (M48: guest kqueues — libuv's runtime-detection probe, a cross-thread `EVFILT_USER` wake, a timeout reached by the idle jump, a pipe's readiness, `EV_ONESHOT`, the refusals by value, and seeks into a blocked `kevent` and inside a timed one)
  - `condvar_e2e` (M48: psynch condition variables — ping-pong, broadcast, a timed wait that expires and one signalled first, node's {0, 1 ns} wait on the only thread, the refused mutex pair, a replay-side refusal, and a seek across a blocked `cvwait`)
  - `jitwp_e2e` (M48: `MAP_JIT` write-protect per thread — native's toggle sequence, two threads in opposite modes, a store to a protected page recorded as the crash it is, V8's none-then-RWX shape, and a step across a toggle)
  - `node_e2e` (M48 rung 9: node prints 1 with a `MAP_JIT` mapping and an SPRR write in the recording, and a 2-second `setTimeout` lands exactly on its deadline by the idle jump; skips loudly without Homebrew node)
  - `node_crash_e2e` (M48 rung 10: node's crash demo, reverse-continued from the crash to a store whose pc is in a `MAP_JIT` range; skips loudly without Homebrew node or its headers)
- **Symmetry rule 2.** Change "(as with the timebase MRS, the Apple-IMPDEF undef-MRS, and the B-family FPAC strip)" to "(as with the timebase MRS, the Apple-IMPDEF undef-MRS, the B-family FPAC strip, and since M48 the per-thread SPRR register `S3_6_C15_C1_5` with the `MAP_JIT` view it selects)".
- **The W^X bullet.** Append: "Since M48 a `MAP_JIT` range is the one region whose stamp changes at run time: the running thread's SPRR value selects `ATTR_CODE` or `ATTR_DATA` for its pages, never both, and the flip flushes through `flush_guest_tlb`, which disarms `MDSCR_EL1.SS` and `MDE` around its stub so a single step can cross it."
- **"Guest threads".**
  - Change "A thread blocks for **three** reasons" to "A thread blocks for **five** reasons".
  - After the sentence ending "(`ThreadTable::unpark`).", insert: "Since M48 a thread also blocks in `kevent` on a guest kqueue (`BlockReason::Kevent`), woken by a knote activation or its deadline, and in `psynch_cvwait` (`BlockReason::Cv`), correlated by the condition variable's **guest address** and woken by `cvsignal`/`cvbroad` or its deadline. Every deadline is on `synthetic_tsc`, `schedule_after_block` makes one idle jump to the earliest of all of them, and a woken thread's return is written at the wake (`deliver_wake`)."
- **The generic arm's asserts.** Change "and since M47 `madvise` and `fork` only;" to "since M47 `madvise` and `fork`, and since M48 `kevent` (363) and every SDK psynch number (`retrace_arch::is_psynch`) only;".
- **The oracle paragraph.** After the sentence about the `SignalDelivery` landmark's inline comparison, add: "M48's `kevent` and psynch mirrors sit inside the `Syscall` chain after its arm-top check, so the count stays seven; `thread_oracle.rs` retags one landmark of each."

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
grep -a -n 'M47: node parked\|\*\*three\*\* reasons\|Ten gates are parked\|stops at `kevent` (363)' CLAUDE.md docs/current-state.md README.md
git add docs/status-log.md docs/current-state.md README.md CLAUDE.md && git status --short
git commit -m "M48 docs: the status-log section, current-state edited in place (rungs 9 and 10, the gate, the limits), README and CLAUDE.md"
```

The `grep` must print nothing: every stale anchor is replaced.

- [ ] **Step 4: The final review (controller).** Save the diff and write the brief:

```bash
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
H=$(git rev-parse --short HEAD)
git diff a7365da..HEAD -- . ':(exclude)docs/sweep-evidence' > $L/review-final-a7365da..$H.diff
wc -l $L/review-final-a7365da..$H.diff
```

Dispatch one reviewer on Opus with `$L/final-review-brief.md`, which states:
- **The diff.** Its path, and that evidence directories are excluded from it but readable.
- **What to read first.** The spec, its §11 and the plan.
- **What the review checks:**
  - the Review Focus list (five items) and spec §4's guard table: does each guard assert the difference it claims?;
  - CLAUDE.md's two symmetry rules, the oracle rule and the platform invariants;
  - every refusal by value, identical on both sides, a `Divergence` on replay, never a panic there;
  - every new `Box_` field through `BoxState` and every constructor;
  - `deliver_wake` writing the vCPU when the woken thread is current;
  - the SPRR arm's admission of only the two commpage values;
  - `flush_guest_tlb`'s `MDSCR` guard restored on every path;
  - the docs against the code and the evidence: every number in `current-state.md`, `README.md` and the status log traced to a log or evidence file;
  - every comment's citations.
- **What to write.** `$L/final-review.md`, with findings labelled **Critical**, **Important** or **Minor**, each with `file:line`, the evidence, and a proposed fix. A clean verdict says so in its first line.

- [ ] **Step 5: The fix wave and the scoped re-review (only if Step 4 found a Critical or Important).**
  - **The fix wave.** One wave fixes every Critical and Important finding, committed as `M48 fix wave: <what> (final review <ids>)`. Minors are parked in the status log's "What stays owed" unless a fix is one line.
  - **Re-runs.** Re-run the chunks the wave touched, each `--no-fail-fast`, logs `$L/fixwave-<chunk>.log`:

    | A change to | Re-runs |
    |---|---|
    | `crates/retrace-box/` | the box chunk and `kq_e2e condvar_e2e jitwp_e2e simd_e2e trim_e2e thread_oracle node_e2e node_crash_e2e` |
    | `crates/retrace-core/` | the ws chunk and the same e2e set |
    | `crates/retrace-arch/` | the ws chunk |
    | `crates/retrace-guest/` | the ws chunk and the e2e set |
    | one test file | its target |
    | anything | clippy over `--workspace --all-targets -- -D warnings` |

    The node targets run detached, as in Task 7. The full gate is not re-run (M47's convention); the gate paragraph says which chunks re-ran.
  - **The scoped re-review.** Dispatch it over `git diff <pre-wave head>..HEAD` into `$L/final-rereview.md`. A Critical or Important finding still open there is H1: stop, do not merge, and report.
  - **The docs.** Amend the status log's "The final review" and current-state's gate paragraph with the wave and its re-runs, and commit `M48 docs: the fix wave`.

- [ ] **Step 6: Merge (controller, from the main checkout).** Every check must pass before the next command runs:

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
git status --short
git rev-parse --short main worktree-m48-node
git merge-base --is-ancestor main worktree-m48-node; echo "main-is-ancestor=$?"
```

Before merging, check three things:
- **Untracked paths the branch tracks.** `git status --short` may show only untracked paths. Compare each untracked path the branch also tracks (the plan file is the expected one) with the branch's copy: `git show worktree-m48-node:<path> | cmp - <path>`.
  - If they are identical, move it out of the way: `mkdir -p /private/tmp/claude-501/m48-main-untracked && mv <path> /private/tmp/claude-501/m48-main-untracked/`.
  - If they differ, stop and report.
- **A tracked change on `main`:** stop and report.
- **`main-is-ancestor` must be 0.** If `main` moved since the branch was cut, stop and report: a merge with new commits needs a re-gate.

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
git merge --no-ff worktree-m48-node -m "Merge M48-node: real node, JIT on, recorded, replayed and reverse-debugged into its JIT code" -m "Gated at <last code commit>: <the tally's all line>."
test "$(git rev-parse HEAD^{tree})" = "$(git rev-parse worktree-m48-node^{tree})"; echo "tree-identical=$?"
cp -R .claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node .superpowers/sdd/
diff -rq .claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node .superpowers/sdd/2026-10-02-retrace-m48-node; echo "ledger-diff=$?"
```

- `tree-identical` and `ledger-diff` must both be 0.
- The merge message's second paragraph carries the real commit and tally line; nothing in angle brackets survives into it.
- The ledger is git-excluded (`.git/info/exclude`), so it is copied, never committed (M45's lesson: `git worktree remove` deletes it).

Then remove the worktree and the branch:

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
git worktree remove .claude/worktrees/m48-node; echo "remove=$?"
git branch -d worktree-m48-node; echo "branch=$?"
git worktree list; git status --short
```

If `git worktree remove` refuses for untracked files, list them with `git status --short` run inside the worktree.
- If every one is in the ledger copy just verified, or is ignored build output, re-run it with `--force`.
- Otherwise stop and report.

- [ ] **Step 7: Push (controller): the milestone's one outward-facing action.** Run it only when all of these hold:
  - the gate is green, with Step 2's tally reconciled;
  - the final review is clean, or its fix wave's re-review has no open Critical or Important;
  - the docs are committed;
  - `tree-identical=0`;
  - the worktree is removed with its ledger copied.

  Otherwise do not push. Report which condition failed.

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
git log --oneline origin/main..main | head -40
git push origin main; echo "push=$?"
git status -sb | head -1
```

`push` must be 0, and the status line must show `main` level with `origin/main`.

- [ ] **Step 8: Memory (controller).**
  - Write `/Users/noahmitchem/.claude/projects/-Users-noahmitchem-Documents-GitHub-retrace/memory/retrace-m48-node.md`, with the memory frontmatter (`name: retrace-m48-node`, a one-line description, `type: project`). It records:
    - the merge commit and that it is pushed;
    - the gate figure;
    - that the v1 bar is closed;
    - the lessons this milestone's ledger records that no repo document already carries (its "What measurement changed" items that bind a successor), each with **Why:** and **How to apply:** lines;
    - links to `[[retrace-m48-node-charter]]` and `[[retrace-m47-gitwrite]]`.
  - Add `- [M48 node](retrace-m48-node.md) — <merge hash> + PUSHED (<tally>); rung 9 and rung 10, the v1 bar closed` to `MEMORY.md`, filled from the merge.
  - Mark the charter memory's grant as discharged, in its own file.

---
