# M43-lldb: lldb in front of the debugger

**Date:** 2026-09-25. **Branch:** `worktree-m43-lldb` from `main` at the M42 merge (`c652cf1`).
**Companion:** `2026-09-25-retrace-m43-lldb-measurements.md` (t0: L1–L10 on lldb's side, R1–R4 on
retrace's). §2 cites it by section. A claim from reading code says so. Where a claim is inferred and
not measured, the spec names the task measurement that owes it.
**Approach:** an RSP server over the existing `Exec` (R1), chosen by the controller under the
operator's autonomous-run authorisation of 2026-09-24 (M41 → M42 → M43).

## 1. Purpose

Today retrace's debugger is a script: `retrace debug <trace> --script 'break X; continue; …'`.
The original design (2026-07-05, its M4) promised a debugger people already know: "a gdb/lldb-remote
server … *Exit:* reverse-step through a real crash in LLDB." M41 made the debugger's answers right
(one hit order, one cursor, a hit oracle) and M42 made stepping sound across exclusive pairs, both
explicitly as this milestone's preconditions.

M43 adds `retrace gdbserver <trace>`, a gdb-remote (RSP) server over one recording. The host's
`lldb` (lldb-2100, Xcode) connects with `gdb-remote`, and debugs the recording forward and
backward:
- `process continue`, `thread step-inst` and friends go forward;
- `process continue -R` goes backward (stock lldb, the `bc` packet);
- a reverse single step is `rsi`, a command retrace ships as a small lldb Python script.

The server is a **translation layer**, not a second debugger. Every motion is one of `Exec`'s
(`crates/retrace/src/debug.rs`), so M41's hit order and M42's pair handling hold under lldb by
construction. What the server adds is the mapping between `Exec`'s cursor and the positions lldb
believes it stands on (§3c), a stepping rule lldb can live with (§3d), and replies that never cost
lldb its connection (§3b).

It also pays four debts that sit directly in the seam's path:
- **M41:** early `?` exits in `cmd_continue` leave a kept session armed. That was latent because an
  `Err` aborts a script. The server keeps its `Exec` after an error, so it is live now (§3b).
- **M42, Ruling F-3:** three `Box_` panics that stepping can reach (§3i). lldb steps far more than a
  script does, and a panic kills the server.
- **M42, Ruling F-5:** a stale comment in `run()`'s prologue that repeats F2's overclaim (§3i).

## 2. What was measured before this spec was written

The companion is the record. In summary, lldb-2100 against a throwaway Python stub, plus reading
retrace's code on `c652cf1`:

| # | Finding | Consequence here |
|---|---|---|
| L1 | Required: `qSupported`, `?`, one register route (`qXfer:features:read:target.xml`), `p`/`g`, `m`. Everything else may be empty. A GPR-only set works except for naming FP registers. | §3g: target.xml with GPRs + FP. |
| L2 | The new macOS loader (chosen by `qHostInfo` `os_version`) symbolicates the exe from `jGetLoadedDynamicLibrariesInfos` with **no** `target create`. With `os_version` and no answer, lldb **unloads** the exe. Listing dyld plants a persistent internal `Z0`. | §3g: answer it, exe only, when the path is known. |
| L3 | lldb never sends `bs`. Only `process continue -R` reaches the server, as `bc`. The direction **sticks** for `continue` until `-F`. A server-armed `bc` answered `reason:trace` is a working reverse step (`l3_rsi`). | §3e: `rsi` = `qRcmd arm-rsi` + `c -R`. |
| L4a/b | `reason:breakpoint` and `reason:trace` display correctly both ways. A `trace` reply that does not move the stepped thread's pc makes lldb re-step **forever**. | §3d. |
| L4c | With the default timing, lldb forward-steps after **every** watch stop, reverse included, so a user can never get backward past the most recent write. With `watchpoint_exceptions_received:after`, lldb never steps; hits reported post-retire forward and pre-retire backward give correct old/new values. | §3c's mapping. |
| L4d/e/f | A plain signal stop makes `c -R` fail ("can't deliver signals while running in reverse"). Mach keys (`metype:1`) and `reason:exception;description:` stops are reversible. `W`/`X` end the session. `replaylog:end` / `replaylog:begin` with a `description:` are reversible "history boundary" stops. | §3c: never `W`, never a plain signal. |
| L5 | `Z0` is used for `breakpoint set`. `Z0` answered `E` has no fallback. Every step-over/out/in and `ni` over a call inserts **one transient** `Z0`; refusing it makes lldb run away to the end. lldb enforces no count. | §3f: cap at the hardware's 6 (R4, amended): the transient cannot be told from a user's `Z0`. |
| L6 | `Z2` per watch, aligned pieces of ≤ 8 bytes; `Z3`/`Z4` for read/access; an `E` reply is a clean creation error. No count enforced. | §3f. |
| L7 | tid 0 is unusable. A step request for thread A answered by a stop on B with A's pc unchanged loops forever. `E` to any resume packet (`c`, `s`, `bc`) disconnects lldb ("lost connection"). A non-moving `reason:exception` stop is safe. | §3b, §3d. |
| L8 | No memory or register writes in ordinary operation. Expression evaluation tries an inferior `mmap` call (`P` writes, then `c`); refusing `P`/`G` is measured safe: no resume follows, and constant expressions still evaluate. Reads come in 0x200-byte lines that may straddle a mapping's end. | §3g, §3h. |
| L9 | `0x03` interrupt arrives during a `c` or `bc`. Resume packets wait indefinitely; other packets time out at 5 s. | §7: interrupt is not in scope. |
| L10 | `lldb -x -b -s <file> </dev/null` is synchronous and deterministic once port, paths and pid are normalised. `-o` silently stops after a crash or boundary stop and exits 0. | §4: the lldb tests use `-s` and a sentinel. |
| R1 | `ReplaySession` exposes registers only as text. `Box_::save_ctx()` and `threads().ctx_of(t)` hold the structured `ThreadCtx`, FP included, for every thread. | §3g: `thread_ctx`. |
| R2 | Every `Err` in `cmd_reverse_continue` leaves `session: None`. `cmd_continue`'s `advance()` failures leave hardware armed **and** the session moved (debug.rs 732, 749, 763). | §3b: one uniform recovery. |
| R3 | The exe is at `0x1_0000_0000`, slide 0, for static and dynamic guests alike. Its path is **not** in the trace. For `record-dyn` guests `argv[0]` in the opening snapshot's stack holds the path as typed. | §3g: `--exe`, else `argv[0]`. |
| R4 | `Exec` reports outcomes only as printed text, and is private to the binary. | §3b: a returned `Halt`. |

## 3. Design

### 3a. Shape

- **CLI:** `retrace gdbserver <trace> [--port <n>] [--exe <path>]`. It binds `127.0.0.1:<n>` (default
  0, an ephemeral port), prints exactly one line to stderr, `listening on 127.0.0.1:<port>`, and
  serves **one** connection. `k`, `D` or the peer closing ends the process with status 0. Nothing
  else the server prints depends on the host, the port aside.
- **Modules,** all in the `retrace` binary crate:
  - `rsp.rs`, pure: packet framing (`$…#cc`, ack and no-ack modes, `}` escaping), hex, the
    target.xml, stop-reply construction, and the image JSON (§3g). Unit-tested without a VM.
  - `gdbserver.rs`: the TCP loop and the packet dispatch, which owns one `Exec`.
  - `debug.rs`: `Exec` becomes `pub(crate)`, and its motions return a `Halt` (§3b).
- **The lldb script** `crates/retrace/lldb/retrace.py` defines `rsi` (§3e). Loaded with
  `command script import`.
- **Single-threaded.** The server blocks in each motion. No threads, no async; `clippy.toml`'s
  thread ban stands.

### 3b. `Exec` returns what happened, and survives its own errors

`Exec`'s four motions, `cmd_continue`, `cmd_reverse_continue`, `cmd_stepi` and
`cmd_reverse_stepi`, return `Result<Halt, String>` instead of `Result<(), String>`:

```rust
pub(crate) enum Halt {
    Break,                             // parked on the breakpoint, (n, k, Bp); pc() is its address
    Watch { watched: u64 },            // a store, parked pre-retire, (n, k, Watch)
    WatchStepped { watched: u64 },     // step_thread only (§3d): the store retired, (n, k+1, Bp)
    WatchSys { watched: u64, thread: u32 }, // a syscall's write, (n, 0, Sys)
    Terminal(ReplayReport),            // exit / crash / fatal signal, (T, K_f, Watch)
    NoEarlierHit,                      // reverse only; the cursor did not move
    Stepped,                           // a step (either direction) completed
    AtStart,                           // reverse-stepi stopped at (1, 0)
    Refused(String),                   // nothing moved: stepi's window end, or §3d's rule 1
}
```

They still print exactly what they print today. The script CLI ignores the `Halt`, so every
transcript is byte-identical (the existing debug tests are the guard). The server passes
`std::io::sink()` and uses the `Halt`. `Exec` also gains a read of its cursor, `(n, k, phase)`.

**Recovery (M41's owed item).** A new `Exec::recover(at)` re-seeks to a saved cursor and restores
its phase. `reseek` drops whatever session exists, armed or moved or none, and seeks a fresh,
breakpoint-clean one. So one call restores every state R2 lists. The server brackets each motion:

1. save the cursor;
2. run the motion;
3. on `Err(e)`, `recover(saved)` and reply the non-moving stop `T05…reason:exception;description:<e>`.

Never an `E` (L7). If `recover` itself fails, the server has no session left. It replies that
same exception stop to every later resume packet without moving, and `E01` to every register or
memory read, until lldb disconnects. The script CLI does not call `recover`: an `Err` still aborts
the script with exit 5, and a kept session never outlives it there.

### 3c. Positions: what lldb stands on

lldb believes the thread stands **between** instructions, and reads memory **now**. With
`watchpoint_exceptions_received:after` (L4c) it reports whatever pc the server gives, so the
server must give the right one:
- going **forward**, a hit is reported **after** the crossing;
- going **backward**, **before** it.

`Exec`'s cursor is a triple, and the server re-parks some of `Exec`'s stops so that the cursor it
keeps always matches the position it reported:

| `Exec` result | `Exec` parks at | the server re-parks at | reply pc | reply |
|---|---|---|---|---|
| forward breakpoint | `(n, k, Bp)` | — | the breakpoint | `reason:breakpoint` |
| forward store watch | `(n, k, Watch)` | steps it: `(n, k+1, Bp)` | past the store | `watch:<watched>` |
| forward syscall watch | `(n, 0, Sys)` | `(n, 0, Bp)` | `(n, 0)` | `watch:<watched>` on the writing thread |
| reverse breakpoint | `(n, k, Bp)` | — | the breakpoint | `reason:breakpoint` |
| reverse store watch | `(n, k, Watch)` | `(n, k, Bp)` | the store | `watch:<watched>` |
| reverse syscall watch | `(n, 0, Sys)` | `(n−1, len(n−1), Bp)` | the trap | `watch:<watched>` |
| no earlier hit | unchanged | `(1, 0, Bp)` | the first instruction | `replaylog:begin` |
| terminal | `(T, K_f, Watch)` | — | the terminal instruction | per §3c's terminal list |

Why each re-park is right, under M41's order `Sys < Bp < Watch`:

- **A forward store watch is stepped:** lldb must see the new value. From `(n, k+1, Bp)` a
  reverse `c -R` finds that same store (it is before the cursor) and parks before it, with the old
  value in memory. That is L4c's measured-correct `ideal` run.
  If the store does not retire when the server steps it (it faults: M41's owed "crashing watched
  store"), the reply is `reason:exception;description:<"the watched store at … did not retire: …">`
  at the store. The cursor stays `(n, k, Watch)`, so the next `c` crosses to the fault through
  `Exec`'s own finish.
- **A forward syscall watch becomes `(n, 0, Bp)`:** the forward store watch's rule, for the same
  reason. `Exec` parks it at `(n, 0, Sys)`, and a reverse `c -R` from there does not find the
  write, because it is not strictly before the cursor: lldb would run past the write it just
  reported. From `(n, 0, Bp)` the write at `(n, 0, Sys)` is behind the cursor. The cost: a
  breakpoint at the pc that `(n, 0)` resumes is then not reported by the next forward `c`, which
  is R4's rule for the pc you stand on. That pc belongs to the thread that runs next, which after
  a blocking syscall is not the writer that lldb shows (plan pre-flight, 2026-09-25).
- **A reverse store watch becomes `(n, k, Bp)`:** from there a forward `c` must report the same
  store again, because the thread is before it. `(n, k, Watch)` would step over it silently,
  `Exec`'s rule for a cursor ON a hit. A further `c -R` does not re-report it: `(n, k, Watch)` is
  after `(n, k, Bp)`. A breakpoint at the store's own pc is then reported in neither direction,
  which is M41's R4 (gdb's rule for the pc you stand on).
- **A reverse syscall watch is re-parked at the trap, `(n−1, len(n−1))`:** that is before the
  syscall wrote. Leaving it at `(n, 0)` would show lldb the *new* value as the stop's
  "new value", and a `modify` watchpoint would silently continue past it (L4c). From the trap, a
  forward `c` crosses the syscall and reports it again, and a further `c -R` does not.
  `n ≥ 2` always holds, because the event that ends window 1 is landmark 1, so its write lands at
  `(2, 0)`.
- **No earlier hit goes to the start:** lldb's reverse-continue semantics are gdb's. With nothing to
  stop at, it runs to the beginning of history and says so. The script CLI keeps its own rule (stay
  put, print `no earlier hit`, M41 Review Focus 5); only the server moves.

**Terminals** keep `Exec`'s park, `(T, K_f, Watch)`, which sits after every hit (M41 R18). The
reply depends on the outcome (L4d/e):
- **exit:** `T05…;replaylog:end;description:<"exited (code N)">;`.
- **crash from an abort** (EC `0x20`, `0x21`, `0x24` or `0x25`):
  `T0b…;metype:1;mecount:2;medata:<code>;medata:<far>;`, which lldb shows as `EXC_BAD_ACCESS`.
  `code` is `1` (`KERN_INVALID_ADDRESS`) for a translation fault (DFSC `0x04..=0x07`), `2`
  (`KERN_PROTECTION_FAILURE`) for a permission fault (`0x0c..=0x0f`), and `1` otherwise.
- **any other crash:** `T0b…;reason:exception;description:<the CLI's "guest crashed: …" line>;`.
- **fatal signal:** `T<sig>…;reason:exception;description:<"guest terminated by signal N">;`.

All four are reversible. `continue` from a terminal reports it again. `s` at a terminal replies the
terminal stop again, never `trace` (L4b). Neither needs server-side state. From `(T, K_f, Watch)` the
terminal instruction never retires, so both motions cross it with `advance()` and re-reach
`Exited` (M41 R18; §3d rule 3).

**Start of recording.** The answer to `?` at connect is `(1, 0)` with
`T05…;replaylog:begin;description:<"start of recording">;`. A reverse step at `(1, 0)` gets the same
stop.

Every stop reply carries:
- `thread:<t+1>`, where retrace's thread `t` maps to RSP thread `t+1` (tid 0 is unusable, L7);
- `threads:` and `thread-pcs:` for every thread that has not exited;
- expedited `fp`, `lr`, `sp`, `pc` (register numbers `1d`–`20`).

The reported thread is the current one, except for a syscall watch going forward, which names the
writing thread from `Halt::WatchSys`.

### 3d. Stepping: `s`, `vCont;s:<t>`

lldb's step must end with the stepped thread's pc moved, or with a non-`trace` reason (L4b, L7). It
also must not fail at a window end: `si` on an `svc` is ordinary. `cmd_stepi` errs there. So the
server steps with a new `Exec::step_thread(t) -> Result<Halt, String>`:

1. **Wrong thread:** `t` is not the current thread. Reply the non-moving
   `reason:exception;description:<"cannot step thread T: only the running thread (C) can step">`,
   L7's measured-safe form. A recording cannot run a thread the recording did not run.
2. **At a terminal:** the terminal again. This is rule 3's own crossing: the terminal instruction
   never retires, and `advance()` re-reaches `Exited`.
3. **Otherwise, one instruction with the watches armed** (`step_armed`, as `cmd_continue`'s
   finish does):
   - `Retired`: the cursor is `(n, k+1, Bp)`. Reply `trace`.
   - `Watch`: retire the store with the watches cleared. The cursor is `(n, k+1, Bp)`. Reply
     `watch:` post-retire, as §3c's forward row.
   - `AtTrap` or `Fault`: cross the event with `advance()`, watches armed for that one event (R10,
     the crossing `cmd_continue` already does). Then:
     - `Exited`: the terminal, parked as `park_at_terminal` does.
     - `WatchSyscall`: a forward syscall watch, §3c.
     - `Event`: if `t` is current at `(n+1, 0)`, reply `trace` on `t`. If `t`'s pc did not move,
       which happens only on a trap that returns to itself, reply
       `reason:exception;description:<"step did not move">` instead of looping lldb. If another
       thread is current, `t` blocked: **run until `t` is current again** (below).

**Running until `t` resumes** is `cmd_continue`'s scan with one extra stop condition,
`until_thread: Option<u32>`, checked first in its `Advance::Event` arm. That arm is where every
thread switch surfaces (M41 §3a settles the switch at the boundary). Arrival at `t` there is
checked **before** the boundary-breakpoint check, so a breakpoint at `t`'s resume pc is not
reported (M41 R4). The user's breakpoints and watchpoints stay armed during the run, so a hit by
another thread in the meantime is reported as that thread's own stop, all-stop semantics.
`cmd_continue` with `until_thread: None` is today's `continue` exactly, and the existing tests
pin that.

lldb's reaction to a step that ends on another thread's breakpoint is **unmeasured** (L7 measured
only a `trace` on the other thread, which loops). Task 5 measures it first on a threaded
fixture. If lldb loops, the fallback is Ruling R7's.

### 3e. Reverse: `bc`, `bs`, and `rsi`

- **`bc`** is `cmd_reverse_continue`, then §3c's re-parks. If the server is armed (next bullet),
  `bc` is a reverse single step instead, and the arming is spent.
- **`bs`** is `cmd_reverse_stepi(1)`: `trace` on the thread current at the new cursor, or the start
  stop at `(1, 0)`. lldb never sends it (L3), but it is the RSP-standard packet and the Rust client
  tests use it.
- **`qRcmd`** (`process plugin packet monitor …`) takes two commands; anything else replies `E01`:
  - `arm-rsi` replies `OK` and makes the next `bc` a reverse step.
  - `where` replies `O<hex "at (n, k) phase=<Sys|Bp|Watch> pc=<pc> thread=<t>\n">` then `OK`.
    It is the server's view of the cursor, for tests and for users.
- **`rsi`,** in `crates/retrace/lldb/retrace.py`, runs `process plugin packet monitor arm-rsi`, then
  `process continue -R`: exactly L3's measured `l3_rsi`. Because lldb's direction sticks (L3), a
  plain `continue` after `rsi` or `c -R` keeps going **backward** until `process continue -F`. That
  is lldb's design, not the server's, and the README says so where it teaches `rsi`.

A reverse step can cross a landmark backward onto another thread's trap, `(n−1, len(n−1))`. The
reply names the thread current there. A `bc` has no step plan in lldb, so a `trace` on a different
thread is displayed, not re-stepped. That is inferred from L3/L7 and measured by Task 5.

### 3f. Breakpoints and watchpoints

- **`Z0`/`Z1`** become `Exec` breakpoints (hardware, `DBGBVR`). The server caps them at **6**, the
  hardware's count (Ruling R4, amended at Task 3's review). lldb's transient step breakpoint (L5)
  is an ordinary `Z0` that the server cannot tell from a user's, so no cap can reserve it a slot: a
  cap of 5 only moved the runaway from 6 user breakpoints to 5. At 6 concurrent breakpoints a
  seventh `Z0`/`Z1` gets `E01`, which is L5's clean creation failure for a user breakpoint and
  L5's runaway for a transient one. A duplicate address is `OK` and idempotent. `z0`/`z1` delete it.
- **`Z2`** becomes an `Exec` watch (`DBGWVR`, write). The length must be 1, 2, 4 or 8, and the
  address a multiple of it: the CLI's own rule, which fits lldb's aligned pieces (L6). The limit is
  4, else `E01`. `z2` deletes it.
- **`Z3`/`Z4`** (read, access) get `E01`: retrace watches writes only. L6 measured that as a clean
  lldb error.

### 3g. Registers, memory, threads, identity, images

- **Registers** come from a new `ReplaySession::thread_ctx(t) -> Option<ThreadCtx>`, which delegates
  to a new `Box_::thread_ctx`. That returns the live `save_ctx()` for the current thread and the
  table's saved context for any other one: `dbg_regs_of`'s own split (R1). `ThreadCtx` is
  re-exported from `retrace-core`.
- **target.xml** is `aarch64-core` (x0–x28, fp, lr, sp, pc, cpsr as 32 bits) plus `aarch64-fpu`
  (v0–v31, fpsr, fpcr). `cpsr` is `regs.cpsr`, never `spsr`: a blocked thread's `spsr` is raw
  exception-entry state (R1, read from code). `p<n>[;thread:<t>;]` and `g` read it; `Hg`/`Hc` answer
  `OK`.
- **Memory:** `m<a>,<l>` returns the **readable prefix** from a new
  `ReplaySession::read_mem_prefix(va, len) -> Vec<u8>`. It walks page by page through the guest's
  own stage-1 translation (`va_to_ipa`), as `insn_at` does (read from code). It stops at the first
  page that does not translate or read. It replies `E08` only when zero bytes are readable. The
  CLI's `x` keeps its all-or-nothing `read_mem`, which treats the address as an IPA.
- **Threads:**
  - `qfThreadInfo` lists `t+1` for each thread that has not exited, then `qsThreadInfo` → `l`.
  - `qC` → `QC<current+1>`.
  - `qThreadStopInfo<t>` → `T00thread:<t>;…` for a thread that is not the reporter.
  - `QThreadSuffixSupported` and `QListThreadsInStopReply` → `OK`.
- **Identity:**
  - `qHostInfo`: `cputype:16777228;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;watchpoint_exceptions_received:after;`,
    plus `os_version:26.0.0;` **only when an exe path is known** (next bullet).
  - `qProcessInfo`: `pid:1;parent-pid:1;cputype:100000c;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;`.
  - Fixed values, so lldb's `Process 1` is stable (L10).
- **Images (L2):**
  - **Path:** `--exe <path>` if given, else `argv[0]` from the opening snapshot's stack when it is
    absolute (`record-dyn` only, R3), else none.
  - **With a path:** `jGetLoadedDynamicLibrariesInfos:` → `OK`, and `…{"fetch_all_solibs":true}` →
    a one-image JSON for the exe at `0x1_0000_0000`. It is built from the Mach-O header and load
    commands in the snapshot (magic, cputype, cpusubtype, filetype, flags, `LC_UUID`, segments), in
    L2's measured shape.
  - **Without a path:** no `os_version`, so lldb takes the old loader, and the user can still
    `target create <exe>` then `target modules load --file <exe> --slide 0` (L2, route (a)).
  - **dyld and the shared cache are never listed** (Ruling R5).

### 3h. Refused and unsupported

- **Refused with `E01`:** `P`, `G`, `QSaveRegisterState`, `QRestoreRegisterState`, `M`, `X`. A
  recording is read-only. Refusing `P` is also what stops expression evaluation from resuming the
  replay under invented registers (L8, measured).
- **Answered empty (unsupported):** `_M`, `_m`, `vFile:*`, `jThreadsInfo`,
  `jThreadExtendedInfo`, `qMemoryRegionInfo`, `qShlibInfoAddr`, `qXfer:libraries:read`,
  `jGetSharedCacheInfo`, and every packet not named in §3.
- **`0x03`** outside a motion is ignored, with no reply. Within one it cannot arrive, because the
  server is not reading (§7).
- **`vCont?`** → `vCont;c;C;s;S`.
  - `c`, `C<sig>`, `vCont;c` and `vCont;C<sig>` are forward continues. The signal is ignored,
    because a recording's signals are its own.
  - `s`, `S<sig>`, `vCont;s:<t>` and `vCont;S<sig>:<t>` are §3d. Without a thread operand the
    step is the current thread's.
- **Ack mode** until `QStartNoAckMode` (`OK`), no-ack after.

### 3i. The M42 hardening (Ruling F-3, F-5)

All three panics sit in `Box_`'s step path, and all three are reachable from an ordinary `si`:

1. **A base-aliasing load-exclusive panics at retire:** `ldxr x9, [x9]`, even with no store
   following. `set_excl_from_retire` reads the base **after** the load overwrote it, and asserts
   that no destination aliases it.
2. **A retire with ISS.ISV = 0 is not classified.** M42 keys the shadow on `SS_ISV_EX`, so a
   stepped load-exclusive reported with ISV 0 sets no shadow, and the pair is lost as before M42.
   M42 t0 measured ISV = 1 on every retire it saw, so this shape is unmeasured.
3. **A store-exclusive to a non-writable or unmapped target panics** in `emulate_stx`. Natively,
   with the monitor held, that store faults, so the recording holds a crash. `plan_stx` refuses the
   target, and `emulate_stx` turns the refusal into a panic.

**Fix for 1 and 2: decode before the step.** `step()` reads the word at pc before running it.
If it decodes (`decode_excl`) as a load-exclusive, it captures the base VA (`base_reg(rn) &
TAG_MASK`) then. At a clean retire, the shadow is set from the pre-decoded instruction and the
captured VA:
- with ISV = 1, ISS.EX must agree with the pre-decode, in both directions, or it panics naming the
  disagreement (the existing hardware-vs-decoder panic, now two-sided);
- with ISV = 0, the pre-decode decides.

The decision is a pure function, `excl::classify_retire(syndrome, predecoded) -> Option<…>`,
unit-tested on both ISV values. The assert on aliasing goes: the captured VA is the marked address.

**Fix for 3: do not emulate.** When `plan_stx` refuses because the target is unmapped or not
EL0-writable, `step()` drops the shadow and steps the store natively. Its other refusals (the
target changed since the load, a mismatched address, an aliased status register) stay panics:
those are M42's unmodelled shapes, and M42 made them loud on purpose. Whether the core raises the
fault for a store-exclusive **whose monitor is lost** is IMPLEMENTATION DEFINED in the
architecture, so Task 1 measures it first on a new fixture:
- **(a)** the core raises the permission or translation fault: the native step reproduces the
  recording's crash, and the fix is complete;
- **(b)** the store just fails with status 1: the guest runs on past the instruction where the
  recording crashed. At the next trap, replay's divergence oracle compares it against the recorded
  `Event::Crash` and fails. That is loud, it goes through the ordinary `Divergence` path the server
  already turns into an exception stop (§3b), and it adds no new `Stop` variant. The step test then
  asserts the divergence, naming (b), instead of the crash.

Which of (a) or (b) holds is Task 1's first measurement, recorded in the ledger either way.

**Measured at plan time (2026-09-25): (a).** The fixture (`llscedge.s`, as the plan gives it)
records to `guest crashed: pc=0x1000003a0 far=0x1000003b0 esr=0x9200004f` (EC `0x24`, WnR, DFSC
`0x0f`, a permission fault), exit 139. Unfixed, the debugger reproduces both M42 panics on it:
`stepi 3` from the start panics at `lib.rs:2916` ("its base is also a destination"), and stepping
the `stxr` from `(2, 3)` panics at `lib.rs:2976` ("is not EL0-writable"). A temporary patch of
fix 3 alone (reverted) made that step fail with `guest crashed at step 0/1: pc=0x1000003a0
far=0x1000003b0`, the recorded fault, and a `continue` from `(2, 3)` then reached
`guest crashed: … esr=0x9200004f` with no divergence. The core raises the fault for a
store-exclusive whose monitor is lost. The step test asserts the crash, and (b)'s branch is not
built.

**F-5:** the comment at `run()`'s prologue (retrace-box `lib.rs`, about lines 2700–2702) says "a
shadow that outlives it belongs to a load whose sequence a branch left". F1 (M42) measured a
straight-line sequence reaching the bound too. The comment is reworded to match M42's corrected
README text.

## 4. Guards: each asserts the difference it makes

Three test surfaces, each owning what only it can see:

- **Unit tests, in the binary** (`rsp.rs`, `debug.rs`, the `--bins` chunk):
  - packet framing round-trips, checksums, `}` escapes, ack and no-ack;
  - each `Halt` from a real `Exec` on `watchsweep`;
  - `recover` after a forced mid-scan `Err`: the session exists, is disarmed, and sits at the saved
    cursor. A tampered copy of a recording (one syscall argument altered, re-encoded with
    `retrace_trace::Writer`) makes `continue` diverge mid-scan with breakpoints armed, which is R2's
    row 763;
  - the image JSON from `crashy`'s snapshot.
- **`gdbserver_e2e`** (new; a Rust RSP client in `tests/util/rsp.rs` over TCP against the spawned,
  codesigned binary). Repo-owned, no lldb, so it guards the protocol on any machine:
  - the connect handshake;
  - `?` = the start stop;
  - `p`/`g` equal to `debug --script regs` at the same position;
  - `m` equal to `x`, with a partial read's prefix;
  - thread list and tids;
  - every row of §3c's table on `watchsweep` (store watch, both directions) and `crashy` (syscall
    watch on `&g.st`, the crash terminal, `EXC_BAD_ACCESS` keys);
  - §3d's step across an `svc`, and across a blocking syscall on `threadrust` (the stepped thread
    resumes; `t+1` numbering);
  - `bs` at `(1, 0)`;
  - the `Z0` cap at 6; `Z3` refused; `P`/`G`/`M` refused with no motion;
  - an `Err` becoming an exception stop with the next `?` at the saved cursor;
  - `k`/`D`.
