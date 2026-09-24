# M42-llsc: t0 measurements

**Date:** 2026-09-24.

**Tree:** `main` at `1d95a93` (the M41 merge), in the `m42-llsc` worktree, before any change.

**Build:** a dev build from `cargo build -p retrace`. It was copied to the scratchpad and ad-hoc
signed with `retrace.entitlements`, as `crates/retrace/tests/util/mod.rs::bin()` does.

**Evidence:** every transcript below is `retrace debug --script` output from that binary, copied
verbatim. The only edit is that the `/usr/bin/time -l` block is reduced to its CPU line. Nothing
here is inferred from reading code unless it says so.

**The fixture.** `llsc` is a scratch guest. It is not a repo fixture; M42's Task 1 adds the real
one. The source is `llsc.s` in the session scratchpad (`m42/t0/`). It was built by hand with
`build.rs`'s asm flags, `clang -arch arm64 -nostdlib -static -Wl,-e,_start`. It runs four LL/SC
shapes in sequence. Each shape is followed by a syscall whose **arguments** carry its result, so
the divergence oracle, which compares `(num, args[0..8])`, names any difference at that landmark:

- `x3` is the cell after the shape;
- `x4` counts how many times the retry loop was **entered**;
- `x5` is a second value, where there is one.

`write(2)` ignores x3–x5; they are there for the oracle.

