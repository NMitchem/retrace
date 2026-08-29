# M21-stackgrow Task 0 measurements

Numbers taken before any production change, on this machine, at `a57d340` (M21-stackgrow's plan
commit, on top of `ccfc8f9` = main @ the M20 close). Tagged **T0-1**…**T0-4** and cited by tag from
the design spec and later tasks rather than restated there.

No code under `crates/` is left changed by this task. T0-1's method briefly touched two files under
`crates/` to get evidence a text grep of the trace log cannot provide (see T0-1); both edits were
reverted before commit, verified with `git diff --stat crates/` showing no output.

## T0-1. Does anything currently land in the believed-stack window `[0x2008000, 0x27C0000)`?

**The brief's suggested command doesn't work, and that is itself worth recording.**
`RETRACE_TRACE=1` logs every trap as `[trap] num=... args=[...]` — trap numbers are numeric
(`mmap` is syscall 197, `mach_vm_map` is trap −15), never the literal strings `mmap`/`vm_map`/
`vm_alloc`, and the args logged are the syscall's *inputs* (a hint address, a size, flags), never the
address the placement algorithm actually chose. Running the brief's literal grep confirms this:

```sh
RETRACE_TRACE=1 cargo run -p retrace -- record-dyn "$HELLO_DYN_PATH" -o /tmp/t0.bin 2>&1 \
  | tee /tmp/t0-1.log | grep -aiE 'mmap|vm_map|vm_alloc' | head -40
```

Zero lines matched (390 lines of trace output, all `[trap]`/`[mach_msg2]`/`[retrace warn]` lines with
no `mmap`/`vm_map`/`vm_alloc` substring anywhere). So this measurement uses Ruling R1's alternative
(b): direct inspection of the box's `backings`/`reservations` after a real dynamic load.

**Guest binary, resolved per Ruling R1(a) first** (`cargo build -p retrace-guest` then newest by
`ls -t`, since three stale copies exist under `target/`):

```
$ find target -name hello_dyn -type f
target/debug/build/retrace-guest-5f848333cca97773/out/hello_dyn                    (Jul 30 18:49:49)
target/aarch64-apple-darwin/debug/build/retrace-guest-1c1babd99bbb9451/out/hello_dyn (Aug 27 06:37:02)
target/aarch64-apple-darwin/debug/build/retrace-guest-803c6b68f3336105/out/hello_dyn (Aug 27 08:36:02)  <- newest, chosen
```

**Method:** temporarily added a `#[doc(hidden)] pub fn dbg_backings(&self) -> Vec<(u64, usize)>` to
`Box_` (mirroring the existing test-only `dbg_internal_state()`, which already exposes
`reservations` but not `backings`) in `crates/retrace-box/src/lib.rs`, and one `eprintln!` of both
just before `record_box`'s final `Ok(RecordSummary { .. })` in `crates/retrace-core/src/lib.rs`,
gated on `trace_log` (the existing `RETRACE_TRACE` flag) so it costs nothing when unset. Ran:

```sh
RETRACE_TRACE=1 cargo run -p retrace -- record-dyn \
  target/aarch64-apple-darwin/debug/build/retrace-guest-803c6b68f3336105/out/hello_dyn \
  -o /tmp/t0-instrumented.bin
```

