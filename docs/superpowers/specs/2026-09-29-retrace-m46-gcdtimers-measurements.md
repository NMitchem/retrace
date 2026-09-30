# M46-gcdtimers t0 measurements

**Companion to** `2026-09-29-retrace-m46-gcdtimers-design.md` (§3a: M1–M4; §11 items 1, 10, 11 and
12) and to the plan's Task 0.

**Where and when.** Measured 2026-09-29 on this machine:
- macOS 26.5.2 (25F84), kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`;
- `Apple clang version 21.0.0 (clang-2100.1.1.101)`;
- `lldb-2100.0.17.203`.

Branch `worktree-m46-gcdtimers` was at **`2bc0730`** for every run. The rulings came after the
measurements, in `3d40e41`.

**Binary.** `cargo build -p retrace` at `2bc0730`, unmodified: no source was edited in t0. It is
`target/aarch64-apple-darwin/debug/retrace`, ad-hoc signed by `tools/codesign-run.sh` (sha256
`220a378e9ff12e22bef70de53e9c6820355ff30238ff02ff8cc302c8b48802d6`). M1(b), M1(c)'s disassembly,
M2 and M3 ran **natively** under lldb.

**Evidence.** `docs/sweep-evidence/2026-09-29-m46-t0/`, whose README says which command produced
each file. No trace (`.bin`) and no built binary is committed. The source copies stay in the
ledger, `.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers/t0/src/`.

**Outcome.**
- **R7's premise is confirmed** (M1(a)).
- The UPTIME timer path reads only `mach_get_times` (M1(b)).
- The guest's clock starts at `0x67e_d716_dc56`, and its counter is the emulated
  `S3_4_C15_C10_6` (M1(c)).
- The manager's first-use and reuse words are observed as inferred. The redelivery word is not
  observed (M2).
- M3 found a third registration on every fixture's path. **H3 was triggered and ruled** (spec §11
  items 11–12: Rulings T0-a and T0-b).
- The base count is 853 (M4).
- Halt 7, H2 and Halt 6 were not triggered.

**The fixtures.** Both are byte copies of the plan's texts. Every build exited 0 and every native
run exited 0 with the expected output (`native-*.out`):

| run | native output |
|---|---|
| `after` default | `fired cell 0x104c14000`, `fired`, `done` |
| `after two` | `A`, `B`, `done` |
| `after wall` | `fired`, `done` |
| `after clock` | `clock ok` |
| `timer` | `tick 1`, `tick 2`, `tick 3`, `done` |

No fixture text changed.

---

## M1 — the clocks

### M1(a) — the clock channel on retrace's side

**Command.** The plan's Task 0 Step 3, verbatim: `export RETRACE_TRACE=1`, then `cargo build -p
retrace` (exit 0), then `record-dyn … after_dyn -- clock`, then `record-dyn … after_dyn`. Two
follow-ups placed the fallback:
- `retrace debug m46-clock.bin --script "break 0x1804b1b30; continue; where; regs"`;
- a native `image lookup -a` of the trap pcs.

**Result.**
- The `clock` record exits 0, and `m1a-clock.out` is `clock bad`.
- `m1a-clock.err` has 19 `num=116` traps.
  - 18 are at pc `0x1804b1b60` with a zero third argument. That is
    `libsystem_kernel`__gettimeofday + 12`, plain `gettimeofday`.
  - One has a nonzero third argument:

  ```
  [trap] num=116 (0x74) pc=0x1804b1b30 args=[0x27ff5d8,0x0,0x27ff5e8,0xfffffc088,0x0,0x0,0x0,0x0]
  ```

- `m1a-pc-lookup.log` symbolizes it: `0x1804b1b30` is `__gettimeofday_with_mach + 8`, and its return
  address `x30 = 0x1804afd34` (`m1a-clock-fallback-where.out`) is `mach_get_times + 100`. It is
  landmark 245, thread 0.