- **`lldb_e2e`** (new; real lldb, `lldb -x -b -s <file> </dev/null`, with a `script print("END")`
  sentinel). It **skips loudly** when `/usr/bin/lldb` cannot run, like `jq_e2e`. The headline is
  on `crashy`, repo-owned:
  1. connect: `stop reason = start of recording`;
  2. `process continue`: `EXC_BAD_ACCESS`, with frame #0 in `crashy`main`;
  3. `watchpoint set expression -w write -s 8 -- <&g.ptr>`, then `process continue -R`:
     `stop reason = watchpoint 1` at the off-by-one store's pc, with lldb's old/new values being
     the garbage and `&g.buf[0]`;
  4. `rsi`: one instruction earlier, `stop reason = trace`;
  5. `process continue -F`: the watch post-retire;
  6. `END`.

  A second test runs the same shape on `cpython_crash` and skips loudly without Homebrew Python.
  That is the original design's demo. A third runs the `crashy` session twice and asserts the
  normalised transcripts are identical (L10).

**Controls (each run on the committed tree, then restored):**
- **C1:** delete §3c's forward store-watch step. Expected RED: the post-retire assertion in
  `gdbserver_e2e`, and the lldb test's `continue -F` row.
- **C2:** delete the reverse store-watch re-park to `Bp`. Expected RED: the "forward after reverse
  re-reports the store" row.
