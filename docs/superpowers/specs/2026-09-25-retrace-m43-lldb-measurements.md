# M43-lldb t0 measurements: lldb-2100's gdb-remote client

**Companion to** `2026-09-25-retrace-m43-lldb-design.md`. This is the t0 record, committed verbatim
below the rule. It cites a probe stub, command files and per-run logs (`$S/…`, `runs/<name>/`) that
lived in the session scratchpad and are **not committed**. Each claim quotes the evidence it rests
on.

---


Measured 2026-09-25 on this machine: `lldb-2100.0.17.203` (Xcode, `/usr/bin/lldb`), macOS 26.5 (Darwin 25.5.0).
Repo read at `worktree-m43-lldb` @ `c652cf1`. Nothing in the repo was modified.

## Method and where the evidence lives

`$S` = `/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/7b6f2ab3-f34f-4380-be19-9af66e2826a4/scratchpad/m43/t0`.

- `$S/stub.py` is a throwaway RSP server. It logs every packet it receives (`<-`) and every reply it sends (`->`)
  to `runs/<name>/stub.log`, and answers from a config file (`$S/cfg/*.py`). The fake process is
  `$S/exe/prog` (built by `$S/exe/build.sh` from `prog.c`, arm64, `-g -O1`). Its memory is the file's own
  segments at their unslid vmaddrs, plus a stack at `0x16fdf0000`. Thread 1 starts parked in `bump` at
  `0x1000003d4`, with `lr = 0x10000041c` inside `main`.
- `$S/run.py <name> <cfg|-> <cmds> [--o]` starts the stub, writes `runs/<name>/cmds.txt`, and runs
  `lldb -b -s runs/<name>/cmds.txt </dev/null` (or `-o` per line with `--o`). The command file always starts with
  `log enable -f runs/<name>/pk.log gdb-remote packets`. The run is killed at 60 s. It saves `out.txt`,
  `err.txt`, `rc.txt` (lldb's exit code and wall time), `pk.log` (lldb's own packet log) and `stub.log`.
- A `MARK x` line in a command file becomes `process plugin packet send qMark:x`, so each lldb command is
  delimited in both packet logs.
- Helper scripts: `show.sh`, `showc.sh`, `showm.sh`, the `*batch.sh` matrices, `stab.sh`, `rsi.py`, `intr.py`, `macho.py`.
- Two runs spun at ~5,000 packets/s until killed (`l7_stepfail3_trace`, `l7_stepswitch`). I trimmed their logs
  to `stub.head.log`/`stub.tail.log`/`pk.head.log` (see `TRIMMED.txt` in each). The full logs were 180 MB + 420 MB.

Every claim below names its run under `$S/runs/`. Claims I could not measure are marked **UNMEASURED**.

---

## L1: connect sequence and register sets

**Commands:** `run.py l1_notarget_full - cmd/l1_basic.txt` (no `target create`); `run.py l1_target_full - cmd/l1_target.txt`
(with `target create exe/prog`); `run.py l1_min cfg/minimal.py cmd/l1_min.txt`; `runenv.sh "UNSUP=…" l1_min_ack …`;
`l1_nothread`; `batch.sh cmd/l1_regs.txt l1_regs cfg/gpronly_xml.py cfg/gpronly_qri.py cfg/full_qri.py`.

**Evidence: full connect sequence, with `target create`.** `l1_target_full/stub.log` shows lldb's requests in order:
```
QStartNoAckMode            <- sent FIRST, before qSupported, even when not advertised
qSupported:xmlRegisters=i386,arm,mips,arc;multiprocess+;fork-events+;vfork-events+;swbreak+;hwbreak+
QThreadSuffixSupported
QListThreadsInStopReply
qHostInfo
vCont?
qVAttachOrWaitSupported
QEnableErrorStrings
qProcessInfo
?
qXfer:features:read:target.xml:0,1ffff     (qRegisterInfo0..N instead when qXfer:features:read+ is not advertised)
qOffsets                   (only with a target)
qStructuredDataPlugins
qShlibInfoAddr
qSupported:…               (sent a second time)
x0,0                       (binary-read probe; empty reply -> lldb uses m)
m7fff5fc00000,200          (old-dyld probe with a target; without one: mfffffff000002010,8 and four more kernel-address probes)
jThreadsInfo
qMemoryRegionInfo:<pc>
m<sp>,200
jThreadExtendedInfo:
qMemoryRegionInfo:<fp+0x10> (x2)
… then the first "(lldb)" prompt
```
The no-target run differs only as noted in the right-hand comments, and it also sends `p0;thread:0001;`.
At batch exit with the process alive, lldb sends `D` (detach) in every run.

**Evidence: a minimal answer set is enough.** `l1_min_ack` answered **every** packet except `qSupported`, `?`,
`qRegisterInfo<n>`, `Hg`/`Hc`, `p`, `m`, `s` and `c` with an empty (unsupported) reply. That included
`QStartNoAckMode`, `qHostInfo`, `qProcessInfo`, `QThreadSuffixSupported`, `vCont?`, `qC`, `qfThreadInfo`,
`qMemoryRegionInfo`, `jThreadsInfo`, `qXfer`, `qShlibInfoAddr`, `qSymbol` and `qOffsets`. `qSupported` was just
`PacketSize=4000` and `?` was `T11thread:1;` with no expedited registers. After `target modules load --file prog --slide 0`:
```
(lldb) register read            -> x0..x28, fp, lr, sp, pc, cpsr correct ("General Purpose Registers:")
(lldb) bt                       -> frame #0 prog`bump(i=4096) at prog.c:4:59 / frame #1 prog`main … at prog.c:7:33
(lldb) disassemble -p -c 2      -> prog`bump: -> 0x1000003d4 <+20>: str x10, [x8, w9, uxtw #3] …
(lldb) thread step-inst         -> Hc-1, s  ->  "stop reason = instruction step into"
(lldb) process continue         -> c -> W00 -> "Process 1 exited with status = 0"
```
With ack mode left on, no NAKs were logged (`grep -c NAK` = 0). Without `vCont?`, lldb steps with `Hc-1` + `s`.
Without `QThreadSuffixSupported`, it reads registers with `Hg1` + `p<n>`. With no `qProcessInfo`, the pid shows as 1.
`l1_nothread` went further: `?` answered `S11` and steps answered `S05` (no `thread:` key, and `qfThreadInfo`
unsupported). lldb still ran everything against an implicit thread 1.

**target.xml or qRegisterInfo.** lldb uses `qXfer:features:read:target.xml:0,1ffff` if and only if `qXfer:features:read+`
is in `qSupported`. Otherwise it walks `qRegisterInfo0…` until it gets `E45` (`l1_regs_gpronly_qri`: `qRegisterInfo0` … `qRegisterInfo22`).
Both work. `register read` names the set after the xml `group` ("general:") or the qRegisterInfo `set`
("General Purpose Registers:"). lldb adds `w0…w31` itself under "supplementary registers".

**GPR-only (no v0–v31/fpsr/fpcr).** Both `l1_regs_gpronly_xml` and `l1_regs_gpronly_qri` pass `register read`,
`frame variable` (`(int) i = 4096`), `bt`, `disassemble`, `thread step-inst` and `register read --all`. The only
failure is naming an FP register: `error: Invalid register name 'v0'.` / `… 'fpsr'.` With the FP set
(`l1_regs_full_qri`), `register read v0 fpsr` gives `v0 = {0x00 …}` and `fpsr = 0x00000010`.
**UNMEASURED:** reading a float/double variable, or an expression with an FP result, on a GPR-only target.

**Conclusion.** Required: `qSupported`, `?` (a T or S stop reply), one register-description route, `p` (or `g`), and `m`.
Everything else may be answered empty. A GPR-only set works for everything except naming FP registers.
Sending FP costs little (`ThreadCtx` has it, see R1), so ship it.

## L2: process identity and dynamic loader

