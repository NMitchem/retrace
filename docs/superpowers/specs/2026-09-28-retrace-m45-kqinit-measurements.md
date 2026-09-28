# M45-kqinit t0 measurements

**Companion to** `2026-09-28-retrace-m45-kqinit-design.md` (§3a: M1–M5). Measured 2026-09-28 on
this machine: macOS 26.5.2 (build 25F84, kernel `xnu-12377.121.10~1/RELEASE_ARM64_T6041`),
`Apple clang version 21.0.0 (clang-2100.1.1.101)`. Branch `worktree-m45-kqinit` at **`a78f28f`**
(the M45 plan commit).

**Binary.** M1 and M2's recordings ran a **throwaway build**: `a78f28f` plus a four-line
`[kevent_qos]` dump for syscall 374 inside `record_box`'s `if trace_log` block, directly after the
`[trap]` line. It prints the issuing thread and `72 × min(x2, 8)` bytes at `x1`, read through the
guest's own stage-1 walk (`Box_::read_va_prefix`). **No row and no arm for 374 was added.**
`target/aarch64-apple-darwin/debug/retrace`, ad-hoc signed by `tools/codesign-run.sh`: sha256
`897cd5f9b6c86b70fca2d73ef90a27fa52cbf3b206f9e90548dce1b4f23cce26`. The edit was restored with
`git checkout -- crates/retrace-core/src/lib.rs` after M2, and `git status --short` then printed
nothing. M2's candidates, M3's fixture and M4 ran **natively**.

**Evidence.** `docs/sweep-evidence/2026-09-28-m45-t0/` (its README says which command produced each
file). No trace (`.bin`) is committed. Scratch logs live in the ledger,
`.superpowers/sdd/2026-09-28-retrace-m45-kqinit/`, and are not committed.

**Outcome.** No halt. M1 equals §2a in every argument and every byte, and the unmeasured fields are
zero. All three GCD candidates reach the same call from thread 0. The fixture's native return is
`rc=0 carry=0` in both modes. `automationmodetool` exits 0 natively. The base count is 839.

---

## M1 — automationmodetool's whole call

**Command.**

```sh
export RETRACE_TRACE=1
cargo build -p retrace > $L/t0-m1-build.log 2>&1; echo "exit=$?"
tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /usr/bin/automationmodetool -o /private/tmp/claude-501/m45-amt.bin > $L/t0-m1.out 2> $L/t0-m1.err; echo "exit=$?"
grep -a -E '^\[trap\] num=374 |^\[kevent_qos\]' $L/t0-m1.err
```

Build `exit=0`. Record `exit=101`: this exit code was printed by the `echo` and is not kept in a
file, but the kept stderr ends in the unchanged M33 panic, which is what exits 101:
`panicked at crates/retrace-arch/src/lib.rs:983:38: M33: syscall 374 (374) has no arg_kinds row`.
Evidence: `m1-automationmodetool.err` (359 `[trap]` lines; 374 is the last).

**Result** (`m1-automationmodetool.err`, with the two traps before it):

```
[trap] num=368 (0x170) pc=0x1804af9f0 args=[0x400,0x27ff258,0x18,0x0,0x1,0x0,0x0,0x3]
[trap] num=367 (0x16f) pc=0x1804afa1c args=[0x0,0x27ff258,0x18,0x0,0x1,0x0,0x0,0x3]
[trap] num=374 (0x176) pc=0x1804afa48 args=[0xffffffff,0x27ff348,0x1,0x0,0x0,0x0,0x0,0x21]
[kevent_qos] thread=0 entry=[01, 00, 00, 00, 00, 00, 00, 00, f6, ff, 21, 00, 00, 00, 00, 02, f8, ff, ff, ff, ff, ff, ff, ff, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00]
```

The dump is exactly 72 bytes. Bytes 0–23 are
`01 00 00 00 00 00 00 00 f6 ff 21 00 00 00 00 02 f8 ff ff ff ff ff ff ff`, and bytes 24–71 are 48
zeros. Decoded against the layout in design §2b:

| bytes | field | value |
|---|---|---|
| 0–7 | `ident` | 1 |
| 8–9 | `filter` | `f6 ff` = −10 (`EVFILT_USER`) |
| 10–11 | `flags` | `0x21` = `EV_ADD \| EV_CLEAR` |
| 12–15 | `qos` | `0x02000000` |
| 16–23 | `udata` | `0xfffffffffffffff8` |
| 24–27, 28–31 | `fflags`, `xflags` | 0, 0 |
| 32–39 | `data` | 0 |
| 40–71 | `ext[0..4]` | 0, 0, 0, 0 |

**Issuing thread: 0** (main).