- **What it hands the guest** (`m1c-gtod-and-fallback.out`): the recorded mach-time out-param at
  `0x27ff5e8` is `27 d9 4e 4d 99 06 00 00`, which is **`0x6994d4ed927`**, the host's
  `mach_absolute_time`.

**Why the stamp test fails** (`m1c-gtod-and-fallback.out`, `x 0xfffffc120 0x28`). The frozen
`NEWTIMEOFDAY_DATA` has:
- `TimeStamp_tick = 0x6994c9e0f85`;
- `TimeStamp_sec = 0x6abc4e40`;
- `Ticks_per_sec = 0x16e3600` (24 MHz).

The guest's clock starts at about `0x67ed716dc56` (M1(c)), which is **below** the stamp.
`__commpage_gettimeofday_internal`'s unsigned `delta = now − TimeStamp_tick` therefore wraps huge.
Its `/* If more than one second force a syscall */ if (delta >= Ticks_per_sec) return 1;` then
always returns 1, and every `mach_get_times` takes the 116 fallback.

`clock bad` follows from the numbers: `before ≤ abs` holds, but `abs = 0x6994d4ed927` (host)
`≤ after ≈ 0x67ed…` (synthetic) does not.

**The default mode.** The record exits 101 at M45's refusal:
`M45: unmeasured kevent_qos shape: x3 (eventlist) is 0x27fea48, measured 0x0. … args=[0xffffffff,0x27fef28,0x1,0x27fea48,0x10,0x0,0x0,0x23]`
(`m1a-after.err`).
- All 16 of its `num=116` traps have a zero third argument. No fallback runs before M45's wall.
- `m1a-after.out` is empty, although the guest wrote `fired cell …` (`write_nocancel(1, …, 0x17)`).
  The CLI prints guest stdout only after the record ends (`crates/retrace/src/main.rs:20,63`).

**Decision.** `clock bad` together with a 116 carrying a nonzero third argument confirms R7's
premise (spec §11 item 1). Task 1 keeps `synthesize_mach_time_out` and its record-arm block.

**Halt considered.** None. A confirmed premise is the case R7 exists for.

### M1(b) — which clock the timer arm reads, natively

**Command.** Step 4's breakpoints, with three changes. Each is forced by a measured defect in the
brief's script (see "Differences from the brief").
1. The commands are attached as `breakpoint set … -C "bt 12" -C "continue"`. `breakpoint command
   add -o A -o B` keeps only the last `-o`.
2. `mach_absolute_time` is broken at libdispatch's 15 `bl mach_absolute_time` call sites, not at
   its entry. A breakpoint at the entry livelocks the guest.
3. The frame grep also matches `_dispatch_event_loop_drain_timers`, because
   `_dispatch_timers_program`, `_run` and `_get_delay` are inlined into it and have no symbols.

The script is `m1b.lldb`, run by `t0-step4-m1b.sh` on `after_dyn`, `after_dyn two` and `timer_dyn`.

**Static result** (`m1b-disasm-timers.log`, `m1b-libdispatch-clock-callsites.txt`). The timer
functions call exactly one clock, `mach_get_times`:

```
libdispatch.dylib`_dispatch_event_loop_drain_timers: :: [0x180357260] <+524>: bl 0x180372d34 ; symbol stub for: mach_get_times
libdispatch.dylib`_dispatch_event_loop_drain_timers: :: [0x1803575c0] <+1388>: bl 0x180372d34 ; symbol stub for: mach_get_times
libdispatch.dylib`_dispatch_event_loop_timer_arm: :: [0x18035a920] <+184>: bl 0x180372d34 ; symbol stub for: mach_get_times
```

None of them calls `mach_absolute_time`, `mach_approximate_time` or `mach_continuous_time`.

**Dynamic result** (`m1b.log`, `m1b-after-two.log`, `m1b-timer.log`; each exited 0 with its
native markers). The hits at frame 0:

| run | `mach_get_times` | `dispatch_time + 80` | `_dispatch_timeout + 64` | `gettimeofday` |
|---|---|---|---|---|
| `after` default | 2 | 1 | 1 | 19 |
| `after two` | 3 | 2 | 2 | 18 |
| `timer` | 4 | 1 | 0 | 17 |

- Every `mach_get_times` hit has this backtrace:

  ```
  thread #2, queue = 'com.apple.libdispatch-manager', stop reason = breakpoint 1.1
    frame #0: libsystem_kernel.dylib`mach_get_times
    frame #1: libdispatch.dylib`_dispatch_event_loop_drain_timers + 528
    frame #2: libdispatch.dylib`_dispatch_kevent_worker_thread + 444
    frame #3: libsystem_pthread.dylib`_pthread_wqthread + 348
  ```

- `dispatch_time + 80` (`← main`) and `_dispatch_timeout + 64` (`← dispatch_after ← main`) are
  direct `mach_absolute_time` reads on the main thread. They are the deadline side, and under
  retrace they read the synthetic timebase.
- Every `gettimeofday` comes from `_mach_boottime_usec + 64` on the main thread.
- `mach_approximate_time`, `mach_continuous_time` and `mach_continuous_approximate_time` were hit
  **0** times, in every mode.

**Decision.** The UPTIME timer's "now" is `mach_get_times`, as sourced (libdispatch
`src/shims/time.h:220-235`). It is the channel R7 closes.

**Halt considered.** Halt 7, for approximate or continuous time on the UPTIME timer path. Not
triggered.

### M1(c) — the commpage timebase words

**Command.** Step 5, verbatim. The address `x 0xfffffc080 0x60` was accepted.

**The disassembly** (`m1c-disasm.log`, `libsystem_kernel`mach_absolute_time`):

```
movk x3, #0x0, lsl #48 ; movk x3, #0xf, lsl #32 ; movk x3, #0xffff, lsl #16 ; movk x3, #0xc088
ldrb w2, [x3, #0x8]
cmp x2, #0x0 ; b.eq mach_absolute_time_kernel
cmp x2, #0x2 ; b.eq <+76>      ; <+80>:  mrs x0, CNTVCTSS_EL0
cmp x2, #0x3 ; b.eq <+104>     ; <+108>: mrs x0, S3_4_C15_C10_6
isb ; ...                      ; <+52>:  mrs x0, CNTVCT_EL0
ldr x1, [x3] ; mrs ... ; ldr x2, [x3] ; cmp x1, x2 ; b.ne <retry> ; add x0, x0, x1 ; ret
```

**The commpage bytes** (`m1c-commpage.out`):

```
0xfffffc080: 3a b9 d5 17 00 00 00 00 56 dc 16 d7 7d 06 00 00 03 01 01 00 00 00 00 00 71 2e 0d 17 ae 04 00 00 …
```

| field | expected | measured |
|---|---|---|
| offset `mach_absolute_time` adds | `0x88` (`_COMM_PAGE_TIMEBASE_OFFSET`) | `0x88`; value `56 dc 16 d7 7d 06 00 00` = **`0x0000_067d_d716_dc56`** |
| user-timebase byte | `0x90` | `0x90`; value **`0x03`**, which selects **`S3_4_C15_C10_6`** |
| `+0x98` | — | `0x4ae170d2e71`, the continuous-time base `_mach_continuous_time_base` reads. It is off the timer path |