**Commands:** `l2_load` (old loader + `target modules load --file prog --slide 0`), `l2_noload`/`l2_osver`
(`cfg/osver.py`: `qHostInfo` adds `os_version:26.5.0`), `l2_osunknown`, `l2_osunknown_notarget`, `l2_macosx_notarget`,
`l2_jimg_exe`, `l2_jimg_dyld` (`rundyld.sh`), `l2_jimg_notarget` (all `cfg/jimages.py`), `l2_xferlib`/`l2_xfer_dump`
(`cfg/xferlib.py`). Each run also logs `log enable lldb dyld` to `runs/<name>/dyld.log`.

**Which loader.** `dyld.log`:
- `ostype:macosx`, no `os_version`: `DynamicLoaderDarwin::UseDYLDSPI: Use old DynamicLoader plugin` (DynamicLoaderMacOSXDYLD).
- `os_version:26.5.0` present: `… Use new DynamicLoader plugin` (DynamicLoaderMacOS).
- `ostype:unknown;vendor:unknown`, with or without `target create`: still `Use old DynamicLoader plugin`
  (`l2_osunknown`, `l2_osunknown_notarget`). No run ever selected a non-Darwin loader.

**What each loader does with an unsupported answer.**
- Old loader (`l1_target_full`): it sends `qShlibInfoAddr` (empty) and probes `m7fff5fc00000,200`. The exe
  stays in `image list` but is not loaded: `frame #0: 0x00000001000003d4` has no symbol, and lldb prints
  `error: 0xffffffffffffffff can't be resolved`, then `error: Failed to disassemble memory at 0x1000003d4.`
  Running `target modules load --file prog --slide 0` (`l2_load`) fixes it: `dyld.log`
  shows `SetLoadAddress segment '__TEXT' load addr is 0x100000000`, `bt` gives both frames with source lines,
  `disassemble` gives `prog`bump:` lines, and `image lookup -a 0x10000041c` gives `prog`main + 28 at prog.c:7:29`.
- New loader, `jGetLoadedDynamicLibrariesInfos:` unsupported (`l2_osver`): lldb sends
  `jGetLoadedDynamicLibrariesInfos:` (a support probe with no argument), then `qShlibInfoAddr` twice. Then
  `DynamicLoaderDarwin::UnloadAllImages[0] … prog`, so the target shows `Target 0: (No executable module.)`
  and no symbols. **A new-loader target that does not answer `jGetLoadedDynamicLibrariesInfos` loses its exe.**
- New loader, `jGetLoadedDynamicLibrariesInfos` answered (`l2_jimg_exe`): the probe gets `OK`, then
  `jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}` gets `{"images":[{load_address, mod_date, pathname,
  uuid, min_version_os_name, min_version_os_sdk, mach_header{magic,cputype,cpusubtype,filetype,flags},
  segments[{name,vmaddr,vmsize,fileoff,filesize,maxprot}]}]}` (see `$S/macho.py::image_json`). The first stop is
  already symbolicated, with source: `frame #0: 0x00000001000003d4 prog`bump(i=4096) at prog.c:4:59 [opt]`. `bt` and
  `disassemble -p` are correct, and `image list` shows only the exe at `0x0000000100000000`.
  **No `target create` is needed** (`l2_jimg_notarget`): lldb loads the exe from `pathname`.
- `jGetSharedCacheInfo` was **never sent** in any run. **UNMEASURED:** what triggers it, and whether listing
  shared-cache dylibs at their slide-0 addresses symbolicates libsystem frames.
- `qXfer:libraries:read+` advertised under the old loader (`l2_xferlib`): lldb does request
  `qXfer:libraries:read::0,1ffff`, but `<library name=…><segment address="0x100000000"/>` did not load the exe
  (the same `can't be resolved` error appeared, `l2_xfer_dump`). This is not a usable mechanism on a macOS target.

**dyld in the image list costs a breakpoint** (`l2_jimg_dyld`, exe + `/usr/lib/dyld` at `0x120000000`).
`dyld.log`: `Found dyld module: /usr/lib/dyld`. `breakpoint list -i` shows:
```
Kind: shared-library-event
-1: name = 'lldb_image_notifier', module = dyld, locations = 1, resolved = 1
```
The stub log shows `Z0,120037a08,4` sent **at connect, before any resume**, and it stays inserted. With the exe
only (`l2_jimg_exe`), `breakpoint list -i` says `No breakpoints currently set.` and no Z packet is ever sent.

**Identity fields.** `qHostInfo`/`qProcessInfo` may be empty (`l1_min_ack`). lldb then still infers a Darwin arm64
target from the exe and shows `Process 1`. The pid in `qProcessInfo` is printed on every stop
(`Process 42 stopped`), so the seam should report a constant.

**Conclusion.** Two cheap routes work:
- (a) `qHostInfo` without `os_version`. The user or test runs `target create <exe>` and then
  `target modules load --file <exe> --slide 0`.