| Shape | Window | Code (`otool -tv`) | Coordinates in the recording |
|---|---|---|---|
| (a) discard-status, dyld `getpid` style. If the cell is still 0 after the pair, the guest issues an extra `getpid` (20) | 1 → `write "a"` | `ldxr w10,[x9]` @`0x10000038c` (`885f7d2a`); `cbnz` @`…390`; `stxr wzr,w0,[x9]` @`0x100000394` (`881f7d20`, the same two words as dyld's); `ldr`/`cbnz` @`…398`/`…39c`; the `getpid` path @`…3a0`/`…3a4`; `a_report` @`0x1000003a8`; write `svc` @`…3c8` | LDX K=3, STX K=5; window length 16 |
| (b) retry loop, three increments | 2 → `write "b"` | `b_retry: add x20` @`0x1000003dc`; `ldaxr x1,[x0]` @`0x1000003e0` (`c85ffc01`); `add`; `stlxr w2,x1,[x0]` @`0x1000003e8` (`c802fc01`); `cbnz w2` @`…3ec`; `subs`/`b.ne` @`…3f0`/`…3f4`; `b_done` @`0x1000003f8`; `svc` @`…418` | stlxr at K = 7, 14, 21; window length 33 |
| (c) CAS: `if (*cas == 5) *cas = 9` | 3 → `write "c"` | `c_retry` @`0x100000430`; `ldaxr` @`0x100000434`; `cmp`; `b.ne`; `stlxr w2,x4,[x0]` @`0x100000440` (`c802fc04`); `cbnz` @`…444`; `svc` @`…468` | LDX K=6, STX K=9; length 19 |
| (d) pair: `+= 10`, `+= 20` | 4 → `write "d"` | `d_retry` @`0x100000478`; `ldxp x1,x2,[x0]` @`0x10000047c` (`c87f0801`); two `add`s; `stxp w3,x4,x5,[x0]` @`0x100000488` (`c8231404`); `cbnz` @`…48c`; `svc` @`…4ac` | LDX K=4, STX K=7; length 16 |
| exit | 5 | `mov x0,#0; mov x16,#1; svc` @`…4b0`–`…4b8` | length 2 |

**The cells**, each in its own 64-byte block:

| Cell | Address | Initial value |
|---|---|---|
| `cella` | `0x100004000` | 0 (4 bytes) |
| `ctr` | `0x100004040` | 0 |
| `cas` | `0x100004080` | 5 |
| `pair` | `0x1000040c0` | {1, 2} |

**Windows (n, 0).** Each window's first instruction is the one after the previous `svc`:
`(2,0)` = `0x1000003cc`, `(3,0)` = `0x10000041c`, `(4,0)` = `0x10000046c`. The debugger reaches
`(n, 0)` with `break <that pc>; continue`, which is the boundary check, and then `delete`s the
breakpoint.

**The recording**, made fresh for t0 in the scratchpad and not committed, was
`RETRACE_TRACE=1 retrace record llsc -o llsc.bin`. Its landmarks:

```
[trap] num=4 (0x4) pc=0x1000003cc args=[0x1,0x100004100,0x2,0x4242,0x0,0x0]
[trap] num=4 (0x4) pc=0x10000041c args=[0x1,0x100004102,0x2,0x3,0x3,0x0]
[trap] num=4 (0x4) pc=0x10000046c args=[0x1,0x100004104,0x2,0x9,0x1,0x0]
[trap] num=4 (0x4) pc=0x1000004b0 args=[0x1,0x100004106,0x2,0xb,0x1,0x16]
[trap] num=1 (0x1) pc=0x1000004bc args=[0x0,0x100004106,0x2,0xb,0x1,0x16]
```

Natively, then: (a) filled the cell with `0x4242` and issued no `getpid`. (b) counted to 3 with 3
entries, so no `stlxr` failed. (c) stored 9 in 1 entry. (d) stored {11, 22} in 1 entry.

**How the runs were bounded.**

- Every run was `/usr/bin/time -l perl -e 'alarm 60; exec @ARGV' retrace …`. There is no
  `timeout` or `gtimeout` on this machine.
- **exit=142** is 128 + SIGALRM: the run was killed at the 60 s bound. **exit=5** is a
  `DEBUG ERROR`.
- To see where a killed run was spinning, the same script was re-run and sampled with `sample
  <pid> 2` after 8 s. The frames quoted are the retrace frames of that sample.
- Full logs, including every `time -l` block, are in the scratchpad at `m42/t0/logs/`.

## Summary

| # | What | Result |
|---|---|---|
| M1 | record, replay ×2, `continue` | **pass**: exit 0 each time, byte-identical output |
| M2 | `stepi` past (a)'s `ldxr`, then `continue` | **diverged**, loud: an extra `getpid` at landmark 1 |
| M3 | (b) under stepping | `stepi 1000000`: **livelock bounded by the count**; the counter stays 0 and the pc stays in the loop. Forward `continue` to a breakpoint after the loop, `reverse-continue` to it, and `reverse-stepi` into window 2: all three **hang**. `reverse-continue` whose phase 2 steps window 2: **silent wrong answer** |
| M4 | `break` on (a)'s `stxr`; `continue` ×2 | **diverged**, loud. The same breakpoint from `reverse-continue` also diverges. A breakpoint on (b)'s `stlxr` gives **phantom hits forward** and a **hang backward** |
| M5 | `watch` (b)'s counter; `continue` ×N | **phantom hits, never ending, silent** (exit 0), at coordinates past the window's real length. `reverse-continue`: **hang** |
| M6 | the same for (c), (d) and (a) | (c) and (d): phantom hits forward, hang backward. (a): diverged in both directions |
| M7 | `stepi K; continue` at every K in each pair | Every K from LDX+1 through STX+1 **diverges**, and the LDX's own K passes, in all four shapes. No hang |
| M8 | the step exit's syndrome at each load-exclusive (added while writing the spec) | ISS.ISV = 1 on every retire; ISS.EX = 1 exactly on the four load-exclusive retires |

---

## M1: native record and replay are clean

```
$ retrace record llsc -o llsc.bin
a
b
c
d
[retrace] fall-throughs: 0
exit=0                                  (0.02 user, 0.00 sys)

$ retrace replay llsc.bin
a
b
c
d
[retrace] fall-throughs: 0
exit=0                                  (0.02 user, 0.00 sys; a second replay identical)

$ retrace debug llsc.bin --script 'continue; where'
> continue
exited (code 0)
> where
at (5, 2) pc=0x1000004b8 thread=0  in d_done+0x28
exit=0
```

## M2: step past (a)'s `ldxr`, then `continue`: diverges

```
> stepi 3
> where
at (1, 3) pc=0x10000038c thread=0  in a_ldx
> stepi
> where
at (1, 4) pc=0x100000390 thread=0  in a_ldx+0x4
> x 0x100004000 4
0x100004000: 00 00 00 00
> continue
DEBUG ERROR: continue diverged at landmark 1 pc 0x1000003a8: syscall mismatch: live (num=20, args=[16962, 0, 0, 0, 0, 0, 0, 0]) != recorded (num=4, args=[1, 4294983936, 2, 16962, 0, 0, 0, 0])
exit=5
```

`stxr` ran natively after a stepped `ldxr`, and it failed. The cell stayed 0, so the guest took the
`getpid` path, which the recording never took. This is M41's Q3, reproduced on a fixture that
retrace owns. Control: `stepi 3; continue` (standing ON the `ldxr`, not yet run) exits 0 (M7).

## M3: (b)'s retry loop under stepping

### M3a: `stepi` by count livelocks, but the count bounds it

```
> break 0x1000003cc
breakpoint at 0x1000003cc (a_report+0x24)
> continue
hit 0x1000003cc at (2, 0)  in a_report+0x24
> delete 0x1000003cc
deleted 0x1000003cc
> stepi 33
> where
at (2, 33) pc=0x1000003ec thread=0  in b_stx+0x4
> x 0x100004040 8
0x100004040: 00 00 00 00 00 00 00 00
> stepi 1000000
> where
at (2, 1000033) pc=0x1000003ec thread=0  in b_stx+0x4
> x 0x100004040 8
0x100004040: 00 00 00 00 00 00 00 00
> continue
DEBUG ERROR: continue diverged at landmark 2 pc 0x10000041c: syscall mismatch: live (num=4, args=[1, 4294983938, 2, 3, 200009, 0, 0, 0]) != recorded (num=4, args=[1, 4294983938, 2, 3, 3, 0, 0, 0])
exit=5                                  (0.96 user, 0.19 sys)
```

**Every stepped `stlxr` fails.** Window 2 is 33 instructions in the recording. After 1,000,033
stepped instructions the counter is still 0 and the pc is still in the loop. `stepi` can't hang,
because `step_insns(k)` stops after k steps. The final `continue` runs natively: the counter ends
correct at 3, and x4 reports 200,009 loop entries against the recorded 3. The cost was about
1.15 µs of CPU per step.

### M3b: a forward `continue` to a breakpoint after the loop hangs

```
> break 0x1000003f8
breakpoint at 0x1000003f8 (b_done)
> continue
hit 0x1000003f8 at (2, +?)  in b_done
exit=142                                (killed at 60 s; 50.73 user, 9.23 sys)
```

- The native scan finds the breakpoint.
- `resolve_nth` then single-steps window 2 from K = 0, looking for `0x1000003f8`, and the stepped
  path never leaves the loop.
- Sample: `cmd_continue → resolve_nth → ReplaySession::step_insns → Box_::step → run_one_for_step`
  (1423 of 1540 samples under `resolve_nth`).
- **No step touches the pair itself.** The breakpoint sits after the loop, and the scan ran
  natively. The single-stepping that resolves a native hit is the whole exposure.

### M3c: `reverse-continue` from the exit to the same breakpoint hangs

```
> continue
exited (code 0)
> break 0x1000003f8
breakpoint at 0x1000003f8 (b_done)
> reverse-continue
exit=142                                (killed at 60 s; 50.79 user, 9.17 sys)
```

Sample: `cmd_reverse_continue → resolve_nth → step_insns → Box_::step` (1420 of 1539). Phase 1
counted the hit natively. The resolver hangs exactly as in M3b.

### M3d: `reverse-stepi` from (3, 0) into window 2 hangs

```
> break 0x10000041c
breakpoint at 0x10000041c (b_done+0x24)
> continue
hit 0x10000041c at (3, 0)  in b_done+0x24
> delete 0x10000041c
deleted 0x10000041c
> reverse-stepi
exit=142                                (killed at 60 s; 49.98 user, 9.96 sys)
```

Sample: `cmd_reverse_stepi → probe_window_len → CheckpointCache::window_len →
ReplaySession::window_len_here → Box_::step` (1523 of 1539). The window-length probe steps to a
trap that the stepped path never reaches.

### M3e: `reverse-continue` whose phase 2 steps window 2: a silent wrong answer

```
> break 0x1000003cc
breakpoint at 0x1000003cc (a_report+0x24)
> continue
hit 0x1000003cc at (2, 0)  in a_report+0x24
> delete 0x1000003cc
deleted 0x1000003cc
> stepi 30
> where
at (2, 30) pc=0x1000003e0 thread=0  in b_ldx
> break 0x1000003f8
breakpoint at 0x1000003f8 (b_done)
> reverse-continue
no earlier hit
> where
at (2, 30) pc=0x1000003e0 thread=0  in b_ldx
exit=0
```

- In the recording, `(2, 30)` is `0x10000040c`, and `b_done` ran at `(2, 25)`. The right answer is
  therefore `hit 0x1000003f8 at (2, 25)`.
- Phase 2 steps only `pk` = 30 instructions, so it terminates. But those 30 steps never leave the
  loop, and it reports "no earlier hit" with exit 0.
- The coordinate `(2, 30)` that `stepi 30` reports is itself a pc the recording never had at
  `(2, 30)`.

## M4: `break` on a store-exclusive

### M4a: `break` on (a)'s `stxr`, `continue` ×2

```
> break 0x100000394
breakpoint at 0x100000394 (a_stx)
> continue
hit 0x100000394 at (1, +?)  in a_stx
resolved (1, 5)
> where
at (1, 5) pc=0x100000394 thread=0  in a_stx
> x 0x100004000 4
0x100004000: 00 00 00 00
> continue
DEBUG ERROR: continue diverged at landmark 1 pc 0x1000003a8: syscall mismatch: live (num=20, args=[16962, 0, 0, 0, 0, 0, 0, 0]) != recorded (num=4, args=[1, 4294983936, 2, 16962, 0, 0, 0, 0])
exit=5
```

The first `continue` is right: `(1, 5)` is the recorded coordinate. The session it parks on was
reached by stepping, though (`reseek`), so the `ldxr` was stepped. The second `continue` finishes
`(1, 5)` by stepping the `stxr`, which fails.

### M4b: the same breakpoint, from `reverse-continue`

```
> continue
exited (code 0)
> break 0x100000394
breakpoint at 0x100000394 (a_stx)
> reverse-continue
DEBUG ERROR: reverse-continue diverged: syscall mismatch: live (num=20, args=[16962, 0, 0, 0, 0, 0, 0, 0]) != recorded (num=4, args=[1, 4294983936, 2, 16962, 0, 0, 0, 0])
exit=5
```

This is E2 in its pure form. Phase 1's **native** scan stops on the `stxr`, and the `ldxr` ran
natively too. Phase 1 then steps off the stop in the **same** session (`step_watched`), and the
store fails. No seek is involved.

### M4c: a breakpoint just after (a): the resolver fails loud

```
> break 0x1000003a8
breakpoint at 0x1000003a8 (a_report)
> continue
hit 0x1000003a8 at (1, +?)  in a_report
DEBUG ERROR: resolve breakpoint hit #1 in window 1: the window ended after 0 hit(s) at K=9
exit=5
```

The resolver's stepped path takes `getpid` and reaches that syscall's `svc` at K = 9. In the
recording, window 1 is 16 instructions long.

### M4d and M4e: `break` on (b)'s `stlxr`: phantom hits forward, a hang backward

```
> break 0x1000003e8
breakpoint at 0x1000003e8 (b_stx)
> continue
hit 0x1000003e8 at (2, +?)  in b_stx
resolved (2, 7)
> continue
hit 0x1000003e8 at (2, +?)  in b_stx
resolved (2, 12)
> continue
hit 0x1000003e8 at (2, +?)  in b_stx
resolved (2, 17)
> continue
hit 0x1000003e8 at (2, +?)  in b_stx
resolved (2, 22)
> continue
hit 0x1000003e8 at (2, +?)  in b_stx
resolved (2, 27)
> where
at (2, 27) pc=0x1000003e8 thread=0  in b_stx
> x 0x100004040 8
0x100004040: 00 00 00 00 00 00 00 00
exit=0
```

The recorded hits are at `(2, 7)`, `(2, 14)` and `(2, 21)`. Everything after the first is a
phantom, spaced one loop iteration (5 instructions) apart, and the counter never moves. Backward:

```
> continue
exited (code 0)
> break 0x1000003e8
breakpoint at 0x1000003e8 (b_stx)
> reverse-continue
exit=142                                (killed at 60 s; 48.06 user, 11.87 sys)
```

Sample: `cmd_reverse_continue → step_watched` (591 samples) interleaved with `advance → Box_::run`
(430). That is phase 1's `Break` arm:

1. the native stop at the `stlxr`;
2. a stepped step-off, where the `stlxr` fails;
3. re-arm, a native retry, and the stop fires again.

This is the phase-1 phantom-hit livelock predicted for E2/E3. It runs entirely inside one session
at native speed.

## M5: `watch` (b)'s counter

### M5a/b: `continue` ×10: phantom hits, never ending, silent

```
> watch 0x100004040 8
watch at 0x100004040 len 8
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 7)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 12)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 17)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 22)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 27)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 32)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 37)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 42)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 47)
> continue
hit watch 0x100004040 (write at 0x1000003e8) at (2, +?)  in b_stx
resolved (2, 52)
> where
at (2, 52) pc=0x1000003e8 thread=0  in b_stx
> x 0x100004040 8
0x100004040: 00 00 00 00 00 00 00 00
exit=0
```

The recorded writes are at `(2, 7)`, `(2, 14)` and `(2, 21)`, taking the counter 0 → 1 → 2 → 3.
Here only `(2, 7)` is right. From `(2, 37)` on, the coordinates are **past the window's real length
of 33**, a position that does not exist in the recording. The whole run exits 0. M5a was a separate run: five `continue`s with an
`x` of the counter between them. It gave the same first five hits and the counter at 0 each time.
The scratchpad logs have both.

**A hardware fact this shows.** The watchpoint **fires pre-retire on a `stlxr` whose store is going
to fail**. The resolver steps the `ldaxr`, which clears the monitor (M3a), and still gets
`Stepped::Watch` at the `stlxr`. Every phantom above is exactly that. So when the hardware raises
the watch stop, it does not check the exclusive monitor first.

### M5c: `reverse-continue` from the exit with that watch: hang

```
> continue
exited (code 0)
> watch 0x100004040 8
watch at 0x100004040 len 8
> reverse-continue
exit=142                                (killed at 60 s; 49.45 user, 10.49 sys)
```

Sample: `cmd_reverse_continue → step_insns` (532) interleaved with `advance → Box_::run` (416).
That is phase 1's `Watch` arm:

1. the native watch stop at the `stlxr` (E3);
2. `step_insns(1)` over it, where the store fails;
3. re-arm, a native retry, and the watch fires again.

## M6: the same watches on (c), (d) and (a)

(c), the CAS cell, forward:

```
> watch 0x100004080 8
watch at 0x100004080 len 8
> continue
hit watch 0x100004080 (write at 0x100000440) at (3, +?)  in c_stx
resolved (3, 9)
> x 0x100004080 8
0x100004080: 05 00 00 00 00 00 00 00
> continue
hit watch 0x100004080 (write at 0x100000440) at (3, +?)  in c_stx
resolved (3, 15)
> continue
hit watch 0x100004080 (write at 0x100000440) at (3, +?)  in c_stx
resolved (3, 21)
> continue
hit watch 0x100004080 (write at 0x100000440) at (3, +?)  in c_stx
resolved (3, 27)
> where
at (3, 27) pc=0x100000440 thread=0  in c_stx
> x 0x100004080 8
0x100004080: 05 00 00 00 00 00 00 00
exit=0
```

(d), the pair's first doubleword, forward:

```
> watch 0x1000040c0 8
watch at 0x1000040c0 len 8
> continue
hit watch 0x1000040c0 (write at 0x100000488) at (4, +?)  in d_stx
resolved (4, 7)
> x 0x1000040c0 16
0x1000040c0: 01 00 00 00 00 00 00 00 02 00 00 00 00 00 00 00
> continue
hit watch 0x1000040c0 (write at 0x100000488) at (4, +?)  in d_stx
resolved (4, 13)
> continue
hit watch 0x1000040c0 (write at 0x100000488) at (4, +?)  in d_stx
resolved (4, 19)
> continue
hit watch 0x1000040c0 (write at 0x100000488) at (4, +?)  in d_stx
resolved (4, 25)
> where
at (4, 25) pc=0x100000488 thread=0  in d_stx
> x 0x1000040c0 16
0x1000040c0: 01 00 00 00 00 00 00 00 02 00 00 00 00 00 00 00
exit=0
```

Each cell is written **once** in the recording, at `(3, 9)` and `(4, 7)`. Every later hit is a
phantom, one 6-instruction iteration apart, and each cell keeps its old value.

Backward, `continue; watch <cell> 8; reverse-continue`:

| Cell | Result | CPU |
|---|---|---|
| (c) `0x100004080` | `exit=142`, killed at 60 s | 49.55 user, 10.40 sys |
| (d) `0x1000040c0` | `exit=142`, killed at 60 s | 49.55 user, 10.40 sys |

(a), the discard-status cell. Its STX is not retried, so the result is a divergence, not a
livelock:

```
> watch 0x100004000 4
watch at 0x100004000 len 4
> continue
hit watch 0x100004000 (write at 0x100000394) at (1, +?)  in a_stx
resolved (1, 5)
> continue
DEBUG ERROR: continue diverged at landmark 1 pc 0x1000003a8: syscall mismatch: live (num=20, args=[16962, 0, 0, 0, 0, 0, 0, 0]) != recorded (num=4, args=[1, 4294983936, 2, 16962, 0, 0, 0, 0])
exit=5

> continue
exited (code 0)
> watch 0x100004000 4
watch at 0x100004000 len 4
> reverse-continue
DEBUG ERROR: reverse-continue diverged: syscall mismatch: live (num=20, args=[16962, 0, 0, 0, 0, 0, 0, 0]) != recorded (num=4, args=[1, 4294983936, 2, 16962, 0, 0, 0, 0])
exit=5
```

## M7: seek into each pair, then `continue`

**Script.** For shape (a): `stepi K; where; continue` from `(1, 0)`. For the other three:
`break <(n,0) pc>; continue; delete <pc>; stepi K; where; continue`.

**Every run cost 0.02 s user, 0.00 s sys.** None hung: `stepi K` is bounded, and the native
`continue` retries and completes.

| Shape | K | Where | `continue` |
|---|---|---|---|
| (a) | 3 | `0x10000038c` `a_ldx` | `exited (code 0)` |
| (a) | 4 | `0x100000390` | **diverged**: live `getpid` (20) ≠ recorded `write` (4), landmark 1 |
| (a) | 5 | `0x100000394` `a_stx` | **diverged**, the same |
| (a) | 6 | `0x100000398` | **diverged**, the same |
| (b) | 4 | `0x1000003dc` `b_retry` | `exited (code 0)` |
| (b) | 5 | `0x1000003e0` `b_ldx` | `exited (code 0)` |
| (b) | 6 | `0x1000003e4` | **diverged** at landmark 2: `args[4]` 4 ≠ 3, with `args[3]` (the counter) 3 = 3 |
| (b) | 7 | `0x1000003e8` `b_stx` | **diverged**, the same |
| (b) | 8 | `0x1000003ec` | **diverged**, the same |
| (c) | 6 | `0x100000434` `c_ldx` | `exited (code 0)` |
| (c) | 7, 8, 9, 10 | `…438`, `…43c`, `…440` `c_stx`, `…444` | **diverged** at landmark 3: `args[4]` 2 ≠ 1, with `args[3]` 9 = 9 |
| (d) | 4 | `0x10000047c` `d_ldx` | `exited (code 0)` |
| (d) | 5, 6, 7, 8 | `…480`, `…484`, `…488` `d_stx`, `…48c` | **diverged** at landmark 4: `args[4]` 2 ≠ 1, with `args[3]`/`args[5]` 11/22 = 11/22 |

**The diverging set is exactly the positions after the LDX has retired, up to and including the
one after the STX:** K from LDX+1 to STX+1, in all four shapes.

Standing on the LDX is safe, and so is the retry target before it.

In (b), (c) and (d) **the values come out right**. The native retry repairs them, and only the
loop-entry count in `x4` shows the lost store. Without that counter in the arguments, those rows
would exit 0. The retry would still add one iteration, which shifts every later K in that window.

**Two cross-checks** go through `checkpointed_seek` (`reverse-stepi` reseeks) rather than
`stepi`. The first confirms that a real seek into the pair is the same as stepping into it:

```
> stepi 8
> reverse-stepi 3
> where
at (1, 5) pc=0x100000394 thread=0  in a_stx
> continue
DEBUG ERROR: continue diverged at landmark 1 pc 0x1000003a8: syscall mismatch: live (num=20, args=[16962, 0, 0, 0, 0, 0, 0, 0]) != recorded (num=4, args=[1, 4294983936, 2, 16962, 0, 0, 0, 0])
exit=5
```

The second measures a window length on the wrong path, and lands `reverse-stepi` on an instruction
the recording never executed:

```
> break 0x1000003cc
breakpoint at 0x1000003cc (a_report+0x24)
> continue
hit 0x1000003cc at (2, 0)  in a_report+0x24
> delete 0x1000003cc
deleted 0x1000003cc
> reverse-stepi
> where
at (1, 9) pc=0x1000003a4 thread=0  in a_after+0xc
> x 0x100004000 4
0x100004000: 00 00 00 00
> continue
DEBUG ERROR: continue diverged at landmark 1 pc 0x1000003a8: syscall mismatch: live (num=20, args=[16962, 0, 0, 0, 0, 0, 0, 0]) != recorded (num=4, args=[1, 4294983936, 2, 16962, 0, 0, 0, 0])
exit=5
```

- In the recording, window 1 is **16** instructions, and `reverse-stepi` from `(2, 0)` should park
  at `(1, 16)` on the `write`'s `svc` at `0x1000003c8`.
- `window_len` stepped window 1, took the `getpid` path, and memoized **9**. So `reverse-stepi`
  parked on `getpid`'s `svc` at `0x1000003a4`.
- The `where` line is silent. Only the following `continue` is loud.
- A related run, `stepi 8; where; stepi 2`, shows the same wrong length from the other side:
  `(1, 8)` is pc `0x1000003a0` (recorded: `0x1000003a8`), and `stepi 2` reports
  `error: window 1 ends after 1 instruction(s)`.

## M8 (added while writing the spec): the step exit reports a load-exclusive

Taken by the controller after t0 closed, on the same tree plus one uncommitted, since-reverted
`eprintln!` of `e.syndrome` in `run_one_for_step`'s EL0-retire arm (`crates/retrace-box/src/lib.rs`,
the `(cpsr >> 2) & 3 == 0` branch). Logs: scratchpad `m42/t0/probe-ex*.log`, script `probe-ex.sh`.

