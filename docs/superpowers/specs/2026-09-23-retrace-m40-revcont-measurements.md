# M40-revcont — t0 measurements: why rung 8's `reverse-continue` costs hours

**Date:** 2026-09-23. **Tree:** `786bf2b` (the M39 merge) plus throwaway instrumentation that was
never committed — the `[prof]` counters in `cmd_reverse_continue` and `checkpointed_seek`, and the
probe file `crates/retrace/tests/m40_probe.rs`. Both are preserved verbatim, with every log cited
below, in the ledger at `.superpowers/sdd/2026-09-23-retrace-m40-revcont/t0/`.

The design spec (`2026-09-23-retrace-m40-revcont-design.md`) cites this document and nothing else
for a measured claim.

## Conditions

- Apple M4 Pro, 12 cores, 24 GB RAM, swap empty at the start (the machine was booted at 20:16).
- **The machine was shared.** Another project's `cargo-mutants` run held the load average at
  **40** on 12 cores (`uptime` at 20:55; the two heaviest processes each at ~570 % CPU). The
  debugger got about half a core (`ps` %CPU 47). So **every duration below is CPU seconds** from
  `ps cputime` or `/usr/bin/time`, never wall-clock. CPU seconds hold up under that load much better
  than wall-clock does, though not perfectly: a contended process can land on efficiency cores. The
  **counts** (sessions, seeks, hits, steps) are exact, and the conclusions rest on them.
- The build is the `dev` profile (opt-level 0, no `[profile]` section in `Cargo.toml`). That is the
  configuration the gate, the e2e tests and M39's demo all use.
- Every `reverse-continue` run was capped by `t0/prof.sh`, a watchdog that kills the debugger at a
  time or RSS ceiling. M39 R15 showed that an uncapped run can exhaust the machine's swap, and the
  machine was shared.

## M1 — the recording

`record-dyn` of Homebrew Python 3.14 on `crates/retrace-guest/py/crash.py`: exit 139, marker
`CRASHPY cell=0xa01722ac8 target=0x4000dead0000 rows=3`, crash `pc=0xa01826e60
far=0x4000dead0000 esr=0x92000005`. 10.65 s user. The trace is **97,619,602 bytes, 1,146 events**.
The debugger's `continue` parks at the crash at **P = (1144, 2470)**. `replay`: exit 139, 11.06 s
user, peak footprint 303 MB. (`t0/record.*`, `t0/replay.*`, `t0/rc1.err` line 4.)

## M2 — the capped `reverse-continue` profile (`rc1`)

Script `continue; watch 0xa01722ac8 8; reverse-continue`, cap 900 s / 6 GB. Killed at the time cap
after **448 s CPU, 19 iterations, 41 seeks**; RSS 187 MB → 2.84 GB. (`t0/rc1.mem`, `t0/rc1.err`.)

- **Every one of the 19 iterations reports the same writer, `pc=0x1804fb414`, in window 1126.**
  Successive K values are mostly **6 apart**: 29627, 29633, 29639, 29645, 29651, 29657, then 47052,
  49233, 49239 … 49275, 65274, 65280, 65764. `resolve_steps` is 5 on most iterations.
- **About 20.8 s CPU per iteration** (iterations 1→19 over the `cputime` samples).
- Each iteration opens **two sessions**: `resolve_hit_k`'s seek, then the next scan's seek. Before
  the checkpoint cache warmed, both were cold.

## M3 — where a session's time goes (`t0/rc1.sample`)

`sample` for 5 s at +120 s, taken during a cold `seek=(1,0)`. All **3,842** samples are under
`checkpointed_seek → ReplaySession::open → retrace_trace::Reader::open_checked`: **2,451 (64 %)** in
`crc32`, a bit-at-a-time loop whose unoptimised `Range` iteration dominates, and **1,391 (36 %)**
in `bincode::deserialize` of `Event`s. `ReplaySession::open` and `ReplaySession::from_checkpoint`
both call `open_checked`, so every session, cold or from a checkpoint, re-reads and re-checks the
whole 97.6 MB trace.

## M4 — where the memory goes (`t0/rc1.vmmap`, `t0/rc1.vmmap2`)

| | at +120 s (4 sessions) | at the kill (41 seeks) |
|---|---|---|
| Physical footprint | 366.3 MB | 2.6 GB |
| `VM_ALLOCATE` | 168.7 MB in 5,051 regions | 2.3 GB in 68,967 regions |

That is about **55 MB and ~1,580 anonymous mappings per session**. The code explains it:
`alloc_pages` (`crates/retrace-box/src/lib.rs:976`) `mmap`s every guest backing, and `Box_` has no
`Drop`. The only `munmap`s are `unmap_overlapping`, `guest_munmap` and `place_fixed`'s case-2
temporary, so a dropped `Box_` releases the VM (`hv_vm_destroy`) and never its host memory. The
`CheckpointCache`'s 256 MiB budget is not where the memory is. `MALLOC_*` stays around 300 MB.