Raw output (the two dump lines, reservations decoded to hex, backings summarized — full raw values
are in the task's report file):

```
[dbg-final] reservations=[(42949771264, 1441792), (42952261632, 1998848), (17179869184, 25769803776)] ...
  -> 0xa00018000..0xa00178000 (nano band, len 0x160000)
  -> 0xa00278000..0xa00460000 (a later ANYWHERE reservation, len 0x1e8000)
  -> 0x400000000..0xa00000000 (the shared-region carveout hole, len 0x600000000)

[dbg-backings] 613 total entries, min=0x4000 max=0xfffffc000
  -> 0 backings overlap [0x2008000, 0x27C0000)
  -> the only backing anywhere near the window is (0x27c0000, 0x40000) — the CURRENT stack backing
     itself, sitting exactly at the window's exclusive upper bound (DYN_STACK_BOTTOM), not inside it
```

A Python pass over both lists (window `[0x2008000, 0x27C0000)`) confirms: **zero backings and zero
reservations overlap the window**. The two low reservations that exist (nano band, one ANYWHERE
mmap growth) both sit at 0xa0... (just below/around `MMAP_BASE` = 40 GiB), nowhere near 32-40 MiB.

**Forecloses:** the concern that reserving the window today would shift some *existing* placement
(and thus every recording's addresses) is unfounded — nothing currently lands there, confirmed by
direct inspection rather than assumption. `MMAP_BASE` being 40 GiB away, plus hint-forward first-fit,
is *why*; this measurement is the *evidence*, not the reasoning.

Both temporary edits were reverted with `git checkout -- crates/retrace-box/src/lib.rs
crates/retrace-core/src/lib.rs` immediately after this measurement; `git diff --stat crates/`
showed no output before commit.

## T0-2. Do any tests pin a reservation count or index?

The brief's two grep commands, run verbatim, need one fix: this shell's `grep` is `ugrep -G`, whose
BRE handling of `\(\)\|\.foo\b` does not behave like GNU/BSD grep's — it matches far too broadly
(it matched every comment merely containing the word "reservations"). Re-run with `command grep`
(the real system grep) and `-E`, which is the substance of what the brief's second command was after:

```sh
$ command grep -rnE "\.reservations\(\)" crates/ --include='*.rs'
crates/retrace-box/tests/carveout.rs:23:    assert_eq!(b.reservations(), &[(base, 0x40000)]);
crates/retrace-box/tests/carveout.rs:25:    assert_eq!(b.reservations(), &[(base, 0x10000), (base + 0x20000, 0x20000)], ...);
crates/retrace-box/tests/carveout.rs:35:    assert_eq!(b.reservations(), &[(base + 0x10000, 0x30000)], "head trim");
crates/retrace-box/tests/carveout.rs:37:    assert_eq!(b.reservations(), &[(base + 0x10000, 0x20000)], "tail trim");
crates/retrace-box/tests/carveout.rs:39:    assert_eq!(b.reservations(), &[] as &[(u64, u64)], "a full-cover dealloc removes the entry");
crates/retrace-box/tests/carveout.rs:63:    assert_eq!(b.reservations(), &[(base, 0x10000), (base + 0x30000, 0x10000)]);
```

All six hits are in `crates/retrace-box/tests/carveout.rs`, and every one is an `assert_eq!` against
a literal array — a **stricter** pin than a bare `.len()`/`[0]` check: it fixes both the exact count
*and* the exact contents. One extra reservation on the same `Box_` would break every one of them.

The other grep the brief asked for (bare `reservations` mentions outside `retrace-box/src/lib.rs`)
turns up only comments (`hello_dyn_e2e.rs:65-66`, `checkpoint.rs:4,43`) plus `carveout.rs`'s own
doc comments — no other file references `.reservations()` at all, confirmed by cross-checking every
file that mentions `load_dynamic` or `HELLO_DYN` for a `reservations` hit:

```sh
$ command grep -rln "load_dynamic\|HELLO_DYN" crates/ --include='*.rs' | xargs command grep -l "reservations"
crates/retrace/tests/hello_dyn_e2e.rs   # comment only, no assertion (verified above)
crates/retrace-box/src/lib.rs           # the implementation itself
```

**The decisive fact: `carveout.rs`'s `boxed()` helper uses the STATIC path, not the dynamic one M21
will touch:**

```rust
// crates/retrace-box/tests/carveout.rs:10-13
fn boxed() -> Box_ {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());  // HELLO, not HELLO_DYN
    Box_::load(&loaded)                                        // Box_::load, not load_dynamic
}
```

`Box_::load` (`lib.rs:1041`) and `Box_::load_dynamic` (`lib.rs:1602`) are separate constructors with
separate `Box_ { .. }` literals; the static one starts `reservations: Vec::new()` and nothing in the
static path ever calls `guest_vm_reserve` on its own — every reservation in these six tests is one
the test itself pushed via explicit `guest_vm_reserve`/`guest_munmap` calls. M21's planned change
(`reserve_believed_stack`, called from `load_dynamic`) never touches `Box_::load`.

**Forecloses:** these six exact-equality pins are real (an extra reservation on the *same* box would
break all six) but are **not at risk** from M21's planned change, *provided* the new reservation is
added only inside `load_dynamic`'s constructor path, as planned — not in `Box_::load` or in any code
shared by both paths. This is a constraint Task 1 must hold, not a green light to skip re-running
`carveout.rs` after the change.

## T0-3. The cost claim (LOAD-BEARING)

Baseline **before any change**, three runs each, `hello_rust_e2e` and `hello_dyn_e2e`, exactly as
specified (`--test-threads=1` included, both suites warm-built first so timing reflects test
execution rather than compilation):

```sh
for i in 1 2 3; do
  /usr/bin/time -p cargo test -p retrace --test hello_rust_e2e -- --test-threads=1 2>&1 | tail -3
done
for i in 1 2 3; do
  /usr/bin/time -p cargo test -p retrace --test hello_dyn_e2e -- --test-threads=1 2>&1 | tail -3
done
```

Raw output:

```
=== hello_rust_e2e ===
run 1: test result: ok. 1 passed ... finished in 8.28s   | real 39.24  user 7.97  sys 0.10
run 2: test result: ok. 1 passed ... finished in 7.23s   | real 35.86  user 6.86  sys 0.10
run 3: test result: ok. 1 passed ... finished in 7.27s   | real 36.83  user 6.84  sys 0.11

=== hello_dyn_e2e ===
run 1: test result: ok. 1 passed ... finished in 4.80s   | real 32.27  user 4.27  sys 0.09
run 2: test result: ok. 1 passed ... finished in 4.74s   | real 33.97  user 4.30  sys 0.08
run 3: test result: ok. 1 passed ... finished in 23.17s  | real 51.95  user 5.06  sys 0.08
```

All 6 runs passed (`0 failed`), so the baseline **was** takeable — the stop condition on an untakeable
baseline does not fire.

**The spread, not just the mean** (this is the number Task 1 Step 8 must beat, so it is reported
honestly, outlier included):

| suite | metric | run1 | run2 | run3 | mean | spread |
|---|---|---:|---:|---:|---:|---:|
| hello_rust_e2e | harness-reported | 8.28s | 7.23s | 7.27s | 7.59s | 1.05s (~14% of min) |
| hello_rust_e2e | `user` (CPU) | 7.97s | 6.86s | 6.84s | 7.22s | 1.13s (~17% of min) |
| hello_dyn_e2e | harness-reported | 4.80s | 4.74s | **23.17s** | 10.90s | 18.43s (outlier-dominated) |
| hello_dyn_e2e | `user` (CPU) | 4.27s | 4.30s | 5.06s | 4.54s | 0.79s (~18% of min) |

hello_dyn_e2e's run 3 is a **4.9x wall-clock outlier** on harness-reported time (23.17s vs ~4.8s) —
but its `user` CPU time (5.06s) is in line with the other two runs (4.27s, 4.30s). CPU time not
growing while wall time balloons means the process spent that extra time **not scheduled on a core**
(contention from something else on the machine), not doing more work — noise, not a regression, and
unsurprising with zero code changed between the three runs of the same binary. `user` is the more
reliable of the two columns for this reason, and its spread (~15-18% of the minimum, both suites) is
the noise floor Task 1's after-numbers must clear before a claimed regression is real.

**Forecloses:** the baseline is real and takeable (no stop condition), but comparing Task 1's
after-numbers against **wall-clock (`real`) alone would be worthless** — a single unlucky run can
read 5x slower with zero code change. Task 1 Step 8 must report `user` time (or repeat enough runs
to average out contention) rather than trust a single wall-clock reading, and must compare against
the **~15-18%** noise floor measured here, not against zero.

## T0-4. Where does the guest actually land today?

```sh
cargo test -p retrace --test stackoverflow_rust_e2e -- --test-threads=1 --ignored --nocapture
```

Raw output (relevant lines):

```
thread 'a_rust_stack_overflow_strikes_its_own_guard_page' (...) panicked at crates/retrace/tests/stackoverflow_rust_e2e.rs:29:5:
libstd's handler must recognize the fault as a stack overflow by comparing si_addr against its own guard range; stderr:
[retrace warn] dyld __mac_syscall(Sandbox) synthesized as success/unsandboxed (not forwarded; host would fault)
[retrace warn] dyld __mac_syscall(Sandbox) synthesized as success/unsandboxed (not forwarded; host would fault)
[retrace] forwarding mach_msg2 host_info (msgh_id 200) to host (decided allowlist)
[retrace] forwarding mach_msg2 host_get_clock_service (msgh_id 206) to host (decided allowlist)
[retrace] forwarding mach_msg2 semaphore_create (msgh_id 3418) to host (decided allowlist)
[retrace] forwarding mach_msg2 task_info (msgh_id 3405) to host (decided allowlist)
RECORD ERROR: non-syscall exit: data abort (EC=0x24 ISS=0x1c08047 FSC=0x7) far/ipa=0x27bff60 (UNMAPPED) pc=0x100000a70 elr=0x1804b1834

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 18.29s
```

This is an **exact** match to the failure already documented in the test's own `#[ignore]` reason
and to R3-c: `data abort (EC=0x24 ISS=0x1c08047 FSC=0x7) far/ipa=0x27bff60 (UNMAPPED)`.

- **FSC = `0x7`** — a **stage-2 translation fault** (no IPA backing exists at all), **not** `0x0f`
  (a stage-1 permission fault, which is what striking a real guard page correctly modelled would
  produce). This is the entire design argument for M21: the guest is not hitting its guard page at
  `0x2004000` at all — it runs off the end of the 256 KiB backing at `0x27C0000` and free-falls
  through 7.72 MiB of *unbacked* IPA before the hypervisor itself faults, ~160 bytes below
  `0x27C0000` (`0x27bff60` = `0x27C0000 − 0xa0`), nowhere near the guard page 8 MiB further down.
- The recording aborts with `RECORD ERROR: non-syscall exit`, not a guest-visible signal — libstd's
  overflow handler never runs, so the test's own assertion (that the handler recognizes the fault via
  `si_addr`) never gets the chance to fire; the panic message shown is the test harness reporting
  that the *recording itself* errored out first.

**Forecloses:** confirms, with a fresh measurement rather than trust in the parked `#[ignore]` text,
that today's failure is a stage-2 translation fault far below the guard page — exactly the shape a
lazily-committed reservation over `[0x2008000, 0x27C0000)` is designed to convert into a stage-1
permission fault at the real guard page instead. If this measurement had come back `0x0f` or an IPA
near `0x2004000`, the whole premise would be wrong; it did not.

## Summary — no stop condition fired

| tag | question | outcome |
|---|---|---|
| T0-1 | anything landing in the window today? | **No** — 0 of 613 backings, 0 of 3 reservations overlap `[0x2008000, 0x27C0000)` |
| T0-2 | tests pinning a reservation count/index? | 6 exact-equality pins in `carveout.rs`, all on the **static** path (`Box_::load`), unaffected by a **dynamic**-path-only reservation |
| T0-3 | cost of the change (baseline) | Taken, 6/6 passed; noise floor ~15-18% on `user` time; `real` alone is unreliable (one 4.9x wall-clock outlier, zero CPU-time growth) |
| T0-4 | today's actual failure | Confirmed **exactly**: EC=0x24 FSC=0x7 (stage-2 translation) at `0x27bff60`, ~160B below `DYN_STACK_BOTTOM`, nowhere near the guard page |

Task 1 may proceed.

## Task 1 Step 8: after-numbers (T0-3 re-measured with `reserve_believed_stack` in place)

Same commands as T0-3, verbatim, run after `reserve_believed_stack()` was wired into `load_dynamic`
(both suites warm-built first). `retrace-box`'s own full test suite passed 220/220 first (including
an explicit `carveout` re-run for constraint C1) before these were taken.