- **C3:** answer an `Err` with `E01` instead of an exception stop. Expected RED: the recovery row
  (the client sees `E01` on a resume).
- **C4:** delete `until_thread`'s arrival check. Expected RED: the blocking-step row, whose step
  then ends on another thread.
- **C5:** remove `watchpoint_exceptions_received:after`. Expected RED: the lldb reverse-watch row
  (lldb re-steps forward and reports the next instruction).
- **C6 (§3i):** undo the pre-decode. Expected RED: the aliasing-load step test panics.

## 5. Task order and why

The order puts the box hardening first, so every later task steps on a box that cannot panic
on §3i's shapes. The wire comes before motion, and motion before lldb.

1. **§3i, the M42 hardening:** the `llscedge.s` fixture (a base-aliasing `ldxr` window; a
   `ldxr`/`stxr` pair on a read-only `__TEXT` word, whose recording holds a crash), the (a)/(b)
   measurement, the pre-decode, `classify_retire` and its unit tests, and F-5. `llsc.s` and
   `llscbound.s` stay frozen.
2. **The wire: everything that does not move.** `rsp.rs` (framing, hex, target.xml, image JSON,
   the stop reply with only the start-of-recording kind), `gdbserver.rs` (the CLI, TCP, handshake,
   `?`, registers, memory, threads, identity, images, `qRcmd where`, refusals, `k`/`D`),
   `ReplaySession::thread_ctx` and `read_mem_prefix`, `tests/util/rsp.rs`, and `gdbserver_e2e`'s
   non-motion rows.