```
$ retrace debug llsc.bin --script 'stepi 6'
syndrome=0xcb000022 newpc=0x100000384
syndrome=0xcb000022 newpc=0x100000388
syndrome=0xcb000022 newpc=0x10000038c
syndrome=0xcb000062 newpc=0x100000390     <- (a)'s ldxr w10,[x9] retired
syndrome=0xcb000022 newpc=0x100000394
syndrome=0xcb000022 newpc=0x100000398
```

Stepping one instruction from each other shape's load-exclusive (`break <ldx>; continue; stepi 1`):

| Shape | Load-exclusive | Syndrome of its retire |
|---|---|---|
| (b) | `ldaxr x1,[x0]` @`0x1000003e0` | `0xcb000062` |
| (c) | `ldaxr x1,[x0]` @`0x100000434` | `0xcb000062` |
| (d) | `ldxp x1,x2,[x0]` @`0x10000047c` | `0xcb000062` |

Every other stepped instruction, in every run, reported `0xcb000022`.

- EC `0x32` (software step, lower EL), IL = 1, IFSC `0x22`.
- **ISS bit 24 (ISV) is 1 on every retire, and ISS bit 6 (EX) is 1 exactly when the stepped
  instruction was a load-exclusive.** This is the architecture's own "a Load-Exclusive was
  stepped" syndrome. Apple's cores and HVF deliver it on the EL2 step exit.