```sh
for i in 1 2 3; do
  /usr/bin/time -p cargo test -p retrace --test hello_rust_e2e -- --test-threads=1 2>&1 | tail -6
done
for i in 1 2 3; do
  /usr/bin/time -p cargo test -p retrace --test hello_dyn_e2e -- --test-threads=1 2>&1 | tail -6
done
```

**Contention caveat — these runs were NOT taken on a quiet machine, unlike T0-3's baseline.** While
they were in flight, `ps` showed two unrelated `cargo test` processes active on this machine from
other sessions: `cargo test -p ts-fixture-app` and `cargo test -p guide-examples`, neither belonging
to this repo. This is exactly the apples-to-oranges risk T0-3 warned about, so it is recorded here
rather than left implicit.

Raw output:

```
=== hello_rust_e2e (after) ===
run 1: test result: ok. 1 passed ... finished in 40.00s  | real 107.48  user 7.71  sys 0.13
run 2: test result: ok. 1 passed ... finished in 7.98s   | real 193.65  user 7.72  sys 0.11
run 3: test result: ok. 1 passed ... finished in 8.14s   | real 37.57   user 6.90  sys 0.29

=== hello_dyn_e2e (after) ===
run 1: test result: ok. 1 passed ... finished in 5.00s   | real 36.15   user 4.31  sys 0.19
run 2: test result: ok. 1 passed ... finished in 13.19s  | real 45.33   user 4.98  sys 0.14
run 3: test result: ok. 1 passed ... finished in 14.60s  | real 49.93   user 5.03  sys 0.12
```

