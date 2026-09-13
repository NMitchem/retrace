# Sweep evidence — M36 Task 2, 2026-09-13

The decisive stderr of every non-clean row of `tools/apple-sweep.sh`, from three full runs of
the 54-entry corpus, copied verbatim from `RETRACE_SWEEP_KEEP`. Traces (≈7–350 MB each) are not
committed; they live in the SDD workspace (`.superpowers/sdd/2026-09-13-retrace-m36-sweepmeasure/
keep-{O,L,I}/<basename>.bin`, git-ignored) and are named per row in that workspace's
`sweep-table.md`, the single source the gates and docs transcribe.

**Binary commit:** `58fb0e7` (branch `m36-sweepmeasure`). Its `crates/` is byte-identical to the
M35 merge `44d302a` (`git diff 44d302a..58fb0e7 --stat -- crates/` is empty; Task 1's two commits
touched only `tools/`). Script: `tools/apple-sweep.sh` at `58fb0e7`, which signs its own copy of
`target/aarch64-apple-darwin/debug/retrace`.

## The runs

The spec asked for two runs, **O** (recorder pid outside M34 §4b's `[0x4000, 0x10000)`) and
**I** (inside). Run O's pids were outside that range and its rows still carried the in-range
signature; the kept trace shows why (below), so a third run **L** at pids below `0x4000` — the
regime M35's out-of-range probes used — was added as the genuinely non-colliding run. All three
are kept.

| run | `pidstart` | recpid range | intended regime | measured regime | `TALLY` |
|---|---|---|---|---|---|
| O | 73426 | 73437–75432 (`0x11EDD`–`0x126A8`) | outside `[0x4000,0x10000)` | colliding — inside `[0x10000,0x18000)`, the guest's `os_alloc_once` slab | `TALLY pass=45 fail=9 skip=0` |
| L | 800 | 812–2851 (`0x32C`–`0xB23`) | below `0x4000` | non-colliding | `TALLY pass=45 fail=9 skip=0` |
| I | 17198 | 17209–19410 (`0x4339`–`0x4BD2`) | inside `[0x4000,0x10000)` | colliding — inside `[0x4000,0x8000)`, the trampoline page | `TALLY pass=46 fail=8 skip=0` |

Every `ROW` line's `recpid` is inside its run's range (0 rows outside, 0 empty, checked with
`awk` over the `ROW` lines); no run crossed a boundary. The pid counter was read with
`sh -c 'echo $$'` and advanced with a loop of `/usr/bin/true`.

**What the three runs say, in one paragraph.** `csh`/`tcsh` panic at the M33 `dup2` assert
(`crates/retrace-core/src/lib.rs:1140`) in every regime. `yes` is killed by the 30 s watchdog in
every regime. The other six rows — `launchctl`, `automationmodetool`, `desdp`, `dyld_info`,
`flex`, `dddiagnose` — take the `brk` (`EC=0x3c pc=0x18035f084`) whenever the pid collides (O, I;
`dddiagnose` on run I instead crashed identically on both sides, `rc=139`) and the RCV-shaped
message-queue `mach_msg2` (`options 0x404000102 pc=0x1804adc34`, `Route::Unsupported`) whenever it
does not (L, 6 of 6). The kept traces show the difference is M34 §4b: 11–12 self-pid
`csops`/`proc_info` calls answered `ESRCH` on O and I, 0 on L.

**Why run O collided.** In every kept trace of the six rows, in every run, landmark #182 is a
`mach_vm_map(size 0x8000, flags 0x49000001)` (tag 73 = `VM_MEMORY_OS_ALLOC_ONCE`) whose returned
address — read back from that landmark's recorded write — is `0x10000`: `first_fit` fills the
gap after `PT_L1_IPA`'s backing. It precedes the first self-pid `csops` (#200), so at the
pid-carrying calls the probe's backings are contiguous over `[0x4000, 0x18000)` = pids
16384..=98303 — 82 % of the pid space, not "roughly half". Non-colliding pids: 1..16383 and
98304..99998. M35's out-of-range probes were at `0x257f`–`0x2662`; the M36 spec §4 first reading
was at `0x10806`–`0x10887` (inside the slab), which is why it saw five `brk`s. The set is
guest-dependent (it is whatever a guest maps below `0x100000` before its own pid-carrying calls),
so "outside `[0x4000, 0x10000)`" is not a regime; the regime-independent fix M34 §4b names is to
stop probing `Scalar` registers as pointers.

**`dddiagnose` in three regimes** (`err` = `err=true` landmarks in the kept trace; self-pid
`ESRCH` counted directly from the `csops`/`proc_info` landmarks carrying the recorder's pid):

| run | recpid | result | `err` | self-pid `ESRCH` | last landmark before the stop |
|---|---|---|---|---|---|
| L | 2340 (`0x924`) | `RECORD ERROR: unsupported mach_msg2 … options 0x404000102` (RCV shape) | 63 | 0 | `mach_msg2` #378 |
| O | 74909 (`0x1249d`) | `RECORD ERROR: … EC=0x3c … pc=0x18035f084` (the libdispatch `brk`) | 75 = 63 + 12 | 12 (169 ×7, 170 ×1, 336 ×4) | `proc_info(2, pid, 17)` → `ESRCH` #378 |
| I | 18781 (`0x495d`) | `PASS (identical fault, rc=139)`: `guest crashed: pc=0x180302eb0 far=0x2000050050 esr=0x92000045` both sides | 71 | 11 (169 ×7, 170 ×1, 336 ×3) | `csops(pid, 0, …)` → `ESRCH` #356 |

The I row's label is the harness's retrace-induced-crash marker (spec §6 control 3), not a pass:
record and replay agree, and the crash follows the mis-answered self-pid calls. Its 71 is not
63 + 8: the I trace ends 22 landmarks before L's wall and lacks three of L's other `err`s and the
twelfth self-pid call.

## Symbolication

The `brk` pc `0x18035f084`, its `elr` `0x1804af110`, the RCV-wall pc `0x1804adc34`, and the run-I
`dddiagnose` crash pc `0x180302eb0` are unslid shared-cache addresses (the guest maps the cache at
slide 0, `crates/retrace-box/src/lib.rs:1387`). On the host, the cache is slid; the symbol is
therefore looked up at `unslid + host slide`.

The spec's `lldb` command was tried first and could not be used, verbatim:

```
$ lldb -b -o 'process launch -s' -o 'image list -o -f libxpc.dylib' -o 'image lookup -a 0x18035f084' -o 'image lookup -a 0x1804af110' /usr/bin/true 2>&1 | tail -20
(lldb) target create "/usr/bin/true"
Current executable set to '/usr/bin/true' (arm64e).
(lldb) process launch -s
error: process exited with status -1 (attach failed (Not allowed to attach to process.  Look in the console messages (Console.app), near the debugserver entries, when the attach failed.  The subsystem that denied the attach permission will likely have logged an informative message about why it was denied.))
```

(`/usr/bin/true` is a platform binary; SIP refuses the attach.) The same command on a
scratchpad-compiled `int main(void){return 0;}` attached (the target sat stopped at entry) and
then printed nothing for ten minutes at `image list`; it was killed. `atos -p <pid>` on a live
copy of the same program hung the same way and was killed. The lookup was done instead with
`dladdr(3)` from a process of my own, which resolves against the same cache mapping every process
shares, plus a raw read of the instruction words at the pc. `dladdr` names the nearest
*exported* symbol; a local symbol closer to the address would not be visible to it.

`sym2.c` (compiled with `cc -o sym2 sym2.c`; the slide is the cache's mapped base minus its
unslid base `0x180000000`):

```c
#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>
#include <unistd.h>
#include <mach-o/dyld.h>
#include <stdint.h>
extern const void *_dyld_get_shared_cache_range(size_t *length);
int main(int argc, char **argv) {
    size_t len = 0;
    const void *base = _dyld_get_shared_cache_range(&len);
    uintptr_t slide = (uintptr_t)base - 0x180000000ULL;
    printf("shared cache base=%p len=%#zx slide=%#lx\n", base, len, (unsigned long)slide);
    uintptr_t addrs[] = { 0x18035f084ULL, 0x1804af110ULL, 0x1804adc34ULL, 0x180302eb0ULL };
    for (int i = 0; i < 4; i++) {
        uintptr_t a = addrs[i] + slide;
        Dl_info info; int ok = dladdr((void *)a, &info);
        printf("unslid %#lx -> slid %#lx: dladdr=%d fname=%s fbase=%p sname=%s saddr=%p (+%#lx)\n",
               (unsigned long)addrs[i], (unsigned long)a, ok, ok ? info.dli_fname : "-", ok ? info.dli_fbase : NULL,
               ok && info.dli_sname ? info.dli_sname : "-", ok ? info.dli_saddr : NULL,
               ok && info.dli_saddr ? (unsigned long)(a - (uintptr_t)info.dli_saddr) : 0UL);
    }
    return 0;
}
```

Output (2026-09-13, this machine):

```
shared cache base=0x18ecdc000 len=0x165170000 slide=0xecdc000
unslid 0x18035f084 -> slid 0x18f03b084: dladdr=1 fname=/usr/lib/system/libdispatch.dylib fbase=0x18f011000 sname=_firehose_task_buffer_init saddr=0x18f03af58 (+0x12c)
unslid 0x1804af110 -> slid 0x18f18b110: dladdr=1 fname=/usr/lib/system/libsystem_kernel.dylib fbase=0x18f189000 sname=__proc_info saddr=0x18f18b108 (+0x8)
unslid 0x1804adc34 -> slid 0x18f189c34: dladdr=1 fname=/usr/lib/system/libsystem_kernel.dylib fbase=0x18f189000 sname=mach_msg2_trap saddr=0x18f189c2c (+0x8)
unslid 0x180302eb0 -> slid 0x18efdeeb0: dladdr=1 fname=/usr/lib/system/libsystem_malloc.dylib fbase=0x18efc0000 sname=mfm_alloc saddr=0x18efdec80 (+0x230)
```

The instruction words around the `brk` pc, read from the live cache mapping (`dis.c`: the same
slide, `*(const uint32_t *)(a + slide)` for `a` from `pc-0x40` to `pc+8`):

```
0x18035f044: a9457bfd
0x18035f048: a9444ff4
0x18035f04c: 910183ff
0x18035f050: d65f0fff
0x18035f054: 350001a0
0x18035f058: d53bd068
0x18035f05c: f9400508
0x18035f060: b9800108
0x18035f064: a9bf57f4
0x18035f068: d00000d4
0x18035f06c: 911fce94
0x18035f070: f034c6d5
0x18035f074: 9128e2b5
0x18035f078: f90006b4
0x18035f07c: f9001ea8
0x18035f080: a8c157f4
0x18035f084: d4200020   <-- pc
0x18035f088: 93407c08
0x18035f08c: a9bf57f4
```

Read: `0xd65f0fff` at `0x18035f050` is `retab` — the end of `_firehose_task_buffer_init`
(`saddr + 0xf8`). The block from `0x18035f054` (`+0xfc`) is an outlined path: `cbnz w0`,
`mrs x8, TPIDRRO_EL0` / `ldr x8, [x8, #8]` / `ldrsw x8, [x8]` (errno, read from the TSD's errno
slot), two `adrp`/`add` pairs and two `str`s into a global (a crash-reason store), then
`0xd4200020` = **`brk #1`** at the recorded pc — `EC=0x3c ISS=0x1` exactly. The `elr` is
`__proc_info + 8`, the return address of the last `proc_info`; in every `brk` trace that last
landmark is `proc_info(2 PIDINFO, <recorder pid>, 17)` answered `ESRCH`. Flavor 17 is not in the
public SDK header (`sys/proc_info.h` jumps from 16 to 19); xnu's `bsd/sys/proc_info_private.h`
defines `PROC_PIDUNIQIDENTIFIERINFO 17` (line 145 of a copy of that header in the session scratchpad). So the `brk` is libdispatch's firehose task-buffer init crashing on a failed
`proc_pidinfo(getpid(), PROC_PIDUNIQIDENTIFIERINFO, …)` — the `ESRCH` §4b manufactures — and not
a libxpc `brk` after `MACH_SEND_INVALID_DEST`, which M23 believed and this measurement retires.

The RCV-wall pc `0x1804adc34` is `mach_msg2_trap + 8` (the trap's return address). The run-I
`dddiagnose` crash pc `0x180302eb0` is libsystem_malloc `mfm_alloc + 0x230` (a data abort,
`esr=0x92000045`, `far=0x2000050050`, identical on record and replay).

## Files

One `<basename>.<run>.<phase>.err` per row and run, verbatim from `keep-<run>/`. `rec.err` is
the recorder's stderr (its first line is `recpid=<pid>`, printed by the wrapper shell that
`exec`s into the recorder); `rp.err` is the replay's, absent when replay did not run (a recorder
panic or timeout ends the row before replay).

- `automationmodetool.I.rec.err` — run I, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `automationmodetool.I.rp.err` — run I, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `automationmodetool.L.rec.err` — run L, the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid `ESRCH`
- `automationmodetool.L.rp.err` — run L, replay ran out of events at the same landmark (`expected recorded syscall, got None`)
- `automationmodetool.O.rec.err` — run O, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `automationmodetool.O.rp.err` — run O, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `csh.I.rec.err` — run I, recorder panic at the M33 `dup2` assert (`lib.rs:1140:17`); no replay ran
- `csh.L.rec.err` — run L, recorder panic at the M33 `dup2` assert (`lib.rs:1140:17`); no replay ran
- `csh.O.rec.err` — run O, recorder panic at the M33 `dup2` assert (`lib.rs:1140:17`); no replay ran
- `dddiagnose.I.rec.err` — run I, the identical malloc crash (`guest crashed: pc=0x180302eb0 far=0x2000050050 esr=0x92000045`) after the serviced refusal; 11 self-pid `ESRCH` in the trace
- `dddiagnose.I.rp.err` — run I, the replay's identical crash line (`rc=139` both sides)
- `dddiagnose.L.rec.err` — run L, the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid `ESRCH`
- `dddiagnose.L.rp.err` — run L, replay ran out of events at the same landmark (`expected recorded syscall, got None`)
- `dddiagnose.O.rec.err` — run O, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 12 self-pid `ESRCH`
- `dddiagnose.O.rp.err` — run O, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `desdp.I.rec.err` — run I, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `desdp.I.rp.err` — run I, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `desdp.L.rec.err` — run L, the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid `ESRCH`
- `desdp.L.rp.err` — run L, replay ran out of events at the same landmark (`expected recorded syscall, got None`)
- `desdp.O.rec.err` — run O, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `desdp.O.rp.err` — run O, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `dyld_info.I.rec.err` — run I, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `dyld_info.I.rp.err` — run I, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `dyld_info.L.rec.err` — run L, the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid `ESRCH`
- `dyld_info.L.rp.err` — run L, replay ran out of events at the same landmark (`expected recorded syscall, got None`)
- `dyld_info.O.rec.err` — run O, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `dyld_info.O.rp.err` — run O, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `flex.I.rec.err` — run I, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `flex.I.rp.err` — run I, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `flex.L.rec.err` — run L, the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid `ESRCH`
- `flex.L.rp.err` — run L, replay ran out of events at the same landmark (`expected recorded syscall, got None`)
- `flex.O.rec.err` — run O, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `flex.O.rp.err` — run O, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `launchctl.I.rec.err` — run I, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `launchctl.I.rp.err` — run I, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `launchctl.L.rec.err` — run L, the RCV-shaped `mach_msg2` wall (`options 0x404000102`, `pc 0x1804adc34`) after the serviced refusal; 0 self-pid `ESRCH`
- `launchctl.L.rp.err` — run L, replay ran out of events at the same landmark (`expected recorded syscall, got None`)
- `launchctl.O.rec.err` — run O, the `brk` (`EC=0x3c pc=0x18035f084 elr=0x1804af110`) after the serviced refusal; 11 self-pid `ESRCH`
- `launchctl.O.rp.err` — run O, replay re-reports the same exception at the same pc as its `DIVERGENCE`
- `tcsh.I.rec.err` — run I, recorder panic at the M33 `dup2` assert (`lib.rs:1140:17`); no replay ran
- `tcsh.L.rec.err` — run L, recorder panic at the M33 `dup2` assert (`lib.rs:1140:17`); no replay ran
- `tcsh.O.rec.err` — run O, recorder panic at the M33 `dup2` assert (`lib.rs:1140:17`); no replay ran
- `yes.I.rec.err` — run I, recorder stderr up to the 30 s SIGKILL; no `RECORD ERROR`, no replay ran
- `yes.L.rec.err` — run L, recorder stderr up to the 30 s SIGKILL; no `RECORD ERROR`, no replay ran
- `yes.O.rec.err` — run O, recorder stderr up to the 30 s SIGKILL; no `RECORD ERROR`, no replay ran