The arguments equal §2a's in all eight registers, **`x1` included** (`0x27ff348`, the same stack
address M44 t0 measured), and the `pc` is the same `0x1804afa48`. The only difference from M44's
run is two fewer `gettimeofday` (116) traps earlier in the run (`m1-vs-m44-traps.diff`: 361 against
359 `[trap]` lines). That is host timing, so a landmark number read from M44's evidence may not
match a new recording's.

**Decision.** §2a's inferred-zero fields are now measured zero, so the emulated shape is this
entry and these arguments, with only `x1` free. **Halt 1: not tripped.** No argument other than
`x1` differs, no byte differs, and the `pc` is the same.

---

## M2 — three GCD candidates

**Command.** The three candidates are written verbatim from the brief (`m2-timer.c`, `m2-after.c`,
`m2-signal.c`). Each should print `fired\ndone\n` natively.

```sh
export RETRACE_TRACE=1
for c in timer after signal; do clang -arch arm64 -o $L/t0-m2/$c $L/t0-m2/$c.c; echo "$c build=$?"; $L/t0-m2/$c > $L/t0-m2/$c.native.out; echo "$c native=$?"; done
for c in timer after signal; do tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn $L/t0-m2/$c -o /private/tmp/claude-501/m45-$c.bin > $L/t0-m2/$c.out 2> $L/t0-m2/$c.err; echo "$c record=$?"; grep -a -E '^\[trap\] num=374 |^\[kevent_qos\]' $L/t0-m2/$c.err; grep -a -E '^\[trap\]' $L/t0-m2/$c.err | tail -1; done
```

These ran on the same throwaway build as M1, before it was restored. `m2-compare.log` then compared
each candidate's 374 `[trap]` line (with `x1` masked) and its `[kevent_qos]` line with M1's, as
text.

**Native** (`m2-native.log`, `m2-*.native.out`, `m2-signal-repeat.log`):

| candidate | build | native rc | native stdout |
|---|---|---|---|
| `timer` | 0 | 0 | `fired\ndone\n` (11 bytes) |
| `after` | 0 | 0 | `fired\ndone\n` (11 bytes) |
| `signal` | 0 | **143**: it hung, and was killed with SIGTERM after more than 120 s | empty |

**`signal` does not do what the brief expects, natively.** It never prints `fired`. Five further
native runs, each bounded at 10 s by `perl -e 'alarm 10; exec @ARGV'`, all hit the alarm with
0 stdout bytes (`m2-signal-repeat.log`: five lines `signal repeat <i> native=142 bytes=       0`,
with `<i>` from 1 to 5). The hang is
deterministic on this host. Its cause was not measured. One possibility, which is not measured: the
source's `EVFILT_SIGNAL` registration is asynchronous, `raise` runs before it, and `SIG_IGN`
discards the signal.

**Under retrace** (`m2-record.log`, `m2-compare.log`):

| candidate | record exit | reaches 374 | 374 args (x1 masked) and 72 entry bytes vs M1 | thread | `x1` | the traps before 374 | `[trap]` lines |
|---|---|---|---|---|---|---|---|
| `timer` | 101 | yes | **equal** | **0** | `0x27ff0d8` | `-14`, `368`, `367` | 244 |
| `after` | 101 | yes | **equal** | **0** | `0x27ff0f8` | `-14`, `368`, `367` | 246 |
| `signal` | 101 | yes | **equal** | **0** | `0x27ff038` | `46`, `368`, `367` | 245 |

Each candidate's 374 is its last `[trap]`, followed by the same M33 panic as M1
(`crates/retrace-arch/src/lib.rs:983:38`). Retrace's stdout (`$L/t0-m2/<c>.out`) was empty for all
three, because each stops before its first `write`. Quoted, `timer`'s (`m2-record.log`):

```
[trap] num=374 (0x176) pc=0x1804afa48 args=[0xffffffff,0x27ff0d8,0x1,0x0,0x0,0x0,0x0,0x21]
[kevent_qos] thread=0 entry=[01, 00, 00, 00, 00, 00, 00, 00, f6, ff, 21, 00, 00, 00, 00, 02, f8, ff, ff, ff, ff, ff, ff, ff, 00, … 48 zero bytes …]
```

`m2-compare.log` prints `<c> entry == M1` and `<c> trap (x1 masked) == M1` for all three.

Under retrace, `signal`'s 374 comes after its `signal(SIGUSR1, SIG_IGN)` (the `46`, `sigaction`)
and before its `raise`, so the recording stops before the point where the native run hangs.

**Decision.**
- **Halt 2: not tripped for any candidate.** All three shapes equal M1's, so all three go to Task 3
  Step 3 (recorded under the landed emulation).
- For Task 3's `T0_M2_THREAD`, the issuing thread is **0** for every candidate. The call is issued
  by main, right after the workqueue pair (368, 367), not by a worker.