All 6 runs passed (`0 failed`).

**`real` (wall-clock) is exactly the unreliable column T0-3 predicted, and worse here because of the
contention:** it ranges 37.57s–193.65s across these six runs — a 2-5x spread with zero code difference
between runs of the *same already-built binary*. Comparing wall-clock before/after would report a
large, entirely fake regression. This is not a hypothetical: it is the observed shape of this run,
and it is the reason Step 8 is specified against `user` CPU time rather than `real`.

**`user` CPU time, before vs. after (the load-bearing comparison):**

| suite | before (user) | before mean | after (user) | after mean | Δ |
|---|---:|---:|---:|---:|---:|
| hello_rust_e2e | 7.97 / 6.86 / 6.84 | 7.22s | 7.71 / 7.72 / 6.90 | 7.44s | **+3.0%** |
| hello_dyn_e2e | 4.27 / 4.30 / 5.06 | 4.54s | 4.31 / 4.98 / 5.03 | 4.77s | **+5.1%** |

Both deltas (+3.0%, +5.1%) sit comfortably inside T0-3's measured noise floor (~15-18% of the
minimum) and nowhere near the ~1.7x (+70%) regression that would invalidate the approach. Per the
brief's stop condition, that threshold was not approached, so no quiet-machine re-run was needed —
the contention caveat above stands as the recorded condition, not as an asterisk on the conclusion.

**Verdict: no cost regression.** A reservation that maps nothing until touched costs nothing until
touched, exactly as designed — `hello_rust_e2e`/`hello_dyn_e2e` never grow into the believed-stack
window, so `reserve_believed_stack`'s bookkeeping-only `guest_vm_reserve` call is the only per-load
cost it adds, and it does not show up above the noise floor.