## M5 — the real hits: one native pass (`t0/probe.err`, probe A)

One `ReplaySession` from landmark 1, the watch armed, `advance()` to the end. At each
`Advance::Watch` the probe stepped over the hit in place (`clear_watchpoints`, `step_insns(1)`,
`arm_watchpoints`) and recorded it:

| landmark | pc | FAR | value at the cell after the store |
|---|---|---|---|
| 1126 | `0x1804fb414` | `0xa01722ac0` | `0x701238000` |
| 1136 | `0x1804fb414` | `0xa01722ac0` | `0x0bd36e903a8a3a8f` |
| 1143 | `0x1804fb100` | `0xa01722ac0` | `0` |
| 1143 | `0x1804fb11c` | `0xa01722ac0` | `0` |
| 1143 | `0xa0182245c` | `0xa01722ac8` | `0x4000dead0000` ← the store of the bad pointer |

**Five instruction hits, zero syscall hits.** The whole pass, including the trace decode, took
**11.55 s user** at 308 MB maximum RSS. Four of the five FARs are `0xa01722ac0`, the base of a wider
store that covers the watched `0xa01722ac8`.

## M6 — stepping with the watch armed is an exact resolver (`t0/probeB.err`, probe B)

`seek(1126, 0)`, arm the watch, then `step_insns(1)` in a loop:

- Every non-hitting instruction retired normally. The loop stopped at **K = 1,765,682**,
  `pc=0x1804fb414`, with `EC=0x34 ISS=0x62 FSC=0x22 far=0xa01722ac0`. That is a watchpoint
  exception, delivered **pre-retire**: the cell read `0` at the stop.
- With the watch disarmed, one more step retired the store, and the cell read **`0x701238000`**,
  M5's value. The pc moved to `0x1804fb418`.
- With the watch re-armed, 1,000 further steps retired cleanly.
- **The store pc executed 183 times** in window 1126 before and at the real write.
- 12.70 s user for the whole run, including a cold seek. Stepping 1.77 M instructions is cheap
  compared with a session open.

`arm_hw_watchpoint`'s doc says watchpoints are armed "NEVER while single-stepping". That rule was
written by analogy with breakpoints, whose pre-retire fire at the current pc would repeat forever. It
had never been measured for watchpoints, and this measurement shows the watchpoint case is clean.

## M7 — forward `continue` names the wrong instruction (`t0/fwd1.out`)

Script `watch 0xa01722ac8 8; continue; x 0xa01722ac8 8; stepi; x 0xa01722ac8 8; where`:

```
hit watch 0xa01722ac0 (write at 0x1804fb414) at (1126, +?)
resolved (1126, 29627)
0xa01722ac8: 00 00 00 00 00 00 00 00
0xa01722ac8: 00 00 00 00 00 00 00 00        ← after stepi: the "hit" instruction did not write it
at (1126, 29628) pc=0x1804fb418 thread=0
```

The real write is at (1126, 1,765,682) (M6). `continue` parked 1,736,055 instructions early, on an
iteration of the same store instruction that wrote a different address. It also printed
`0xa01722ac0` as the watched address, not the watched `0xa01722ac8`.

## What the measurements mean (inference, labelled)

- `resolve_hit_k` (`crates/retrace/src/debug.rs:224`) returns the first K at or after `from_k` whose
  pc equals the hit's pc. A watch hit is identified by **address**, not by pc. When the writing
  instruction runs on other addresses first, which is what a `memset`-style loop does, the resolver
  lands on an earlier run of it.
- `cmd_reverse_continue` then resumes from that wrong K + 1, re-finds the **same** real hit, and
  resolves one run of the store further on. So it pays one full iteration per *run of the store
  instruction* between each real hit and the previous resume point, not one per real hit. Its final
  answer stays correct, because the last resolution always lands on the real write, but window 1126
  alone contributes **182 false iterations** (M6), and windows 1136 and 1143 add more.
- At ~20.8 s CPU per iteration under this load (two sessions, each re-decoding the trace, M3), and
  ~55 MB leaked per session (M4), that accounts for M39's 3.42 h and its swap exhaustion. The exact
  iteration count of a full run was not measured, because the run was capped.
- `watch_cli`'s fixture, `WATCHLOOP`, runs its store instruction **only** at the watched address
  (the test itself calls it "the (single) store pc"), so pc and address agreed. That is why the
  resolver's error went unseen from M5 to here. The other watch transcripts (`thread_watch_e2e`,
  `crashy_*`, `reverse_debug_e2e`) were not examined for this; the gate will show whether any of
  their pinned coordinates move.