- **Finding for the controller.** `signal`'s native reference is a hang, not `fired\ndone\n`, so
  it cannot pass Task 3's gate body, which asserts `rec.stdout == b"fired\ndone\n"`. Task 3 Step
  3's record loop runs it unbounded, and if the recording reproduces the native hang, the loop would
  stall. Task 3's gate order puts `timer` and `after` first, and both are natively sound. Whether to
  bound or drop `signal` is the controller's ruling, not t0's.

---

## M3 — the fixture's native output

**Command.** `$L/t0-m3/kqinit_dyn.c` is the controller-ruled fixture (`kqinit_dyn.ruled.c`,
Ruling P1: `badptr` passes `1ull << 47`), copied unchanged (`cmp` identical; sha256
`5001f2c326742e086fa6cb959b7c3dd489465431a39fb96c8d3ce17c98435ed1`). Task 2 commits the same text.

```sh
clang -arch arm64 -o $L/t0-m3/kqinit_dyn $L/t0-m3/kqinit_dyn.c; echo "build=$?"
for m in "" straddle; do $L/t0-m3/kqinit_dyn $m > $L/t0-m3/native-${m:-plain}.out 2>&1; echo "mode '${m}' rc=$?"; cat $L/t0-m3/native-${m:-plain}.out; done
```

**Result** (`m3.log`, `m3-native-plain.out`, `m3-native-straddle.out`):

```
build=0
mode '' rc=0
kqinit rc=0 carry=0
mode 'straddle' rc=0
kqinit rc=0 carry=0
```

Each `.out` is exactly the 20 bytes `kqinit rc=0 carry=0\n`. The refusal modes (`flags`, `badptr`)
were not run natively, as the brief directs.

**Decision.** The host kernel returns 0 with carry clear for the measured call, issued from the
stack and from an entry that straddles a 16 KiB page, after a `dispatch_async` warm-up. The box's
constant is therefore the kernel's. **Halt 6: not tripped.**

**One inference, not measured by t0.** The fixture's warm-up (`dispatch_async` onto
`dispatch_get_global_queue(0, 0)` plus a semaphore) has the same shape as `dispatch_dyn.c`. That
guest's `dispatch_e2e` gate is not ignored, and it asserts a record exit of 0 (M44 closed green).
t0 did not re-run it. The gate could not pass if that shape issued a 374, because the M33 panic
would stop the recording. So under retrace, the fixture's hand-issued call should be
its only 374, which Task 2's `ev.len() == 1` assumes. Whether the native run's libdispatch issued
its own 374 before the fixture's call was not measured.

---

## M4 — automationmodetool natively

**Command.**

```sh
/usr/bin/automationmodetool </dev/null > $L/t0-m4-native.out 2> $L/t0-m4-native.err; echo "rc=$?"
wc -c < $L/t0-m4-native.out; head -3 $L/t0-m4-native.out; head -3 $L/t0-m4-native.err
```

**Result** (`m4.log`, `m4-native.out`, `m4-native.err`):

```
rc=0
      97
Automation Mode is disabled.
This device requires user authentication to enable Automation Mode.
```

- **rc: 0.**
- **stdout: 97 bytes**, two lines. The first line is `Automation Mode is disabled.`
- **stderr: empty** (0 bytes).

**Decision.** The native rc is 0, so outcome A keeps `records_and_replays_clean`, which asserts
rc 0 (Task 3 Step 2A's first branch), and no `launchctl`-style body is needed. The text reports host
state (Automation Mode disabled, authentication required), so it is this host's output, not a
constant of the binary. A gate should compare record and replay stdout with each other, not with
this text.

---

## M5 — the base `#[test]` count

**Command.**

```sh
grep -r -c -E '^\s*#\[test\]' crates --include='*.rs' | awk -F: '{s+=$2} END {print s}'
```

**Result** (`m5-test-count.txt`, at `a78f28f`): **`839`**. The file also lists the per-file counts it
sums, for the close's file-by-file reconciliation. Among them are `crates/retrace-arch/tests/census.rs:2`
and `crates/retrace-arch/tests/legacy_equivalence.rs:3`.

**Decision.** This equals the brief's expected 839. M44 closed at 832 passed + 9 ignored = 841, which
is these 839 plus the two `census.rs` tests that `legacy_equivalence.rs` compiles a second time.
No reconciliation is owed before Task 1. §9's prediction starts from 839.

---

## Halts considered

| halt | condition | result |
|---|---|---|
| 1 | any M1 argument other than `x1`, the `pc`, or any of the 72 entry bytes differs from §2a; or an unmeasured field is non-zero | not tripped: all equal, and `x1` is also equal to M44's |
| 2 | a GCD candidate's 374 differs from M1's | not tripped for `timer`, `after` or `signal` |
| 6 | the fixture's native default or `straddle` run does not print exactly `kqinit rc=0 carry=0` | not tripped: both print it, rc 0 |

Halts 3–5 belong to later tasks.