- So a retired load-exclusive can be recognised from the step exit itself, with no instruction
  decode on the ordinary step path.
- Not measured: the byte and halfword forms, `ldaxp`, and 32-bit pairs. They are the same
  instruction class, so the same bit is expected; the spec's decoder cross-check fails loud if
  that expectation is wrong.

## What t0 changes in the picture

1. **The exposure is not only "a step retires an LDX".**
   - A **native** stop mid-pair breaks the pair in the session that stopped, and phase 1 then
     livelocks on a retry loop (M4e, M5c, M6).
   - Resolving a native hit **by stepping** reaches the pair even when nothing the user armed is
     inside it (M3b, M3c).
   - So does measuring a window's length (M3d, and the `(1, 9)` cross-check).
   - Any window that contains a retry loop is unreachable by stepping, and anything that needs its
     length or an exact K in it hangs.
2. **Silent wrong answers exist beside the loud ones:**
   - `no earlier hit` (M3e);
   - phantom hits at coordinates past the window's end (M5a/b, M6, M4d);
   - a `reverse-stepi` that parks on a `getpid` `svc` the recording never ran (M7).

   Every one of these exited 0, or showed its first symptom only on a later command.
3. **A watchpoint fires on a store-exclusive that will fail** (M5). A design-B emulator that
   synthesizes the watch stop before emulating the store gives the same answer the hardware gives
   today.