- (b) `qHostInfo` with `os_version`, and answer `jGetLoadedDynamicLibrariesInfos` (probe → `OK`,
  `fetch_all_solibs` → the exe's JSON). Symbolication then needs zero user commands. (b) is the one to build.

Never list dyld unless the plan is to accept the persistent `lldb_image_notifier` Z0 (see L5).

## L3: reverse execution

**Commands:** `l3_dir` (`cfg/rev1.py`), `l3_frombp`, `l3_norev_{none,contonly,steponly}` (`nrbatch.sh`), `l3_monitor`,
`l3_monitor2`, `l3_rsi` (`cfg/monitor.py` + `$S/rsi.py`), `l3_condbp`. Also `lldb -b -o 'help process continue' …`
(`runs/help_step.txt`), and an SB API listing via `lldb -b -o 'script print(…dir(lldb.SBProcess)…)'`.

**Evidence.**
- `ReverseContinue+` in `qSupported` is what enables `bc`. Without it (`l3_norev_none`, `l3_norev_steponly`):
  `error: Failed to resume process: target does not support reverse execution of processes.` With only
  `ReverseContinue+` (`l3_norev_contonly`), `process continue -R` sends `bc`.
- **`bs` was never sent by any command in any run**, with `ReverseStep+` advertised throughout. That covers
  `thread step-inst`, `si`, `ni`, `step-in`/`step-out`/`step-over`, conditional and ignore-count breakpoints in
  reverse (`l3_condbp`), and watchpoint stops in reverse. The CLI has no reverse step: `help thread step-inst`
  has no direction option, `help process continue` shows only `-F`/`-R` on continue, `apropos reverse` finds
  nothing, and no `settings` key mentions direction. The SB API exposes `SBProcess.ContinueInDirection` and
  `lldb.eRunReverse`/`eRunForward`/`eStopReasonHistoryBoundary`, and nothing reverse on `SBThread`.
- **The direction persists for `continue` only** (`l3_dir`):
  ```
  process continue -R            -> Z0,100000414,4 / bc
  thread step-inst (after -R)    -> z0 / vCont;s:1 / Z0     (FORWARD)
  si, ni                         -> vCont;s:1               (FORWARD)
  process continue   (no flag)   -> bc                      (still REVERSE)
  process continue -F            -> z0 / vCont;s:1 / Z0 / c
  process continue   (no flag)   -> c
  ```
  `rsi` (below) also leaves the direction reversed: a later plain `process continue` sent `bc` (`l3_rsi`).
- Reverse-continue **from a breakpoint** does not step off it (`l3_frombp`): stopped at bp `0x100000414`,
  `process continue -R` sends a bare `bc` with the Z0 still inserted, twice in a row. Forward from the same
  bp sends `z0 / vCont;s:1 / Z0 / c`.
- A breakpoint whose condition is false (`-c 0`) gets another `bc` in reverse, with no step
  (`l3_condbp` stub.log lines 77–277):
  - `bc` → bp hit;
  - the condition is evaluated once (the `_M`/`P` storm of L8);
  - then `bc`, `bc`, `bc`;
  - the stub's queue ran out, so the last `bc` got `replaylog:begin` → `history boundary`.

  An **ignore count in reverse is UNMEASURED**: that `bc` hit the exhausted queue.
  An ignore count forward re-continues by stepping off the bp first (`c`, `z0`, `vCont;s:1`, `Z0`, `c`, stub.log 323–335).
- **A reverse single step is possible, but only through a side channel.**
  - `process plugin packet monitor <cmd>` sends `qRcmd,<hex>`. O-packet output is printed decoded, and a
    final raw reply prints as `response: …` (`l3_monitor2`).
  - A server-side move via qRcmd alone leaves lldb's register cache **stale**: after `monitor rsi` moved pc to
    `…3d0`, `register read pc` still printed `0x00000001000003d4` (`l3_monitor`).
  - Arming the server (`qRcmd arm-rsi` → `OK`) and then `process continue -R` works: that `bc` is answered
    with the pc moved back 4 and `reason:trace`, and lldb shows `stop reason = trace` with fresh registers.
    `$S/rsi.py` wraps both in an lldb command `rsi`. `l3_rsi` shows two `rsi` steps (`…3d0`, then `…3cc`),
    each `qRcmd,61726d2d727369` followed by `bc`.

**Conclusion.**
- Premise corrected: in lldb-2100 only `process continue -R` (alias `c -R`) and `SBProcess.ContinueInDirection`
  reach the server in reverse, and only as `bc`. The `bs` packet exists in the client but no measured command
  produces it.
- "Reverse-step through a real crash in LLDB" therefore needs a retrace-shipped lldb Python command
  (qRcmd arm + `bc`, measured working). Reverse-continue alone is stock.
- `-F` restores forward, and `continue` without a flag keeps the last direction.

## L4: stop replies

**Commands:** `wbatch.sh` (`cfg/watch.py` × `WFORM=reason|watch` × `WEXC=none|before|after`) → `l4_w2_*`;
`l4_w2_after_stepped` (`WSTEPPED=1`); `l4_watch_reason_none`; `cbatch.sh` → `l4_crash_*`; `ebatch.sh` → `l4_ends_*`;
`l4_ends2`; `l10_hb_s`; `ideal`.

**(a) Breakpoint.** `T05thread:1;…;reason:breakpoint;` prints `stop reason = breakpoint 1.1` for both a `c` and a `bc`
answer (`l3_dir`, `l3_frombp`). If lldb has no site at that pc, the same reply prints `stop reason = signal SIGTRAP`
(`l5_hw` after `breakpoint delete 1`, and `l3_norev_contonly`). A `T05` with no `reason:` also prints `signal SIGTRAP`.

**(b) Step.**
- An `s`/`vCont;s` answered with `reason:trace` prints `instruction step into` (si), `instruction step over` (ni),
  `step in` / `step out` / `step over` for the source-level commands (`l5_stepping`).
- A `bc` answered with `reason:trace` prints `stop reason = trace` (`l3_monitor`, `l3_rsi`).
- **A `reason:trace` reply that does not move the stepped thread's pc makes lldb step again forever:**
  `l7_stepfail3_trace` logged 285,856 × `vCont;s:1` in 60 s and was killed.

**(c) Watchpoint.** `watch:<addr>` and `reason:watchpoint;description:<hex "addr idx hitaddr">` behave identically
(`l4_w2_watch_*` vs `l4_w2_reason_*`). Both print `stop reason = watchpoint 1` plus `Watchpoint 1 hit: old value: … new value: …`.
- With no `watchpoint_exceptions_received` key, or `…:before` (`l4_w2_reason_none`/`_before`), lldb treats the
  stop as pre-retire and **steps over the store itself**: `z2,100004020,8 / vCont;s:1 / Z2 / z2 / Z2`. It then
  reports at the next instruction (`0x1000003ec`) with `old value: 5 new value: 6`.
- **In reverse it does the same forward step.** `bc` gets a watch stop at `…3e8`, lldb sends `vCont;s:1`
  (FORWARD) and reports at `…3ec` with `old value: 6 new value: 6`. On retrace that step re-executes the store. The
  next `c -R` from `(n,k+1)` finds that same store again, so a user could never get backward past the most recent write.
- With `watchpoint_exceptions_received:after` (`l4_w2_reason_after`), lldb sends only `z2/Z2` and **never steps**,
  in either direction. It reports the pc it was given.
- The clean form is `l4_w2_after_stepped` / `ideal`: key `after`; forward hits reported **post-retire**
  (pc past the store, new value in memory); reverse hits reported **pre-retire** (pc on the store, old value in
  memory). lldb prints:
  ```
  c -F : stop reason = watchpoint 1, frame 0x1000003ec,  old value: 0  new value: 1
  c -R : stop reason = watchpoint 1, frame 0x1000003e8,  old value: 1  new value: 0
  c -R : (next older write)                              old value: 5  new value: 4
  ```
- `watchpoint set variable counter` defaults to `type = m` (modify). lldb silently re-sends `c` when the value did not
  change (`l4_w2_*` first round: three watch stops auto-continued to `W00`). `-w write` gives `type = w`, which reports every hit.

**(d) Crash.**
- `T0bthread:1;metype:1;mecount:2;medata:1;medata:10;` → `stop reason = EXC_BAD_ACCESS (code=1, address=0x10)`
  (`l4_crash_mach`). `reason:exception;description:<hex text>` → the text verbatim (`l4_crash_exc`). Both together →
  `EXC_BAD_ACCESS (code=1, address=0x10)` (`l4_crash_excmach`). With either, `bt` works
  (`frame #1: 0x0000000100000434 prog`main … at prog.c:8:19`), and **`process continue -R` works (`bc`)**.
- A plain signal (`T0bthread:1;`, `…reason:signal;`, or `reason:signal;description:…`) prints `signal SIGSEGV` or the
  description. After it, **`process continue -R` fails:
  `error: Failed to resume process: can't deliver signals while running in reverse.`**
  The error text is in `err.txt` of `l4_crash_sig`, `_sigreason`, `_sigdesc` and the three `l4_ends_T0x…` runs.
  A forward `continue` after a signal stop resends the signal as `vCont;C<sig>:1`. The stub.logs show exactly one each:
  - `vCont;C0b:1` in `l4_ends_T0bsig_T05begin`;
  - `vCont;C06:1` for `T06` (`l4_ends_T06_T05begin`);
  - `vCont;C09:1` for `T09` (`l4_ends_T09_T00begin`).
  `process handle SIGSEGV -p false` removes the error (`l4_ends2`).
- `T06thread:1;reason:exception;description:<hex "signal SIGABRT">;` prints `stop reason = signal SIGABRT`, and
  reverse works (`l4_ends_T06exc_T05begin`).

**(e) End of recording.**
- `W00` → `Process 42 exited with status = 0 (0x00000000)`. After it, `process continue -R` →
  `error: Process must be launched.` (`l4_w2_reason_none` first round; `X09` likewise: `exited with status = 9`,
  `l4_ends_X09_T05begin`).
- `T09` behaves as a SIGKILL signal stop, so reverse is poisoned as above.
- `T05thread:1;replaylog:end;` → `stop reason = history boundary`. The process stays alive, `c` can be repeated,
  and `c -R` works (`l4_ends_T05end_T05begin`). It also works without a `thread:` key (`…nothread…`); lldb then
  asks `qfThreadInfo`.
- `replaylog:end;description:<hex>` prints the description instead: `stop reason = end of recording: exited (code 0)`.
  `SBThread.GetStopReason() == eStopReasonHistoryBoundary` holds (`l4_ends2`, `l10_hb_s`:
  `AFTER-END-1 True end of recording: exited (code 0)`).
- `reason:history_boundary` is not a recognized reason and prints `signal SIGTRAP` (`l4_ends2`).

**(f) Start of recording.** `T05thread:1;replaylog:begin;` → `history boundary`. With `description:<hex "start of recording">`
→ `stop reason = start of recording`, and `eStopReasonHistoryBoundary` holds (`l4_ends2`, `l10_hb_s`). It also works
as the answer to `?` at connect (`ideal`: `Process 1 stopped / * thread #1, stop reason = start of recording`).

**Conclusion.** Use Mach-exception keys (optionally with `reason:exception;description:`) for crashes.
Use `replaylog:end|begin` + `description:` for both boundaries. Use `watchpoint_exceptions_received:after`,
reporting forward watch hits post-retire and reverse watch hits pre-retire. Never send a plain signal stop, and never send `W`/`X`.

## L5: breakpoints

**Commands:** `l3_dir`, `l3_frombp`, `zbatch.sh` → `l5_z0unsup_memok|memno`, `l5_z0err_memok|memno`, `l5_hw`;
`l5_stepping` (`cfg/emu.py`, a tiny BL/RET-aware stepper whose `c` runs to the most recently inserted bp);
`l5_zfail_E01` (`cfg/emu_zfail.py`); `l2_jimg_dyld`.

**Evidence.**
- `breakpoint set -a` and `breakpoint set -n` use **Z0** (`Z0,100000414,4`). `breakpoint set -H` uses **Z1**. The kind is always 4.
- Insertion is immediate at `breakpoint set` when the process is live (`l5_hw`: `Z1`, `Z0` before any `c`).
  Breakpoints **stay inserted across stops**. lldb removes one only to step off it (`z0 / vCont;s:1 / Z0 / c`),
  at `breakpoint delete`, and at detach.
- `Z0` answered empty (unsupported) → lldb retries the same address with `Z1` and uses Z1 from then on (`l5_z0unsup_*`).
- `Z0` answered `E01` → **no fallback**: no Z1, and **no `M`/`X` memory writes** in either `MEMW` setting. The location
  stays unresolved (`breakpoint list`: `locations = 1` with no `resolved`), and hits print `signal SIGTRAP` (`l5_z0err_*`).
- lldb's `qSupported` offers `swbreak+;hwbreak+`. `BreakpointCommands+` was never sent or used.
- **Internal breakpoints.**
  - The exe-only image list plants none (`l2_jimg_exe`, `l5_stepping`: `No breakpoints currently set.`).
  - A dyld image plants **1 persistent** Z0 at `lldb_image_notifier`, at connect (`l2_jimg_dyld`).
  - Source/call stepping plants **1 transient** Z0 per command, inserted just before `c` and removed right after
    (`l5_stepping`):
    ```
    ni over `bl _bump`   : vCont;s:1, Z0,10000041c,4, c, z0,10000041c,4
    thread step-in       : vCont;s:1 x3, Z0,1000003f8,4, c, z0
    thread step-out      : Z0,100000434,4, c, z0
    thread step-over     : Z0,100000448,4, c, z0
    ```
  - **If that transient Z0 is refused, lldb runs away** (`l5_zfail_E01`):
    `warning: failed to set breakpoint site at 0x10000041c for breakpoint -2.1: error: 1 sending the breakpoint request`,
    then it still sends `c`, which ran to `W00` (`Process 42 exited with status = 0`) instead of stopping after the call.
- **UNMEASURED:** internal breakpoints from language runtimes (ObjC/Swift) when their libraries are listed.

**Conclusion.** Slot pressure is the user's breakpoints, plus 1 transient for every step-over/step-out/step-in/`ni`-over-call,
plus 1 persistent if dyld is listed. lldb enforces no count. So the seam must either always keep a slot
free for the transient one, or implement overflow beyond 6 by some non-hardware means. Refusing an internal Z0 is a correctness bug, not a clean error.

## L6: watchpoints

**Commands:** `l6_sizes` (`cfg/wsizes.py`: `_M` served, P refused), `w6batch.sh` → `l6_err_E01`, `l6_err_empty`, `l6_nowsi`,
`l6_z2_E01`, `l6_z2_nowsi`, `l6_z2_five`.

**Evidence (`l6_sizes`).**
```
-w write -s 8 -- 0x100004020      -> qWatchpointSupportInfo: (num:4;), Z2,100004020,8
-w write -s 4 -- 0x100004000      -> Z2,100004000,4
-w write -s 1 -- 0x100004009      -> Z2,100004009,1
-w read  -s 4 -- 0x100004010      -> Z3,100004010,4
-w read_write -s 4 -- 0x100004018 -> Z4,100004018,4
watchpoint delete 1               -> z2,100004020,8
watchpoint disable 2              -> z2,100004000,4
-w write -s 8 -- 0x100004004      -> Z2,100004000,8 + Z2,100004008,8   (lldb splits into aligned 8-byte pieces)
-w write -s 16 -- 0x100004000     -> Z2,100004000,8 + Z2,100004008,8
```
- Insertion is immediate at creation. `qWatchpointSupportInfo` is asked once and **not required**: with it unsupported,
  Z2 still goes out and succeeds (`l6_z2_nowsi`).
- **lldb does not enforce `num:4`.** It sent a fifth `Z2` with no complaint (`l6_z2_five`).
- `Z2`/`Z3` answered `E01` or empty → `error: Watchpoint creation failed (addr=0x100004020, size=8).` /
  `error: Setting one of the watchpoint resources failed`, and nothing is left armed (`l6_z2_E01`, `l6_err_*`).
- `watchpoint set expression` **evaluates an expression**. Depending on L8's answers, that can mean `_M` allocation,
  `M` writes to the allocation, or an attempted inferior function call (L8).
  `watchpoint set variable -w write counter` needs no expression evaluation (`l4_w2_*`: only `qWatchpointSupportInfo`, `Z2`).

**Conclusion.** Z2 is the only kind retrace can honour. Refuse Z3/Z4 with an E-code (a clean error). The server must enforce
the 4-slot limit itself and accept lldb's aligned 8-byte splitting.

## L7: threads

**Commands:** `t7batch.sh` → `l7_threads_default`, `l7_threads_nosuffix` (`THREADSKEY=0 SUFFIX=0 LIST=0`),
`l7_threads_jti` (`JTI=1`); `l7_stepfail*` (`cfg/threads_stepfail.py`, `cfg/resume_err.py`); `l7_stepswitch`
(`cfg/stepswitch.py`); `l7_tid0` (`cfg/tid0.py`).

**Evidence.**
- lldb's thread index `#n` is discovery order, **not** the tid. When the initial stop names tid 2, `thread #1` is tid 0x2 (`l7_stepfail_E01`).
- With `threads:`/`thread-pcs:` in T replies, lldb never asks `qfThreadInfo` after a stop. It asks `qThreadStopInfo<tid>`
  for each non-reporting thread (answered `T00thread:1;…`, "no reason"). Without them (`l7_threads_nosuffix`), every stop
  is followed by `qfThreadInfo`/`qsThreadInfo`.
- With `QThreadSuffixSupported`, registers are read as `p<n>;thread:<tid>;` with no `Hg`. Without it, it uses `Hg<tid>` + `p<n>`.
  `thread select 2` sends nothing, and `register read` on the selected thread sends `p…;thread:0002;` (or `Hg2`).
- `jThreadsInfo`, when supported, is fetched once after each stop (`l7_threads_jti`). It is not required: unsupported everywhere else.
  `jThreadExtendedInfo:` is asked at connect, and unsupported is fine.
- Step with 2 threads → `vCont;s:<tid>` **only** (the other thread is not resumed). Continue → plain `c`, even with
  `vCont` supported. Reverse → `bc`, which has no thread operand.
- **An E-reply to a resume packet kills the session.** `vCont;s:1 → E01`:
  `Process 42 exited with status = -1 (0xffffffff) lost connection`, then `error: Process must be launched.`
  (`l7_stepfail2_E01`). `bc → E01` does the same (`l7_stepfail3_bcerr`).
- A non-moving stop reply with `reason:exception;description:…` on the stepped thread is displayed and is safe
  (`l7_stepfail3_desc`: `thread #2, stop reason = cannot step thread 1: it is blocked; only the running thread (2) can step`).
- **A step of thread A answered by a stop on thread B, with A's pc unchanged, makes lldb re-step A forever**
  (`l7_stepswitch`: 349,194 × `vCont;s:1` in 60 s, killed).
- **tid 0 is unusable** (`l7_tid0`): `T11thread:0;threads:0,1` gives `Process 42 stopped` with **no threads** listed.
  `register read` → `error: Command requires a process which is currently stopped.`, and lldb loops on `qfThreadInfo`.
- `QListThreadsInStopReply` and `QThreadSuffixSupported` are both optional (`l1_min_ack`, `l7_threads_nosuffix`).

**Conclusion.**
- Map retrace tid `t` → RSP tid `t+1`.
- Put `thread:`, `threads:` and `thread-pcs:` in every T reply, and answer `qThreadStopInfo` for the other threads.
- Never answer a resume packet with an E-code.
- A step request for thread A must end with A reported either with a moved pc, or with a non-trace reason. On retrace
  that means stepping across a *blocking* syscall runs until A is scheduled again and retires the `svc`, since other threads run meanwhile.
- Stepping a thread that is not the one on the vCPU needs an explicit policy. The measured-safe answer is a non-moving
  `reason:exception` + description stop.

## L8: memory reads, and the writes lldb attempts

**Commands:** `l8_mem`; `xbatch.sh` → `l8_expr_unsup_regw`, `l8_expr_unsup_noregw`, `l8_expr_alloc_noregw`, `l8_expr_unsup_nowrites`
(`cfg/alloc.py`); `l3_condbp`; tallies over every `runs/*/stub.log`.

**Evidence.**
- Reads come in 0x200-byte cache lines (`m16fdffe00,200`). An `E08` for an unmapped line:
  - `memory read -c 16 0x200000000` → `error: memory read failed for 0x200000000`.
  - A read that crosses into an unmapped line prints the mapped half, prints **zeros** for the rest, and adds
    `warning: Not all bytes (16/32) were able to be read from 0x16fdffff0.`
  - `0x10` → `error: error reading data from section __PAGEZERO` (`l8_mem`).
- `qMemoryRegionInfo` is **not needed**. With it unsupported, `bt`, `disassemble` and stepping all work (`l1_min_ack`).
  When present, lldb uses it heavily (763 requests across all runs).
- **No memory or register writes in ordinary operation.** Across all runs, `M`/`X`/`P`/`G`/`_M`/`QSaveRegisterState`
  appear only in runs that evaluate an expression: `expr`, `watchpoint set expression`, and a breakpoint condition
  (`grep -l` over `runs/*/stub.log`). Breakpoints use Z packets and steps use `s`/`vCont;s`.
- **Expression evaluation can resume the inferior.** `expr 1+2` with `_M` unsupported and `P` accepted
  (`l8_expr_unsup_regw`):
  ```
  _M1000,rwx -> ""   QSaveRegisterState -> ""   g
  P0=0 P1=0x1000 P2=7 P3=0x1002 P4=-1 P5=0 P1e(lr)=0x100000400 P1f(sp)=… P20(pc)=0xffffffffffffffff
  Z0,100000400,4   c   (!)   … G<saved regs>   z0,100000400,4
  ```
  That is an inferior `mmap(0,0x1000,RWX,MAP_ANON|MAP_PRIVATE,-1,0)` call. lldb **sent `c`**. On retrace that would
  advance the replay under a register state lldb invented.
- With `P` refused (`E01`), `P0` fails first. lldb then tries to restore with `G` and every `P<n>` (≈70 refused writes),
  and **never resumes**. `expr 1+2` still prints `(int) $0 = 3` (`l8_expr_unsup_noregw`, `…_nowrites`).
- With `_M` served (`l8_expr_alloc_noregw`), lldb allocates `_M1000,rwx`, `_M1000,rw`, `_M81000,rw` and writes its result
  with `M300001010,…`. There are no register writes and no resume.
- A breakpoint condition (`-c 0`) triggers the same allocation attempt on its first evaluation (`l3_condbp`).
- **UNMEASURED:** an expression that calls a target function with `_M` served and `P` refused. It should fail cleanly
  because `P` is refused, but I did not run it.

**Conclusion.** Refuse `P`, `G` and `QRestoreRegisterState`, and refuse `M`/`X` into guest memory. Serving `_M`/`_m` from a
debugger-side map outside the guest address space removes the refused-write storm and makes `expr` quiet.
Partial `m` reads must return the readable prefix (see R-side note on `read_mem`).

## L9: miscellaneous protocol

**Commands:** `l1_min_ack`, `l9_interrupt` (`cfg/hang.py` + `$S/intr.py`), `l9_slow` (`cfg/slow.py`), and a packet tally over all runs.

**Evidence.**
- `QStartNoAckMode` is always the first packet. Both answers work (`OK` in most runs, empty in `l1_min_ack`).
- `PacketSize=20000` (hex) led lldb to ask for target.xml as `0,1ffff`. Memory reads stayed 0x200 per packet regardless.
- **Never sent in any run:** `qEcho`, `vFile:*`, `qPlatform_*`, `vKill`, `bs`, `jGetSharedCacheInfo`, `BreakpointCommands`.
- `qVAttachOrWaitSupported` is sent at connect; empty is fine.
- `process kill` → `k` (stub answered `X09`) → `Process 42 exited with status = 9 (0x00000009) killed`.
  Batch exit with a live process → `D`. The `W`/`X` exits send nothing further.
- **Interrupt** is the raw byte 0x03, sent while a `c` **or a `bc`** is outstanding (`l9_interrupt`: `c`,
  then `^C (interrupt)` at +0.5 s; `bc`, then `^C` at +0.5 s). Answering with a T stop completes it (`stop reason = signal SIGSTOP`).
  A batch run whose async `process continue` is still running at exit also sends ^C and then `D`.
- **Timeouts** (`l9_slow`; `plugin.process.gdb-remote.packet-timeout (unsigned) = 5`):
  - A `bc` answered after 8 s was accepted with no timeout: stub.log `0.207 <- bc`, `8.218 -> T05…reason:breakpoint;`,
    and lldb printed `stop reason = breakpoint 1.1`. Resume packets wait.
  - A `qRcmd` answered after 8 s **did time out** in lldb. Its `pk.log` order is `send qRcmd,736c6f77`,
    `send qC`, `read OK`, `read QC1`: at 5 s lldb sent a `qC` resync, then consumed the late `OK`. It still printed
    `response: OK`, and the session continued normally.
  - **UNMEASURED:** a slow `m`/`p` (non-resume) reply, where a resync might not recover as cleanly.

**Conclusion.** Implement 0x03 during long scans. Keep every non-resume reply under 5 s. Implement `k`
(end the session) and `D` (detach: close cleanly).

## L10: batch driving for tests

**Commands:** every run (`lldb -b -s cmds.txt </dev/null`); the `--o` variants `l1_regs_gpronly_xml_o`, `l10_crashbatch_o_*` and
`l10_hb_o`; `stab.sh` (5× the watch session, 5× the crash session); `lldb -x -b -o 'script print("X-OK")'`.

**Evidence.**
- `lldb -b` is **synchronous**: every `process continue`/`thread step-inst` blocks until the stop, and the output is in
  command order. No `SetAsync(False)` is needed. `SetAsync(True)` inside a batch stops the batch after the resume (`l9_interrupt`, first version).
- **The first failing command ends the batch with exit code 1**, with both `-s` and `-o`. `settings set interpreter.stop-command-source-on-error false`
  did **not** change this (`l1_regs_gpronly_xml`, `l1_regs_gpronly_xml_o`). An error message inside a successful command
  (the connect's `error: 0xff… can't be resolved`) does not stop it.
- **`-o` silently stops at a crash or history-boundary stop, and exits 0**:
  - `l10_crashbatch_o_sig` / `_o_excmach`: after `stop reason = signal SIGSEGV` / `EXC_BAD_ACCESS`, the remaining
    `-o` commands never run, lldb sends `D`, and rc=0.
  - `l10_hb_o`: the same after `end of recording: exited (code 0)`.
  - With `-s`, all of them continue (`l10_crashbatch_s_*`, `l10_hb_s`).
- A clean batch exits 0 with the process still alive (lldb detaches).
- **Deterministic?** 5/5 identical `out.txt` for each of two sessions (watch forward/reverse; crash + reverse), once the
  port and run path are normalised (`stab.sh`: `watch run 2..5: identical to run 1`, `crash run 2..5: identical to run 1`).
- Nondeterministic or environment-bearing text a test must not assert on:
  - the port in `(lldb) gdb-remote 127.0.0.1:<port>`;
  - absolute paths in the `command source`/`Executing commands in`/`Current executable set to` lines;
  - the `$S` path in `declare @ '…/prog.c:2'`;
  - the pid in `Process <pid>` (constant only if the seam fixes it);
  - `warning: … compiled with optimization` on stderr.
- `-x` (skip `.lldbinit`) is accepted with `-b`. No `~/.lldbinit` exists here, so its effect is **UNMEASURED**.

**Conclusion.** The test invocation is `lldb -x -b -s <cmds file> </dev/null`. Assert on:
- a final sentinel line (`script print("END")`), because silent truncation is otherwise possible;
- `stop reason = …` lines;
- `frame #0: 0x… module`sym…` lines;
- `old value:` / `new value:` lines.

Assert on the exit code only together with the sentinel. Never use `-o` for anything after a stop that might be a crash or a boundary.

---

## R1: register access through `ReplaySession`

**Read:** `crates/retrace-core/src/lib.rs:2736–2848, 2996`; `crates/retrace-box/src/lib.rs:4333, 4735–4774, 5755–5832, 6059–6066`;
`crates/retrace-box/src/thread.rs:18–51, 155–159`; `crates/retrace/tests/blockedctx.rs:1–14`.

- **Confirmed: string-only.**
  - `ReplaySession::dbg_regs()` (`lib.rs:2841`) returns `x0..x30, sp, pc, elr, far, spsr`. It prints `SPSR_EL1`, not CPSR.
  - `dbg_regs_of(tid)` (`:2748`) returns the same text shape from the saved context for a non-current thread.
  - `dbg_fp_regs()` (`:2996`) returns `q0..q31, fpcr, fpsr` for the **current thread only**, and is documented as test-only.
  - `pc()` (`:2814`) is the only structured register accessor.
- The structured data already exists.
  - `Box_::save_ctx()` (`box lib.rs:4735`, `pub`) returns a `thread::ThreadCtx`:
    `{ regs: Regs{x[31], pc, sp_el0, cpsr}, fp: [u128;32], fpcr, fpsr, tpidrro_el0, elr, spsr }`, read off the live vCPU.
  - `Box_::threads().ctx_of(tid)` (`thread.rs:159`) is the saved context of a non-current thread.
    `switch_to_thread` (`box lib.rs:5755`) fills it with `save_ctx()` at every switch.
  - **So q0–q31/fpsr/fpcr are readable for a non-current thread** (`ctx.fp`, `ctx.fpsr`, `ctx.fpcr`).
  - `ThreadCtx` lives in `pub mod thread`. `retrace-core` already re-exports `retrace_box::thread::{BlockReason, ThreadState}` (`core lib.rs:9`).
- **Cheapest accessor:**
  - `Box_::thread_ctx(&self, tid) -> Option<ThreadCtx>`, which returns
    `if tid >= len {None} else if tid == current {Some(self.save_ctx())} else {Some(ctx_of(tid).clone())}`.
    This has exactly `dbg_regs_of`'s current/saved split (`box lib.rs:5822–5831`).
  - A `ReplaySession::thread_ctx` delegator.
  - `pub use retrace_box::thread::ThreadCtx` in `retrace-core`.
- Which PSTATE to report as `cpsr`: `regs.cpsr`. Two facts from reading the code:
  - `set_x0_err_and_return` (`box lib.rs:3293–3299`) emulates the `eret`: `PC=ELR`, `CPSR=SPSR` with C updated. So after
    any completed syscall, the live and saved `regs.cpsr` is the EL0 PSTATE.
  - `ctx.spsr` of a Wait-blocked thread is raw exception-entry state (`0x60000000`, C set; `box lib.rs:3339–3347`).
    It is the wrong register to show.

  This is derived from reading the code; **UNMEASURED** on a live blocked thread through lldb.

## R2: early exits in the four motion commands (`crates/retrace/src/debug.rs`)

State after the Err, by class:
- **(None)**: `session: None`. The next `sess()` panics on `expect("live session")`.
- **(Armed)**: the live session still has hardware breakpoints and/or watchpoints armed.
- **(Moved)**: the live session is no longer at the cursor `(n, k)`.

`line(…)?` errors are I/O on the output writer. The seam's writer is an in-memory `Vec<u8>`, so they cannot happen there, and they are listed only for completeness.

| line | exit | state left |
|---|---|---|
| `cmd_stepi` 589→595 | `line("error: …")?` after `step_insns` failed | Moved (spent session parked mid-window, not reseeked) — I/O only |
| `cmd_stepi` 598 | `self.reseek(n0, k0)?` | None |
| `cmd_reverse_stepi` 616 | `self.probe_window_len(n)?` | None (probe drops the session first, `:406`) |
| `cmd_reverse_stepi` 623 | `line("at start of recording")?` | None if a probe ran, else consistent — I/O only |
| `cmd_reverse_stepi` 628 | `self.reseek(n, k)?` | None |
| `park_at_terminal` 652/655/660 | `line(…)?` | whatever the caller had (from the scan: Armed + Moved) — I/O only |
| `park_at_terminal` 663 | `probe_window_len(t)?` | None, cursor still pre-command |
| `park_at_terminal` 664 | `reseek(t, kf)?` | None |
| `cmd_continue` 695 | `line(…)?` (Sys-then-bp report) | phase not advanced — I/O only |
| `cmd_continue` 708 | `stepped?` (`step_armed` Err: an unhandled fault) | watches already cleared (707); **Moved** (guest parked on a faulting insn, state unspecified) |
| `cmd_continue` 717 | `line(…)?` | phase already Watch — I/O only |
| `cmd_continue` 730 | `self.reseek(n, k)?` | None |
| `cmd_continue` 732 | `advance().map_err(diverged)?` | **Armed (watches, 731) + Moved** |
| `cmd_continue` 734 | `park_at_terminal(…)` | see park rows |
| `cmd_continue` 738 | `line(…)?` | cursor not yet updated to `(n', 0, Sys)` — I/O only |
| `cmd_continue` 749 | `return Err("… hardware stop during a one-event crossing …")` | **Armed (watches) + Moved** |
| `cmd_continue` 754 | `return Err("… breakpoint stop with no breakpoint armed …")` | Armed (whatever breakpoint fired) |
| `cmd_continue` 763 | `advance().map_err(diverged)?` in the scan | **Armed (breakpoints + watches, 760–761) + Moved** — the case M41 named |
| `cmd_continue` 768/784/803/827 | `line(…)?` | Armed + Moved — I/O only |
| `cmd_continue` 775/776/777 | `resolve_nth(…)?` / `line` / `reseek` | None (774 dropped it) |
| `cmd_continue` 792 | `park_at_terminal` | see park rows (the scan session is dropped by its probe) |
| `cmd_continue` 810/812/814 | `resolve_nth` / `line` / `reseek` | None (809) |
| `cmd_reverse_continue` 863–1016 (every `?` and `return Err`: 863, 872, 885, 894, 918, 936, 949, 962, 966, 991, 992, 994, 998, 999, 1001, 1002, 1007–1009, 1014, 1016) | — | **None** (860 drops the live session up front; the scan session is local). `n/k/phase` are unchanged, so the cursor is valid but has no session |

Two more observations:
- `cmd_stepi` handles its own `step_insns` Err (window end, fault, crash-at-step) by printing `error: …` and
  reseeking. That path is not an `Err` return.
- Arm-count overflow is an `assert!` in `arm_breakpoints`/`arm_watchpoints` (`core lib.rs:2820, 2831`), a panic
  rather than an Err. It is reachable only if `Exec`'s 6/4 checks are bypassed.

**Conclusion.** Recovery after any Err is uniform: `reseek(self.n, self.k)` plus restoring the saved phase. `reseek`
drops the old session, so any arming goes with it, and seeks a fresh, breakpoint-clean one. The seam must do that before
replying, and must turn the Err into a stop reply, never an E-code (L7).

## R3: executable path and load address

**Read:** `crates/retrace-trace/src/lib.rs:7–71`; `crates/retrace/src/main.rs:8–60`; `crates/retrace-box/src/lib.rs:76, 209, 1938–2094`;
`crates/retrace-core/src/symbols.rs:285–300`; `crates/retrace-guest/build.rs:1–15`. Fixture layout measured with
`bash $S/r3_layout.sh` (`runs/r3_layout.txt`, reading the main checkout's built fixtures).

- **The exe path is not in the trace.** `Event::Snapshot { regs, mem }` carries registers and memory only, and there is no header field.
- **`record-dyn` guests:**
  - `load_dynamic` maps the exe at **its own vmaddrs (slide 0)**. `__TEXT` = `EXE_BASE` = `0x1_0000_0000`
    (`runs/r3_layout.txt`: `hello_dyn`, `crashy` `__TEXT vmaddr 0x0000000100000000`).
  - dyld goes at `DYLD_BASE` = `0x1_4000_0000` (vmaddr 0 + 5 GiB).
  - The shared cache is at **slide 0**, its own unslid addresses (`lib.rs:1998`).
  - `build_start_stack` writes `KernelArgs` at the initial `sp_el0`: `[sp]` = main mach_header address (`0x100000000`),
    `[sp+8]` = argc, and `[sp+16]` = `argv[0]`, the pointer to the path string. `apple[0]` is `"executable_path=<argv[0]>"` (`lib.rs:2058–2093`).
  - `argv[0]` is the path **as typed on `record-dyn`** (`main.rs:51`), not canonicalized. It may be relative, and the
    recording's working directory is not recorded.
  - So the path is recoverable from the opening Snapshot (read `sp_el0`, follow `argv[0]`), with that caveat.
- **Static asm fixtures** (`-nostdlib -static -Wl,-e,_start`):
  - `__TEXT` is also at `0x100000000` (`hello`, `crash`: `__TEXT vmaddr 0x0000000100000000`, `LC_UNIXTHREAD`, `_start` at `0x100000380` / `0x1000002e8`).
  - The stack is one granule below `STACK_TOP_IPA` = `0x20000`. There is no argv, so **no path anywhere in memory**.
- The Mach-O header (hence `LC_UUID`) is in the snapshot for both kinds. M19's `Symbols::from_snapshot` already parses
  images at `EXE_BASE` and `DYLD_BASE` from the snapshot's `__LINKEDIT` (`symbols.rs:294`). **UNMEASURED:** whether every
  asm fixture carries `LC_UUID`, and whether lldb can locate an image by uuid alone when `pathname` is absent or wrong.
- **Conclusion.** The seam knows the load address (fixed: `0x100000000`, slide 0) and can recompute the header, segments and UUID
  from the snapshot. The **path** has to come from the seam's own command line (`--exe <path>`), or from `argv[0]` in the
  snapshot for `record-dyn` (if absolute), or from a user `target create`.

## R4: `Exec` outcome → printed line → code path (the map the seam needs)

| outcome | exact line(s) | where |
|---|---|---|
| bp hit, forward scan mid-window | `hit {pc:#x} at ({n}, +?){annot}` then `resolved ({n}, {k})` | `debug.rs:768, 776` |
| bp hit, forward at a boundary | `hit {pc:#x} at ({n}, 0){annot}` | `:784`; the Sys-then-bp finish case `:695` |
| bp hit, reverse | `hit {pc:#x} at ({n}, {k}){annot}` | `:994` |
| watch hit, forward, on the finish step | `hit watch {watched:#x} (write at {pc:#x}) at ({n}, {k}){annot}` | `:717` |
| watch hit, forward scan | `hit watch {watched:#x} (write at {pc:#x}) at ({n}, +?){annot}` then `resolved ({n}, {k})` | `:803, 812` |
| watch hit, syscall write (forward) | `hit watch {watched:#x} (syscall write) at ({n}, 0)` | `:738, 827` |
| watch hit, reverse (store / syscall) | `hit watch … (write at …) at ({n}, {k}){annot}` / `… (syscall write) at ({n}, 0)` | `:1001` / `:1008` |
| exited | `exited (code {code})` | `park_at_terminal :652` |
| crashed | `guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}{annot}` | `:655` |
| fatal signal | `guest terminated by signal {sig}` | `:660` |
| no earlier hit | `no earlier hit` (cursor unchanged) | `:1014` |
| at start of recording (rstepi at (1,0)) | `at start of recording` (then reseeks as far as it got) | `:623` |
| stepi window end | `error: window {n} ends after {M} instruction(s)` (the text before `"; cannot step"`) | `:594–595`, message from `core lib.rs:2879` |
| stepi into a crash / fault | `error: guest crashed at step {d}/{k}: pc=… far=…` / `error: fault during step …` | `core lib.rs:2883 / 2875` via `:595` |
| any Err | returned to `run_script` → `DEBUG ERROR: {e}`, exit 5 | `main.rs` `Some("debug")` arm |

Proposed stop replies (tid = retrace tid + 1; always `thread:`, `threads:`, `thread-pcs:` and expedited `1d–21`):
- bp → `T05…reason:breakpoint;`
- forward watch → step the store first (post-retire), then `T05…watch:<watched>;` (or `reason:watchpoint;description:…`).
- reverse watch → pre-retire `T05…watch:<watched>;`
- syscall-write watch → `T05…watch:<watched>;` at `(n, 0)`.
- exit → `T05…replaylog:end;description:hex("exited (code N)");` at the parked `svc`.
- crash → `T0b…metype:1;mecount:2;medata:<code>;medata:<far>;`, with code 1 or 2 from the DFSC.
  Or `reason:exception;description:hex("guest crashed: pc=… far=… esr=…")`.
- fatal signal → `T<sig>…reason:exception;description:hex("guest terminated by signal N");`
- no earlier hit → reseek to `(1, 0)` and `T05…replaylog:begin;description:hex("start of recording");`
  `Exec` itself stays put, so the seam must choose. Staying put but claiming "begin" would show the wrong pc.
- stepi window end → **not an outcome for lldb**: `s` must cross the landmark (one `advance()`), because `si` on an `svc` must work.
- Err → reseek plus `T05…reason:exception;description:hex(err)`.

`Exec` today prints lines to a `Write` and is private to the `retrace` bin (`debug.rs:321`). The seam needs these as a
returned enum, not parsed text.

---

## Recommendation

### Minimal packet set the server must implement

| packet | answer |
|---|---|
| `QStartNoAckMode` | `OK` (then stop acking) |
| `qSupported:*` | `PacketSize=20000;QStartNoAckMode+;qXfer:features:read+;QThreadSuffixSupported+;QListThreadsInStopReply+;ReverseContinue+;ReverseStep+` |
| `qHostInfo` | `cputype:16777228;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;os_version:26.5.0;watchpoint_exceptions_received:after;` |
| `qProcessInfo` | `pid:1;parent-pid:1;cputype:100000c;cpusubtype:0;ostype:macosx;vendor:apple;endian:little;ptrsize:8;` (fixed pid) |
| `qXfer:features:read:target.xml:o,l` | aarch64 core (x0–x28, fp, lr, sp, pc, cpsr[32]) + fpu (v0–v31, fpsr, fpcr) |
| `QThreadSuffixSupported`, `QListThreadsInStopReply`, `QEnableErrorStrings` | `OK` |
| `jGetLoadedDynamicLibrariesInfos:` / `…{"fetch_all_solibs":true}` | `OK` / exe-only JSON (`$S/macho.py::image_json` shape), load_address `0x100000000` |
| `?` | `T05thread:1;threads:…;thread-pcs:…;<expedited>;replaylog:begin;description:<hex "start of recording">;` |
| `qfThreadInfo`/`qsThreadInfo`, `qC`, `qThreadStopInfo<t>` | `m<t+1,…>` / `l`, `QC<t+1>`, `T00thread:<t+1>;…` for non-reporting threads |
| `Hg`/`Hc` | `OK` (only used without the thread suffix) |
| `p<n>[;thread:t;]`, `g` | from `ThreadCtx` (R1) |
| `m<a>,<l>` | the readable prefix; `E08` only if zero bytes are readable |
| `Z0`/`Z1` | `OK`; must not fail for lldb-internal step breakpoints (L5) |
| `Z2` | `OK` up to 4 (enforced server-side), else `E0x` |
| `Z3`/`Z4` | `E0x` (read watchpoints unsupported) |
| `z0`–`z2` | `OK` |
| `c`, `vCont;c`, `C<sig>` | forward continue → a stop reply (never `W`/`X`, never `E`) |
| `s`, `vCont;s:<t>` | step thread t until **t** retires one instruction (crossing a landmark and blocking syscalls) → `reason:trace` on t with a moved pc, or a non-trace reason |
| `bc` | reverse continue, or — if armed by qRcmd — reverse single step (`reason:trace`); never `E` |
| `bs` | implement as reverse step for completeness (never observed) |
| `qRcmd,<hex>` | `reverse-stepi` arming + a few inspection commands (`where`), output via `O<hex>` packets then `OK` |
| 0x03 | abort the running scan at the next landmark, reply `T05…` (or `T11`) at the resulting position |
| `k` / `D` | `X09` then close / `OK` then close |
| `_M<size>,<perm>` / `_m<addr>` | serve from a debugger-side map outside guest IPA space (optional but recommended) |
| `P`, `G`, `QSaveRegisterState`, `QRestoreRegisterState`, `M`/`X` into guest memory | `E0x` (refusal is measured safe: no resume follows) |
| everything else | empty (unsupported) |

### Stop reply per outcome

See R4's list. The invariants:
- (1) Always name a real thread with tid ≥ 1.
- (2) Always carry `threads:`/`thread-pcs:`.
- (3) Never a plain-signal reply: it poisons reverse and makes `c` send `C<sig>`.
- (4) Never an `E` for a resume packet: it disconnects.
- (5) A `trace` reply must move the stepped thread's pc, or lldb loops forever.

### End and start of recording

End: `replaylog:end;description:<hex "exited (code N)">;`. lldb shows `stop reason = exited (code N)`, the process stays
alive, and `c -R` works.

Crash end: `metype:1;…` (EXC_BAD_ACCESS). Signal end: `reason:exception;description:"guest terminated by signal N"`.
Both are reversible.

Start: `replaylog:begin;description:<hex "start of recording">;`. Use it at connect, for a reverse-continue with no
earlier hit (after reseeking to `(1,0)`), and for rsi at `(1,0)`. `SBThread.GetStopReason()` returns
`eStopReasonHistoryBoundary` for both boundaries (a test can assert on it through `script`).

### Breakpoint-slot pressure

- 0 internal breakpoints at connect (exe-only image list; do not list dyld).
- +1 transient Z0 per `thread step-over`/`step-out`/`step-in`/`ni`-over-a-call, lasting one `c`.
- lldb enforces nothing and ignores failures of internal breakpoints (it runs away).
- With 6 DBGBVR slots, either:
  - reserve 1 slot for lldb's transient breakpoint and cap user Z0/Z1 at 5 (refuse the 6th with `E`, measured clean
    for a user breakpoint); or
  - accept any count and fall back to single-step pc matching when more than 6 are armed (correct but slow).

  Watchpoints: 4 DBGWVR. lldb splits unaligned or 16-byte watches into aligned 8-byte `Z2`s, and doesn't count.

### lldb invocation for tests

```
lldb -x -b -s <cmds.txt> </dev/null
```
cmds: `gdb-remote 127.0.0.1:<port>` … `script print("END")`. Never `-o` (it silently stops after a crash or boundary stop with rc 0).
Assert on:
- `stop reason = …`
- `frame #0: 0x… prog\`sym…`
- `old value:` / `new value:`
- `AFTER`/`END` sentinels

Normalise or avoid the port, absolute paths and the pid (L10). A reverse single step is `command script import <retrace>/rsi.py` + `rsi`.

### What makes the seam harder than "an RSP server over `Exec`"

1. **No reverse step in stock lldb-2100.** `bs` is never emitted, and there is no CLI or SB reverse-step.
   Reverse stepi needs a retrace-shipped lldb Python command (qRcmd arm + `bc`, measured working in `l3_rsi`).
   The exit criterion "reverse-step through a real crash in LLDB" is reachable only that way, or as reverse-continue.
2. **Direction is sticky.** After any `c -R` (or `rsi`), a plain `c` goes **backward** until `c -F`. This is lldb-side
   and cannot be fixed by the server; the user-facing docs and the Python command must say so.
3. **Watchpoint semantics must be re-mapped, not passed through.** AArch64's default "before" makes lldb forward-step
   after a reverse watch hit, which traps the user at the most recent write. The server must advertise
   `watchpoint_exceptions_received:after`, report forward hits **post-retire** (the server steps the store itself) and
   reverse hits **pre-retire**.
   `Exec`'s `Phase::Watch` ("the cursor is ON the store, `continue` steps over it without reporting") contradicts lldb's
   expectation after a reverse watch stop: the next forward `c` should report that same store post-retire. So the
   seam needs its own phase mapping on top of M41's hit order.
4. **Stepping semantics differ from `cmd_stepi`.** lldb's `s` must:
   - cross a landmark (`cmd_stepi` errors at the window end);
   - on a blocking syscall, run other threads until the stepped thread retires (else lldb loops forever: 349,194 steps in 60 s);
   - at a terminal, return the terminal stop, not `trace` (else lldb loops).
   `vCont;s:<t>` for a thread that is not on the vCPU needs a policy. A non-moving `reason:exception` stop is the only measured-safe one.
5. **Error handling is inverted.** `Exec` surfaces errors as `Err` → script abort. Over RSP, any `E` to a resume packet
   disconnects lldb (`lost connection`), so every `Err` must become a stop reply, followed by R2's reseek recovery. Several
   `Err` exits in `cmd_continue` leave hardware armed and the session moved (R2 rows 732, 749, 763).
6. **lldb may try to run code.** Expression evaluation (including `watchpoint set expression` and breakpoint conditions)
   attempts an inferior `mmap` call: `P` writes, then `c`. `P`/`G` must be refused. That was measured safe: no `c` follows.
7. **Internal step breakpoints consume hardware slots**, and a refused one makes lldb run away (L5).
8. **Interrupt needs socket polling during scans.** lldb sends 0x03 during `c` and `bc` and waits indefinitely, so a 40 s
   reverse-continue (M40, rung 8) is uninterruptible unless the single-threaded seam polls between landmarks.
9. **Symbolication needs `jGetLoadedDynamicLibrariesInfos` + `os_version`**, or a manual `target modules load --slide 0`.
   Answering neither leaves an unsymbolicated exe. With `os_version` but no `jGet…` answer, lldb unloads the exe outright.
   The exe **path is not in the trace** (R3).
10. **R-side gaps:**
    - `ReplaySession` exposes registers only as strings (R1). The fix is small: `thread_ctx`.
    - `read_mem` is all-or-nothing within one backing (`core lib.rs:2844–2848`, `box lib.rs:4634–4642`), while lldb reads
      0x200-byte lines that can straddle a backing end. The seam needs a prefix/partial read (walk backings) or lldb shows zeros plus a warning.
    - `Exec` is text-only and private (R4).
11. **tid 0 is unusable** (retrace's main thread is 0): the RSP tid must be `t+1`.
