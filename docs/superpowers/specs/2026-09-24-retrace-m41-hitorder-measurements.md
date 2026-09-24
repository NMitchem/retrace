# M41-hitorder — t0 measurements

**Date:** 2026-09-24. **Tree:** `main` at `c68ba6d` (the M40 merge), dev build, worktree
`m41-hitorder` before any change. Every transcript below is the `retrace debug --script` output of
that tree, copied verbatim; nothing here is inferred from reading code unless it says so.

**Recordings** (made fresh for t0, in the session scratchpad, not committed):

| Name | Guest | Command |
|---|---|---|
| `ws.bin` | `watchsweep` (asm) | `retrace record <OUT>/watchsweep -o ws.bin` |
| `fio.bin` | `fileio` (asm) | `retrace record <OUT>/fileio -o fio.bin` |
| `tr.bin` | `threadrust` (Rust, dyld) | `RETRACE_TRACE=1 retrace record-dyn <OUT>/threadrust -o tr.bin` |

**Addresses** (from `otool -tv` of the fixtures; `threadrust`'s from the `[trap]` log and `regs 1`):

- `watchsweep`: the sweeping store `0x1000003a0`, the next instruction `0x1000003a4`, the second
  writer `0x1000003b4`, `buf[40]` = `0x100004140`. Window 1 is the whole program up to the `write`:
  the sweep store for element *i* sits at K = 8 + 5*i*, so `buf[40]`'s sweep store is K = 208 and
  the second writer is K = 328.
- `fileio`: the `read`'s `svc` `0x1000003c4` (the last instruction of window 3, K = 5), the first
  instruction after it `0x1000003c8` (= (4, 0)), the next `0x1000003cc`, and `buf` = `0x100004000`.
- `threadrust`: main's one `__ulock_wait` (515, from `pthread_join`) returns to `0x1804afaf8` (the
  `[trap]` line's `pc`, i.e. ELR). The trace holds exactly one 515 and one 361. The child's first
  instruction is `0x1804ecc14` (thread 1's saved pc, `regs 1`, at the switch). These are shared-cache
  addresses: valid for this host's cache, so any test must discover them, never hardcode them.

## M1 — R11, silent: a pre-step that lands on a second breakpoint

```
> break 0x1000003a0
> break 0x1000003a4
> continue
hit 0x1000003a0 at (1, +?)  in sweep+0x4
resolved (1, 8)
> continue
hit 0x1000003a4 at (1, +?)  in sweep+0x8
resolved (1, 14)
> where
at (1, 14) pc=0x1000003a4 thread=0  in sweep+0x8
exit=0
```

Control, `break 0x1000003a4; continue; where` alone: `resolved (1, 9)`. The pre-step from (1, 8)
lands on (1, 9), the hardware fires there, and the resolver starts at `kctx + 1` = 10, so it names
the next pass of the loop. Exit 0: nothing outside a ground truth can tell.

## M2 — R11, loud, and what its message actually counts

```
> break 0x1000003c4
> break 0x1000003c8
> continue
hit 0x1000003c4 at (3, +?)  in _start+0x44
resolved (3, 5)
> continue
hit 0x1000003c8 at (4, +?)  in _start+0x48
DEBUG ERROR: resolve breakpoint hit #1 in window 4: window 4 ends after 0 instruction(s); cannot step 1
exit=5
```

Window 4 is `0x3c8`…`0x3e0`, **6** instructions, so "ends after 0" looked like a poisoned seek. It
is not. Controls: `break 0x1000003cc; continue` from a fresh session resolves `(4, 1)` (P1), a
boundary hit `break 0x1000003c8; continue` reports `(4, 0)` and steps on cleanly (P2), and
`break 0x1000003c4; break 0x1000003cc; continue; continue` crosses the same boundary and then
resolves `(4, 1)` (P4). So the crossing leaves nothing behind. The resolver started at K = 1, walked
to the `write`'s `svc` at K = 6 without seeing `0x3c8` again, and failed there. The `0` is
`step_insns(1)`'s count **within that one-instruction call**, because `resolve_nth` steps one
instruction per call. The window length it appears to report is not a window length.

## M3 — breakpoint on a watched store, forward: the watch half is skipped

`watch 0x100004140 8; break 0x1000003b4`. Ground truth: three hits, watch (1, 208) from the sweep,
then the breakpoint at (1, 328), then the second writer's watch at (1, 328).

```
> continue
hit watch 0x100004140 (write at 0x1000003a0) at (1, +?)  in sweep+0x4
resolved (1, 208)
> continue
hit 0x1000003b4 at (1, +?)  in sweep+0x18
resolved (1, 328)
> continue
exited (code 0)
```

The third `continue` pre-steps by re-seeking to (1, 329) with nothing armed, so the store retires
unobserved. Exit 0.

## M4 — the same shape, backward: the breakpoint half is skipped

Same arming, run to the exit, then `reverse-continue` ×4:

```
> reverse-continue
hit watch 0x100004140 (write at 0x1000003b4) at (1, 328)  in sweep+0x18
> reverse-continue
hit watch 0x100004140 (write at 0x1000003a0) at (1, 208)  in sweep+0x4
> reverse-continue
no earlier hit
```

From the watch hit at (1, 328), "strictly before P = (1, 328)" excludes the breakpoint at
(1, 328), which precedes the store. So M3 and M4 each lose a different half of the same pair.

## M5 — syscall write plus a breakpoint at (n, 0), forward: the breakpoint is skipped

`watch 0x100004000 8; break 0x1000003c8`. Ground truth: the `read` writes `buf` (a syscall hit at
(4, 0)), then `0x3c8` executes at (4, 0).

```
> continue
hit watch 0x100004000 (syscall write) at (4, 0)
> continue
exited (code 0)
```

The second `continue` is parked on `0x3c8`, so the pre-step rule steps off it: a breakpoint hit
that was never reported.

## M6 — the same shape, backward: the syscall hit is skipped

Same arming, run to the exit, then `reverse-continue` ×2:

```
> reverse-continue
hit 0x1000003c8 at (4, 0)  in _start+0x48
> reverse-continue
no earlier hit
```

From (4, 0), phase 1 counts a `WatchSyscall` at (4, 0) only when `(4, 0) < (pn, pk)`, which fails at
pk = 0. This was not in the design discussion's list: it was found here.

## M7 — arriving at (n, 0) by stepping: the syscall write behind you is not found

`watch 0x100004000 8; break 0x1000003cc; continue; continue; reverse-stepi; where; reverse-continue`:

```
> continue
hit watch 0x100004000 (syscall write) at (4, 0)
> continue
hit 0x1000003cc at (4, +?)  in _start+0x4c
resolved (4, 1)
> reverse-stepi
> where
at (4, 0) pc=0x1000003c8 thread=0  in _start+0x48
> reverse-continue
no earlier hit
```

The `read`'s write to `buf` happened before (4, 0), and the watch is armed. This is M40's deliberate
exclusion (its Ruling 2, a stuck-loop fix), measured here as the answer a user actually gets.

## M8 — T3 on `threadrust`: a phantom hit, and a real hit reachable in neither direction

Forward, `break 0x1804afaf8` (main's resume pc; exactly one real execution, when main resumes after
the child exits):

```
> continue
hit 0x1804afaf8 at (263, 0)
> where
at (263, 0) pc=0x1804afaf8 thread=0
> threads
* thread 0: Blocked(Wait { addr: 807432244 })
  thread 1: Runnable
> stepi
> where
at (263, 1) pc=0x1804ecc18 thread=1
> continue
hit 0x1804afaf8 at (269, +?)
DEBUG ERROR: resolve breakpoint hit #1 in window 269: window 269 ends after 0 instruction(s); cannot step 1
exit=5
```

- **The phantom (silent).** At (263, 0) main has just blocked. `pc()` and `where` still show main,
  and `continue`'s boundary check reports a hit that nothing executes: the next instruction to
  retire is thread 1's.
- **The real hit (loud).** At (269, 0) the child has just exited. The hardware fires on main's
  first instruction after the switch, and the resolver can't find it: `kctx + 1` skips K = 0, and at
  K = 0 `pc()` is still the exited child's.

Backward, from the exit with the same breakpoint: `reverse-continue` exits 5 with the identical
resolver error in window 269. Phase 1's hardware counts the hit correctly, but the resolver's
pc-before-step read at (269, 0) sees the outgoing thread.

The missed-hit shape, `break 0x1804ecc14` (the child's first instruction, at (263, 0)):

```
> continue
hit 0x1804ecc14 at (263, +?)
DEBUG ERROR: resolve breakpoint hit #1 in window 263: window 263 ends after 0 instruction(s); cannot step 1
exit=5
```

The README's Known limit names `reverse-continue` only. **Forward `continue` has the same blind
spot**: its boundary check and its pre-step both read `pc()` at (n, 0). `threadrust` already carries
both T3 shapes, so no new fixture is needed to reproduce them.

## Not measured at t0 (owed to plan Task 0)

- That `step()` with a breakpoint armed at the current pc stops pre-retire with the breakpoint
  class, and one disarmed step then retires (the premise of the oracle's `step_armed`).
- The cost of exhaustively single-stepping `threadrust` (the oracle's budget question).
- That approach A leaves `blockedctx`'s two measurements unchanged.