3. **Continue, both ways:** `Exec`'s `Halt` for all four motions and `recover` (§3b), `c`/`vCont;c`
   and an unarmed `bc`, §3c's re-parks, `Z*`/`z*`, terminals and error stops. Also the
   tampered-trace recovery unit test, `gdbserver_e2e`'s continue rows, and C1–C3.
4. **Stepping:** §3d's `step_thread` and `until_thread`, `s`/`vCont;s`, `bs`, `arm-rsi`, then
   `gdbserver_e2e`'s step rows and C4. Last, the measurement §3d owes: lldb's reaction to a step
   ending on another thread's breakpoint (R7).
5. **lldb:** `retrace.py`, `lldb_e2e` (crashy, cpython, determinism) and C5.
6. **Close:** the chunked gate, the audit, README ("What works today": a *Debugging with lldb*
   section with the exact commands; "Known limits": §7's list), `docs/status-log.md`, CLAUDE.md's
   gate list (`gdbserver_e2e`, `lldb_e2e`).

The split follows a toolchain fact. `clippy -D warnings` rejects a function, an enum variant or
even a variant's field that the non-test build never uses. So each task introduces only what its own
non-test code reads. Pure layers therefore cannot land ahead of the code that calls them.

## 6. Acceptance

- `lldb_e2e`'s crashy session is green on this machine, and its five rows assert what §4 says.
- `gdbserver_e2e` is green and covers every row of §3c's table and every bullet of §3d.
- C1–C6 each turned RED as predicted. A control that stays green is a finding, not a formality.
- Every transcript of the script debugger is byte-identical to `c652cf1`'s: no existing debug test
  changes an assertion.
- The chunked gate is green with 0 failures, `--bins` and `Doc-tests` included, and the count
  reconciles file by file against M42's 747/0/9 over 142.
- `TRACE_MAGIC` has not moved, and `crates/retrace-trace` has no diff.

## 7. Halt rules, and what this milestone deliberately does not do

**Halt** (the charter's list): a red gate that survives one fix round; a new `#[ignore]`; any
`TRACE_MAGIC` bump; a nondeterministic record/replay flake; a measured premise that makes every
path a guess; anything destructive or outside the repo. A measurement that contradicts this spec is
a ledger Ruling and a re-scope, not a halt.

**Not in M43** (each goes on the README's Known limits):
- **Interrupting a motion** (`0x03`, `process interrupt`, ^C). The server does not read the socket
  while it replays, so a long `c -R` (rung 8 takes ~40 s) runs to completion. lldb waits, because
  resume packets have no timeout (L9). Polling between landmarks would thread a callback through
  M41's scans; that is its own milestone.
- **Read and access watchpoints** (`Z3`/`Z4`): retrace watches writes.
- **Symbols for dyld and the shared cache.** Only the exe is listed (R5). Frames in `libsystem`
  show addresses.
- **Expression evaluation that runs code.** It is refused by construction (`P`/`G`). Constant
  expressions still evaluate (L8).
- **Stepping a thread that is not the running one** (§3d, rule 1).
- **More than 6 breakpoints or 4 watchpoints at once,** and a step-over/out/in (or `ni` over a
  call) while all 6 breakpoints are the user's: its transient breakpoint is refused and lldb runs
  on to the next stop (L5).
- **A reverse step in stock lldb.** It comes only from `rsi`, and lldb's sticky direction is
  documented, not fixed.
- **The CLI's `where` phase** (M41's owed item) stays owed. The server's `monitor where` prints the
  phase for its own cursor.
- **M41's and M42's other owed items**, which are not in the seam's path.

## 8. Rulings (made while writing this spec)

- **R1 — an RSP server over `Exec`,** over (b) a second debugger in the server, and over (c) a
  server that parses `Exec`'s printed text. (b) would fork M41's hit order: one definition is what
  M41 bought. (c) would make every transcript line a protocol. Cost if wrong: `Exec`'s signatures
  change (a returned `Halt`), which the script CLI ignores.
- **R2 — §3c's re-parks live in the server, not in `Exec`.** The script CLI's semantics (a watch
  park is ON the store; `no earlier hit` stays put) are M41's, tested, and user-visible. lldb's
  differ because lldb reads memory at the stop. Cost if wrong: two position rules to keep straight,
  mitigated by §3c's table being the single statement of the server's.
- **R3 — `watchpoint_exceptions_received:after`,** over the AArch64 default `before`. `before`
  makes lldb step forward after every reverse watch stop, which L4c measured as a trap: the user
  can never get past the most recent write. Cost if wrong: none measured; `after` is also what
  lldb uses on x86.
- **R4 — 6 breakpoints (amended; it was 5).** L5: lldb's step-over/out/in inserts one transient
  breakpoint and runs away if it is refused. The first version capped at 5 to keep a slot free,
  but Task 3's review showed the transient is an ordinary `Z0`: with 5 user breakpoints it was
  the sixth and was refused. A cap of 6 is safe for steps at up to 5 user breakpoints, where 5
  was safe only at up to 4, and it also admits a sixth user breakpoint. A software fallback
  (single-step pc matching past 6) was rejected: correct, but it turns `continue` into a
  single-step crawl with no bound. Cost if wrong: a user holding all 6 loses step-over.
- **R5 — list the exe only.** dyld costs a persistent internal breakpoint (L2), one of the 6, and
  `jGetSharedCacheInfo` was never sent by lldb (L2, UNMEASURED what triggers it). Cost if wrong:
  frames in dyld and libsystem are unnamed.
- **R6 — interrupt is out of scope** (§7). Cost if wrong: a user cannot abort a long reverse
  continue, and waits for it.
- **R7 — a step that blocks runs other threads with the user's hits armed** (§3d), and reports a
  hit on another thread as that thread's stop. This is all-stop semantics, and it never skips a hit
  silently. **Fallback,** if Task 5 measures lldb looping on it: run with nothing armed until `t`
  resumes, and document that hits by other threads during one blocked step are not reported. Cost
  if wrong: one of the two, chosen by measurement.
- **R8 — no `_M` side allocation.** Refusing `P` already stops expression evaluation from resuming
  (L8). A debugger-side allocation map would quiet lldb's refused-write storm, at the price of
  choosing a VA range the guest provably never maps. Cost if wrong: about 70 refused packets per
  expression, invisible to the user unless Task 6 measures an error line, in which case it is
  added.
- **R9 — the M42 hardening rides in M43** (charter; Ruling F-3). It is in the seam's path: lldb
  steps freely, and a panic kills the server. Cost if wrong: one extra task.
- **R10 — pid 1, `os_version:26.0.0`, fixed.** A constant keeps lldb's transcripts stable (L10), and
  `os_version` only selects the loader (L2). Cost if wrong: none measured.

## 9. Gate prediction

M42 closed at **747 / 0 / 9 over 142**. M43 adds two test binaries (`gdbserver_e2e`, `lldb_e2e`) →
**144**. By task:
- T1 ≈ +5 (`excl.rs` units, plus `llsc_e2e` rows for `llscedge`);
- T2 ≈ +15 (`rsp.rs` units in `--bins`, plus `gdbserver_e2e`'s non-motion rows);
- T3 ≈ +13 (`debug.rs` units, plus continue rows);
- T4 ≈ +8 (step rows);
- T5 ≈ +3 (`lldb_e2e`).

Total **≈ 791 / 0 / 9 over 144**, measured at the close and reconciled file by file. No new `#[ignore]`: the lldb and CPython
tests skip loudly at run time and are counted as passes, as `jq_e2e` is. The plan fixes the exact
per-task counts.

## 10. Outcome

*(Filled at the close.)*