4. **The retry shapes hide the lost store in their values.** Only an iteration count, or K, shows
   it (M7). A repo fixture has to publish its entry count, as this one does in `x4`, or its gate
   cannot see the difference the milestone makes.

## Not measured at t0

- **The dynamic path.** dyld and the cache `getpid` pairs are M41's measurement and were not rerun
  here. Nothing on `threadrust` or `cpython_crash` was rerun, including M41's Q3 bisection.
- **A checkpoint captured mid-pair.** The cost gate stores only seeks of 64 or more steps, and every
  fixture window is 33 steps or shorter, so no run here stored or restored one. The poisoned
  checkpoint is still inferred.
- **Other widths and orderings:** byte and halfword exclusives, 32-bit pairs (`ldxp w`),
  `ldaxp`/`stlxp`, an acquire form of (a), `Rn = sp`, `Rs`/`Rt` aliasing, a tagged (TBI) base.
- **`clrex`, `wfe`, and an LDX with no STX.**
- **Whether a watchpoint fires on a store-exclusive that fails without any debugger exit.** Natively
  the monitor passes, so that case can't be produced. Only the stepped case (M5) was observed.
- **How long the hangs would last beyond 60 s.** They are bounded there. The claim that they never
  end rests on M3a: 1,000,033 stepped instructions, and the counter never moved.
- **The asynchronous host-interrupt residual** (research note §5b).
