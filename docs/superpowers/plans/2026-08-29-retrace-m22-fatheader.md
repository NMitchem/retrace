# M22-fatheader plan

Design: [`../specs/2026-08-29-retrace-m22-fatheader-design.md`](../specs/2026-08-29-retrace-m22-fatheader-design.md).
Measurements: [`../specs/2026-08-29-retrace-m22-fatheader-measurements.md`](../specs/2026-08-29-retrace-m22-fatheader-measurements.md).

Branch: `worktree-m22-fatheader`, cut from `main` at `ccfc8f9`.

> **Note on execution.** This milestone was measured, designed and implemented in one session, so
> the plan below records the task breakdown as it was actually carried out rather than as a forecast.
> Each task's RED step was genuinely observed; where a test could not be written before its
> implementation (the gate in t2 came after t1's fix was already in the tree), the fix was **mutated
> back out** to observe the failure, then restored. Both mutations are recorded below.

## t1 — `slice_native`, and the loader calls it

**RED.** Three tests in `crates/retrace-guest/src/lib.rs::fat_tests`:

1. `parse_macho_accepts_a_fat_binary` — parse `/usr/lib/dyld` (universal on this machine) directly
   and require it to match `parse_macho(slice_arm64e(..))` on entry, cpusubtype and segment count.
   Guards itself with an assert that dyld really is fat, so it cannot pass vacuously.
2. `fat_parse_prefers_arm64e_over_plain_arm64` — synthetic fat carrying both arm64 slices, **arm64
   listed first** so a first-match loop picks wrong. Pins S5.
3. `fat_parse_falls_back_to_plain_arm64` — synthetic `x86_64 + arm64`, the shape `slice_arm64e`
   panics on.

Observed failure, all three: `assertion left == right failed: not a 64-bit Mach-O (MH_MAGIC_64)`,
`left: 3199925962` (`0xBEBAFECA`). Failing for the missing feature, not a typo.

**GREEN.** `CPU_SUBTYPE_ARM64_ALL` in `retrace-arch`; `is_thin` / `is_fat` / `fat_find` extracted in
`retrace-guest`; `slice_arm64e` rewritten onto `fat_find` keeping its signature and panic;
`slice_native` added; `parse_macho` calls it as its first statement.

**Verify.** `cargo test -p retrace-guest --lib` → 9 passed / 0 failed.

**Mutation check.** Delete the `let b = slice_native(b);` line → the three new tests FAIL and the
other six still pass. Restored → 9 passed. The tests are load-bearing and precisely scoped.

## t2 — the headline gate, and the wall behind it

`crates/retrace/tests/sysbin_e2e.rs`:

- `an_apple_system_binary_records_and_replays` — `/bin/echo` with argv `["hi"]`, through
  `util::assert_rung_records_and_replays` (clean `exit(0)`, exact stdout, replayed twice), so a guest
  that died inside dyld cannot pass. Asserts `/bin/echo` is genuinely fat first.
- `an_objc_heavy_system_tool_records_and_replays` — **`#[ignore]`d** at the measured wall
  (`pc=0x4204`, 13 binaries), with the exact exception text and an explicit statement that the cause
  is unmeasured. Announces loudly rather than skipping silently if `/usr/bin/aa` is absent.

**Verify.** `cargo test -p retrace --test sysbin_e2e` → 1 passed / 1 ignored.

**Mutation check.** Delete the `slice_native` call → `an_apple_system_binary_records_and_replays`
FAILS on `not a 64-bit Mach-O (MH_MAGIC_64)`. Restored → green. The gate can fail.

## t3 — the two documents

Per the repo's two-documents rule: **append** a Status section to `docs/status-log.md`, and **edit**
the README's "What works today" / "Known limits" in place.

## t4 — the gate

Full `just gate`, chunked (the workspace run exceeds the 10-minute ceiling). Reconcile the total
against `main`'s 476 / 0 / 2 over 106 binaries by diffing `#[test]` counts file-by-file, not by
trusting a sum. Expected delta: **+4 tests** (3 in `retrace-guest`, 1 running + 1 ignored in the new
`sysbin_e2e` target) and **+1 test binary**.

**DONE.** **480 passed / 0 failed / 3 ignored across 107 test binaries**, all **58 chunks `EXIT=0`**,
clippy clean at `-D warnings` over `--workspace --all-targets`.

The predicted delta was exact. Per chunk: A 118 → **121** (the three `retrace-guest` fat-header
tests), B **219 → 219**, `--bins` **11 → 11**, retrace targets 128 → **129** (`sysbin_e2e`'s one
running test, plus its ignored one). B and `--bins` holding still is the reconciliation's real
content: a loader change disturbed nothing beneath it.

Two process notes worth keeping:

- **The runner waited for the machine.** A concurrent M21 session was mid-`cargo test -p
  retrace-box`. The gate script polled until that session's VM processes were quiet for a full
  minute before starting, with a 45-minute backstop so it could not hang. Half an hour of waiting
  bought a result both milestones can trust; running concurrently risked flaking *theirs*.
- **The tally reads only chunks recorded complete in `gate-exitcodes.txt`**, never `cat *.log` —
  M20's log records a tally that read 337 instead of 118 because a glob swept in another chunk's
  half-written log, and the giveaway was that the excess was exactly chunk B's total.