- A value of 3 means `mach_absolute_time` reads the Apple counter, which `try_emulate_timebase`
  handles.
  - A value of 0 would send it to the `-3` trap, a host clock (H1).
  - A value of 2 would mean `CNTVCTSS_EL0`, which is not emulated (Halt 7's class).
- The byte is frozen from the host at load, so it is this host's value.

**The guest's `mach_absolute_time` at start.** By the plan's formula it is `0x1_0000_0000 +
0x67dd716dc56` = **`0x67e_d716_dc56`**. The first emulated read returns `SYNTH_TSC_START +
SYNTH_TSC_STRIDE`, so the first value the guest sees is `0x67e_d717_0056`. Both are far below
`0x4000_0000_0000_0000`.

**Decision.**
- Task 1's `COMMPAGE_TIMEBASE_OFFSET_IPA` is `COMMPAGE_IPA + 0x88` = `0xF_FFFF_C088`.
- `now_guest()` is `synthetic_tsc` plus the 8 bytes read there (wrapping).
- The offset is this host's, frozen into the page and captured in the snapshot, so it is read, not
  hard-coded.

**Halt considered.** Halt 7, for a start at or above 2^62, or `CNTVCTSS_EL0`. Not triggered.

**R7's premise confirmed.**

---

## M2 — the manager's entry, natively

**Command.** Step 6's loop, with `m2.lldb` in the `-C` form:
`breakpoint set -n start_wqthread -C "register read x0 x1 x2 x3 x4 x5 sp" -C "memory read -s8 -fx -c27 $x0-0x480" -C "continue"`.
The breakpoint resolves at `start_wqthread` +0. It was run by `t0-step6-m2.sh`: `after '' exit=0`,
`after 'two' exit=0`, `timer exit=0`.

**Result.** Every manager stop (`x4 & 0x100000`) has:
- `x0 = 0x16fe87000`, the pthread `self`;
- `x3 = 0x16fe86b80 = x0 − 0x480`;
- `sp = x3`;
- `x5 = 1`;
- `x2 = 0x16fe04000`, the stack bottom;
- `x1` = the kport (`0xc03` or `0x1c03`).

| flags word | where | decoded | class |
|---|---|---|---|
| **`0x3C4008`** | the first manager entry of every run | TSD_BASE_SET\|EVENT_MANAGER\|KEVENT\|NEWSPI\|PRIO_QOS\|8 | first use, **observed** |
| **`0x1E4008`** | after every fire (1× default, 2× `two`, 4× `timer`) | EVENT_MANAGER\|KEVENT\|NEWSPI\|REUSE\|PRIO_QOS\|8 | reuse, observed |
| **`0x1E0000`** | **not observed** | — | redelivery. It stays inferred: no native `0x40` found events waiting |
| `0x64005` / `0x64004` | the handler's worker (after: QoS 5; timer: QoS 4) | NEWSPI\|REUSE\|PRIO_QOS\|qos | not a manager. `x3 = 0`, `x5 = 0`, `sp = x0` |

`0x3C4008` is observed rather than inferred because natively the manager was the process's first
workqueue thread.

**The delivered events** (`memory read` at `x0 − 0x480`):
- **The first-use manager gets one `USER` event.** It matches §2c byte for byte:

  ```
  0x16fe86b80: 0x0000000000000001 0x020000000021fff6
  0x16fe86b90: 0xfffffffffffffff8 0x0000000000000000   (rest 0)
  ```

- **Each reuse manager gets one timer event.** It matches §2c, including the inferred flags
  `0x35`:

  ```
  0x16fe86b80: 0xffffffffffffff00 0x020000000035fff9
  0x16fe86b90: 0x00000001edfff970 0x0000000000000000
  0x16fe86ba0: 0x0000000000000001 0x0000000000000000
  0x16fe86bb0: 0x000000000001d4bf 0x0000000000000000
  ```

  That is ident `…ff00` (tidx 0), filter −7, flags `0x35`, qos `0x02000000`, udata
  `_dispatch_timers_heap`, fflags and xflags 0, data **1**, ext[0] 0, and **ext[1] = the leeway**.
  The leeway is `0x1d4bf` for 100 ms, `0x3a97f` for 200 ms, and `0` in `timer_dyn` (leeway 0).
- **`timer`'s last manager entry is a `USER` event** (`0x1E4008`), which the cancel's poke raised.

**Also observed.**
- **One native thread plays every role in these runs.** The same `self` is the first-use manager,
  the reused manager and the handler's plain worker. The box spawns fresh plain workers (M18).
  Plain-worker reuse is outside M46 (§7).
- **The worker's view of `x0 − 0x480` shows the manager's outgoing change list, written in place**
  (q:6847). For example, `two` shows `…ff00, 0x020000000015fff9, heap, 0x118, 0x6a0f7ac09ba, 0,
  0x3a97f`.

**Decision.** The design can express every register and event.
- `WQ_ENTRY_FLAGS_MANAGER_FIRST = 0x3C4008` (t0 M2, observed).
- `…_REUSE = 0x1E4008` (t0 M2, observed).
- `…_REDELIVER = 0x1E0000` (inferred, not observed).
- `sp = x3 = self − 0x480`, and `x5 = n`.
- The two event layouts are exactly §2c's.

**Halt considered.** H2. Not triggered.

---

## M3 — the native call sequence, per fixture mode

**Command.** Step 7's loop, with `m3.lldb` in the `-C` form (`t0-step7-m3.sh`): `after '' exit=0`,
`after 'two' exit=0`, `after 'wall' exit=0`, `timer exit=0`, each with its native markers.
- The ternary address `'$x1 ? $x1 : $sp'` is accepted. It was tested first, because an lldb error
  aborts a `-b` script.
- A `kevent_id` sentinel breakpoint was never hit.
- `m3parse.py` decodes each log into `m3-calls.txt`.
- `m3-origin.lldb` (`t0-m3-origin.sh`) then took backtraces of the extra registrations
  (`m3-origin-{default,wall}.log`).

**The default mode, in order** (`m3-calls.txt`):

| # | call | thread | detail |
|---|---|---|---|
| 1 | `workq_kernreturn 0x400` | main | `x2 = 0x18` |
| 2 | `workq_kernreturn 0x80` | main | `x2 = 0x20ff` |
| 3 | `kevent_qos x7 = 0x21` | main | init: ident 1, −10, flags `0x21`, qos `0x02000000`, udata `~7` |
| 4 | `kevent_qos x7 = 0x23` | main | ident 0, −14, flags `0x185`, qos `0x02000000`, fflags `0xf0000037` |
| 5 | `kevent_qos x7 = 0x23` | main | ident = a port (`0x1e03`), **−8 `EVFILT_MACHPORT`**, flags `0x0385`, qos `0x840008ff`, udata heap, fflags `0x0700080e` |
| 6 | `kevent_qos x7 = 0x23` | main | **poke**: ident 1, −10, flags 0, **qos `0x00000000`**, udata `~7`, fflags `0x01000000` |
| 7 | `workq_kernreturn 0x40` | manager | n = 1: timer ADD, ident `…ff00`, −7, flags `0x15`, fflags `0x118`, data = deadline (nonzero), ext[1] `0x1d4bf` (nonzero) |
| 8 | `workq_kernreturn 0x20` | manager | `x2 = 1`, `x3 = 0x10ff`: the handler's worker request, after the fire |
| 9 | `workq_kernreturn 0x40` | manager | n = 0 |

**`two`.** Steps 1–6 as above (port `0x1c03`), then:
- `0x40` (n = 1: A, `…ff00`, `0x118`, ext[1] `0x1d4bf`);
- `0x20`;
- `0x40` (n = 1: B, `…ff00`, `0x118`, ext[1] `0x3a97f`);
- `0x4` (thread 3);
- `0x20` (thread 3);
- `0x40` (thread 3, n = 0);
- `0x4` (thread 2).

B's arm after A's fire travels through `KEVENT_RETURN`, as §3f's `two` assumes.

**`timer`.** Steps 1–6 (port `0x1d03`), then three rounds of `0x20` (`x3 = 0x40008ff`) followed by
a `0x40` (n = 1: `…ff00`, `0x118`, data = deadline, **ext[1] = 0**), with `0x4`s between the
rounds. Natively, the manager alternates between threads 2 and 3.

**`wall`.** Steps 1–6 (port `0x1e03`), then:
- 7. `kevent_qos x7 = 0x23` **from the manager**: ident `0x1b03`, **−8**, flags `0x0385`, qos
  `0x02000000`, fflags `0x0700000e`;
- 8. `0x40` (n = 1): ident **`0xffffffffffffff06`**, −7, flags `0x15`, **fflags `0x9c`**, data
  `0x18d9f0de8dcc4030` (wall ns), ext[1] `0x4c4b40` (5 ms, in ns);
- 9. `0x20`;
- 10. `0x40` (n = 0);
- 11. `0x4`.

**Origins** (`m3-origin-*.log`):
- **Call 5, in every mode.**
  - `_voucher_activity_debug_channel_init + 72` calls `task_get_special_port(0x203, which = 0xa)`,
    which is `TASK_DEBUG_CONTROL_PORT`. `task_get_debug_control_port` is that call as a macro (SDK
    `mach/task_special_ports.h:119`).
  - Then `_voucher_activity_debug_channel_init + 144 → _dispatch_lane_resume_activate →
    _dispatch_mach_activate → _dispatch_kq_unote_update → _dispatch_kq_poll + 164 → kevent_qos`.
  - Calls 3–6 all run inside one `_dispatch_kq_drain` on main. It is entered from
    `main → _dispatch_mgr_queue_push → _dispatch_event_loop_poke`, which the first `dispatch_after`
    triggers.
- **`wall`'s call 7.**
  - `_dispatch_event_loop_drain_timers + 1348 → _dispatch_event_loop_timer_arm + 220 → .cold.1 →
    _dispatch_mach_host_notify_update → _dispatch_mach_notify_port_init → kevent_qos`.
  - It is followed on the manager by `host_request_notification(host 0x1d03, 1 =
    HOST_NOTIFY_CALENDAR_CHANGE, port 0xd03)`.
  - Both come **before** the `0x40` that carries the `0x9c` timer.

**The values the plan asked for:**

| item | measured | the plan's value |
|---|---|---|
| the poke's `qos` (bytes 12–15 of the `EVFILT_USER` entry) | **`0x00000000`**, in every mode | Task 2's `MANAGER_POKE.qos = 0`: confirmed |
| the `wall` timer's `fflags` | **`0x9c`** | pinned at box level (Ruling T0-b) |
| timer ident ↔ fflags | `0xffffffffffffff00` (tidx 0) ↔ `0x118`, in every UPTIME mode; `0xffffffffffffff06` (tidx 6 = WALL×3 + 0) ↔ `0x9c` | Task 2's `UPTIME_TIMER_FFLAGS` with `0x118` at tidx 0: confirmed |
| `0x40` calls, default mode | **2** (one with n = 1, one with n = 0) | Task 4 test 1's bound of 8: holds |
| `0x40` calls, the other modes | `two` **3**, `wall` **2**, `timer` **3** | — |
| an immediate timer registration (`0x23`, filter −7) | **none**, in any mode | Halt 6: not triggered |
| a third registration (`0x23`, filter neither −14 nor −10) | **yes**: call 5 (every mode) and `wall`'s call 7 | H3: triggered and ruled, below |

**H3: triggered and ruled (spec §11 items 11–12).**
- **Under retrace**, the path to call 5 goes through `task_get_special_port(which = 10)`
  (`mach_msg2`, msgh_id 3409). retrace's 3409 arm modeled only `which == 4`.
- **libdispatch** crashes on a nonzero `kr` (`voucher.c:842`). It connects the debug channel only
  `if (dbgp)` (`voucher.c:844`).
- **Ruling T0-a.** 3409 with `which == 10` answers `KERN_SUCCESS` with `MACH_PORT_NULL`. Task 4
  implements it.
  - retrace keeps no debug control port: its 3410 arm drops the port libtrace sets (M2-setport).
  - Call 5's `EVFILT_MACHPORT` registration therefore never happens under retrace, and H3's
    register-only branch is not needed.
  - The reply is deterministic, so replay recomputes and byte-compares it.
  - A kept or minted port with a register-only knote stays owed.
- **Ruling T0-b.** Task 5 test 4 expects the refusal of `wall`'s call 7 by its filter
  (`changelist[0].filter is 0xfff8`), with no timer armed. `0x9c` stays pinned at box level
  (`kqmanager.rs`) and in `gcdshapes.rs`.

**The M45 recording.** `docs/sweep-evidence/2026-09-28-m45/automationmodetool.rec.err` has 363
`[trap]` lines. The last is the refused
`num=374 … args=[0xffffffff,0x27ff298,0x1,0x27fedb8,0x10,0x0,0x0,0x23]`, and the panic is at line
552. No call follows landmark 363. Its only special-port RPCs are the earlier `msgh_id=3409`
(libxpc's `which = 4`) and `3410` (libtrace's set). `m1a-after.err` is the same. So
`task_get_debug_control_port` is reached only past the model, and Ruling T0-a is what meets it.

---

## M4 — the base `#[test]` count

**Command.** `grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'`

**Result.** **`853`** (`m4-count.out`).

**Decision.** This equals the expected figure: M45's 846 + 9 = 855, which is 853 plus the two
`census.rs` tests compiled twice. No reconciliation is needed.

**Halt considered.** None.

---

## Differences from the brief

1. **H3 fired.** It was ruled as T0-a and T0-b (M3).
2. **`breakpoint command add -o A -o B` keeps only the last `-o`** (lldb-2100). As written, the
   brief's M1(b), M2 and M3 scripts run only `continue` and print no backtrace, registers or memory
   (`m1b-attempt1-head.log`).
   - Every script here uses `breakpoint set … -C "<cmd>" -C "<cmd>"`, which is repeatable and runs
     in order.
   - Later tasks that copy those scripts must do the same.
3. **A breakpoint on `mach_absolute_time` livelocks the guest.**
   - Every stop lands inside `__commpage_gettimeofday_internal`'s seqlock retry loop: `bl
     mach_absolute_time` at `+40`, and the `TimeStamp_tick` re-check at `+60`. The stop outlasts
     the kernel's stamp-update interval, so the loop never exits.
   - Measured: 21,637 consecutive hits in 90 s at `__commpage_gettimeofday_internal + 44`
     (`m1b-attempt2-head.log`). A conditional breakpoint livelocked too (`m1b-attempt3.log`).
   - M1(b) breaks at libdispatch's 15 `bl mach_absolute_time` sites instead, found by a census of
     libdispatch's whole `__text`.
   - Those need `-s libdispatch.dylib` so that the file address slides (`m1b-attempt4.log`).
4. **`_dispatch_timers_program`, `_dispatch_timers_run` and `_dispatch_timers_get_delay` are
   inlined.** The frame to match is `_dispatch_event_loop_drain_timers` (and
   `_dispatch_event_loop_timer_arm`).
5. **`0x1E0000` was never observed.** It stays inferred. `0x3C4008` was observed.
6. **`wall` makes a calendar-change registration and a `host_request_notification` before its
   `0x40`.** This is what Ruling T0-b rests on.
7. **The native sequence has `workq_kernreturn(0x80, 0, 0x20ff, 0)` between `0x400` and the kq
   init.** That is `WQOPS_SET_EVENT_MANAGER_PRIORITY`. retrace's trace shows `368(0x400) → 367 →
   374` with no `0x80` (`m1a-after.err`). It was not investigated, and is recorded for whoever next
   touches the workq opcode table.
8. **After a fire, the manager issues `workq_kernreturn(0x20, 0, 1, prio)`** for the handler's
   worker, before its `0x40` (default #8; timer, before every `0x40`). M18 admits `0x20`, but
   Task 4 meets it from the manager thread.
9. **A record that panics prints no guest stdout** (`m1a-after.out` is empty). `fired cell …` is
   visible only on records that finish.
