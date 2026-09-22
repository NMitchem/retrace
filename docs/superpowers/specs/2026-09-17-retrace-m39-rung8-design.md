# M39-rung8 — a real script that crashes: CPython recorded, replayed, and reverse-debugged to the store

**Date:** 2026-09-17. **Branch:** `m39-rung8` from `main` at the M38-owed merge (`832a1cb`).
**Companion:** `2026-09-17-retrace-m39-rung8-measurements.md` (the probe, taken before this spec
was finalised — §2 cites it and nothing else in this document claims a measurement it does not
carry).

## 1. Purpose

The 2026-07-05 vision spec names one headline: *reverse-stepping through real CPython after a
crash on a real script*. Rung 7 (M25/M26) is the real interpreter running `-c 'print(1)'` —
startup, one builtin, exit 0. Between rung 7 and the headline sit four things no milestone has
measured together: a **script file** loaded from disk, **stdlib work** on real data (imports that
`dlopen` lib-dynload extensions, file I/O, parsing), a **hardware fault inside the interpreter**
on a pointer the script computed, and a **reverse-debug session** from that fault back to the
store of the pointer. This milestone closes that gap on a repo-owned script and calls it
**rung 8**. It does not touch the lldb-remote seam or async signals — those are the other two v1
gaps, chartered separately.

The shape follows M25→M26 rather than M38: a bounded walk from a measured first wall, where each
wall cleared gets a repo-owned guard asserting on the difference it makes, because the headline
gate itself skips without Homebrew Python and so guards nothing (`bigread_e2e`'s lesson).

## 2. What was measured before this spec was written

The companion measurements document is the record; this is its summary, and §3f designs only
what it measured.

- **On today's tree the script gets everything but `ctypes`.** The script file loads from disk;
  `import json`/`os`/`sys` complete (`_json….so` is a runtime `dlopen` and it loads);
  `crash.json` is opened, read and parsed; `target` is computed as `0x4000dead0000`; the
  console marker reaches fd 1. Rung 7 is 834 traps; the walk reaches 1069 before stopping.
- **The one wall before the marker is `import ctypes`**, and it is libffi's, not CPython's:
  `PyInit__ctypes` allocates a closure; Apple's libffi `vm_allocate`s a 3-page region,
  `dlopen`s **`/usr/lib/libffi-trampolines.dylib`** (a fat x86_64+arm64e system dylib — it
  loads), and `vm_remap`s that dylib's freshly `MAP_FIXED`-mapped RX `__TEXT` (2 pages) into the
  region, **shared** (`copy = FALSE`), `VM_FLAGS_OVERWRITE|FIXED`, `VM_INHERIT_SHARE`. That
  `mach_vm_remap` (msgh_id **4813**) has no route and aborts the record:
  `RECORD ERROR: unsupported mach_msg2 … msgh_id 4813 dest 0x203 … send_size 92`. This is on
  every `import ctypes` on this OS, not on the fixture's use of it.
- **Runtime `dlopen` is not a wall**, including a fat arm64e system dylib: three runtime
  `__TEXT` loads beyond rung 7's one completed (`_json`, `_ctypes`, `libffi-trampolines`).
- **No `arg_kinds` row was missing** on the path; none of M38's owed set was reached.
- **Unmeasured:** everything after `import ctypes` under retrace (the `cast`, the marker, the
  deref, `Event::Crash`, its DFSC and thread tag, replay, reverse-continue); what the real
  kernel returns for `cur`/`max_protection` on this remap; the second wall, if any.

The fixture's import order is kept as drafted (`ctypes` first): the wall is the same either
way, and the `ctypes`-last variant exists only in the measurements document, as the bisection.

## 3. Design

### 3a. The fixture: `crash.py` + `crash.json`

Two repo files under `crates/retrace-guest/py/`, exposed as path constants `CRASH_PY` and
`CRASH_JSON` in `retrace-guest`'s `lib.rs` (`concat!(env!("CARGO_MANIFEST_DIR"), …)`). Python
fixtures need no compile step, so `build.rs` is untouched; the script locates its data file
relative to `__file__`, and both exist at replay because they are repo files.

`crash.py` does modest, real stdlib work and then faults:

1. `import ctypes, json, os, sys` — `json` pulls `_json.cpython-314-darwin.so` and `ctypes`
   pulls `_ctypes.cpython-314-darwin.so`; each is a runtime `dlopen` of a non-cache dylib (the
   measured count is exactly these two plus libffi's trampolines — no `_struct` load). `_ctypes` links only `/usr/lib/libffi.dylib` (shared cache) and libSystem — no
   Homebrew libffi (measured with `otool -L`).
2. Opens `crash.json`, parses it, builds a table `name → int(base,16) + int(offset,16)`, and
   selects `target = table[cfg["target"]]`. The bad address is **computed from data**, never a
   literal in the script, so "who stored this pointer?" has a real answer.
3. `p = ctypes.cast(target, ctypes.POINTER(ctypes.c_long))` — `_ctypes`'s `cast()` stores the
   value into the new Pointer object's buffer. **That store is the milestone's target.**
4. Writes the marker `CRASHPY cell=<ctypes.addressof(p)> target=<target> rows=<n>` to stdout and
   flushes — the M6 marker convention: the test **discovers** the cell from the recording, never
   hardcodes it. `addressof(p)` is the buffer `cast()` wrote, the cell `p[0]` will load from.
5. `print(p[0])` — `Pointer_item` loads the pointer from the cell and dereferences it: a stage-1
   EL0 data abort with `FAR == target`. `target = 0x4000_DEAD_0000` (bit 46 set, L1 index
   0x400, never mapped, < 2^47) — the same FAR `crashy.c` and `asm/crash.s` use, chosen for the
   same reason. A trailing `UNREACHED` write is the negative marker.

Natively (2026-09-17, Homebrew `python@3.14`): the marker line, then exit 139.

### 3b. The walk

Wall 1 is measured and designed (§3f). Record `crash.py` on the branch with it cleared; at each
further wall, in order:

1. **Name it** from `RETRACE_TRACE=1` output: the landmark, the syscall/trap, the thread.
2. **Classify it.** A missing `arg_kinds` row → add the row **with the landmark that reached it**
   as its evidence (the operator's ruling of 2026-09-17: rows are added only as CPython reaches
   them; rows only the Apple sweep reaches stay owed). A mechanism gap (a `Box_` path, a new
   trap shape, a replay asymmetry) → a design note in the task report, then a fix under symmetry
   rules 1 and 2.
3. **Guard it.** Every mechanism fix gets a repo-owned fixture (C or asm, compiled by
   `build.rs`) whose test asserts on the difference the fix makes and is **red before the fix**
   (the red run kept in the task report). A row addition is guarded by the `arg_kinds` census
   and the sweep.
4. **Re-record.** Repeat.

**Wall ceiling: six, counting wall 1.** A seventh wall halts the milestone (§5) for a re-spec,
with the walk so far written up — the M2 chain went ten deep before anyone stopped to look, and
M25 parked at its second replay with a diagnosis that was wrong three ways.

### 3c. The gate: `cpython_crash_e2e`

One test binary, `crates/retrace/tests/cpython_crash_e2e.rs`, skipping loudly (the
`cpython_e2e` `eprintln!`) when the framework interpreter is absent. It records
`REAL crash.py` through `record-dyn`, and asserts on the difference rung 8 makes — never on
exit 139 alone, which a guest that died in dyld also produces:

1. **The script ran.** Recorded stdout contains one `CRASHPY cell=… target=0x4000dead0000
   rows=3` line and no `UNREACHED`; `cell` is parsed from it.
2. **The crash is the deref.** The trace's terminal event is `Event::Crash` with
   `far == 0x4000_dead_0000` and a translation-class DFSC (`0x04..=0x07` — a level-1 fault on
   an unmapped L1 index; the exact code is pinned by T1's first record that reaches it, and
   narrowed in the test then). Its `thread` tag equals the tag on the marker `write(1)`
   landmark — the deref runs on the thread that wrote the marker — a relative assertion that
   needs no prediction about how many threads CPython starts.
3. **Replay agrees, twice.** Both replays exit 139 with byte-identical stdout, and the oracle
   reports no divergence (the M6 convention: a verified crash replay is a successful replay).
4. **Reverse-continue reaches the store.** A scripted `debug --script` session:
   `continue; watch <cell> 8; reverse-continue; where; stepi; x <cell> 8`. Asserted: the
   `reverse-continue` stop's `(N,K)` is strictly before the crash coordinate; after `stepi`,
   the eight bytes at `cell` are `target` little-endian. That is "the instruction that wrote the
   bad pointer", checked by its effect and independent of symbols — `cast()` is a static
   function in `_ctypes.so`, so no symbol assertion is made (M19 symbolicates the main image
   and does not reach a runtime-loaded dylib's symbols; that is not this milestone's wall).
5. **Repo-owned guards** for each mechanism fix cleared on the walk, one binary each, named for
   the mechanism (the `bigread_e2e` / `bigwrite_e2e` pattern), so the class is guarded on a
   machine without Homebrew.

### 3d. Symmetry obligations

- A new handler arm goes in **both** dispatch loops (`record_box` and `ReplaySession::advance`)
  before the generic forward arm, calling the same `Box_` method with the same arguments; replay
  byte-compares the recomputed reply. If it `return`s before the generic dispatch, it is a new
  `verify_thread` site — the count in CLAUDE.md moves from **7** and the change says so.
- Deterministic instruction emulation goes below the trace, in `Box_::run()`.
- `TRACE_MAGIC` moves only if a wall needs a trace-shape change. The bump is **pre-authorised
  conditionally** (operator, 2026-09-17, by approving this shape): if a wall needs it, it is not a
  halt, the value goes `RT\x00\x0a → RT\x00\x0b`, and CLAUDE.md/README say so; if no wall needs
  it, it does not move.

### 3e. What "done" looks like to a user

The README's "What works today" gains rung 8 and a **demo transcript**: `record-dyn` of the
interpreter on `crash.py` (exit 139), `replay` (identical), and the `debug` session from the
crash to the store — the vision spec's headline, on a script anyone with Homebrew Python can run.

### 3f. Wall 1 — `mach_vm_remap` (4813), serviced as a stage-1 alias

**What it is.** A Mach VM RPC on the guest's own task port: "make `size` bytes at
`target_address` map the same memory as `src_address`" — for libffi, shared (`copy = FALSE`),
at a fixed target it chose (`VM_FLAGS_OVERWRITE|FIXED`), from a source that is the RX `__TEXT` of
a dylib dyld just placed. The reply carries `KERN_SUCCESS`, the target address, and the
mapping's current and maximum protections.

**The precedent is `mach_vm_map` (4811), `Route::ServiceVmMap`**, the box's other serviced VM
RPC: a route in `machmsg::route`, a pure decoder, one `Box_` method, a pure reply encoder;
record appends `Event::Syscall` carrying the synthesised reply as its `writes` and applies it;
replay recomputes the reply through the *same* `Box_` method and byte-compares it against the
recording before applying — the comparison is the divergence check (symmetry rule 1). 4813
follows it exactly:

- `machmsg.rs`: `Route::ServiceVmRemap` for msgh_id 4813 on the guest task port;
  `decode_vm_remap(&[u8]) -> Result<VmRemapReq, String>` (the 92-byte layout in the measurements
  document, including the `src_task` port descriptor); `encode_vm_remap_reply(reply_port,
  target, cur, max) -> Vec<u8>` (60 bytes with trailer, the `vm_map` reply's shape plus two
  protections). Golden-tested against the captured request bytes, as the 4811 codec is.
- `Box_::guest_vm_remap(target, size, src, exec) -> u64`: for each 16 KiB page, the target's L3
  entry is set to the **source's IPA with the source's attributes** (`ATTR_CODE` for these RX
  pages) — an alias, no copy, no new backing — and then **`flush_guest_tlb`**: the target range
  sits inside a region the guest `vm_allocate`d and may already have translated as RW/UXN, and
  M9's rule is that a stale RW entry under a now-executable page must be invalidated by the
  guest's own `tlbi` before it is executed. Returns the target address. W^X holds: the alias is
  RO+exec because its source is.
- Both dispatch arms call it with the decoded `(target, size, src, cur_protection & EXEC)` and
  encode the reply from its return plus the protections; replay compares bytes.

**What the model asserts, fail-loud** (each is scope this spec lacks; measured libffi needs
none of them): `copy == TRUE` (a copy would need new backing and a memcpy — a different
operation); `VM_FLAGS_ANYWHERE` (the box would have to choose the address — the 4811 first-fit
could serve, but nothing measured asks); a `src_task` descriptor naming any port but the guest's
own task; a source range not fully mapped in stage 1; `size` not a page multiple.

**The protections in the reply are measured, not chosen.** A synthesised reply must be *correct*,
not merely deterministic — libffi, or any later caller, may test `cur_protection & EXECUTE`.
The task that lands this route first runs a freestanding native C probe (`vm_allocate` 3 pages,
`mmap` a dylib's text `MAP_FIXED|PROT_EXEC`, `vm_remap` it shared with `OVERWRITE|FIXED`, print
`cur`/`max`) and pins the model to the kernel's answer, with the probe's output in the task
report. Prediction, marked as such: `cur = READ|EXECUTE (5)`, `max` unknown.

**Why this is not a snapshot or checkpoint hazard.** The stage-1 tables live in guest memory at
the fixed IPA layout, so the final full-memory snapshot and M4's `BoxState` checkpoints capture
the alias with everything else; an alias adds no backing and the IPA-indexed diff never counts a
page twice. Watchpoints are by VA and unaffected.

**What it does not model.** Un-remapping (`vm_deallocate` of the alias is the ordinary
deallocate path if it is ever reached — unmeasured, natively unreached before the crash);
remaps between tasks; `copy = TRUE`. Each asserts by name.

**Guard.** A repo-owned freestanding C fixture (`vmremap_dyn.c`: `vm_allocate` a region,
`vm_remap` its own `__TEXT` page into it shared, **call through the alias**, print the result) and
`vmremap_e2e` asserting the alias executed — red before the route exists (the record aborts on
4813), green after — so the mechanism is guarded on a machine without Homebrew Python.

## 4. Task order and why

- **T0 — the probe** (done before this spec; §2 and the measurements doc). Its output is the
  first wall and a census; nothing from it is implemented.
- **T1 — fixture + gate, red.** Land `crash.py`/`crash.json` and the constants, write
  `cpython_crash_e2e` with all four assertions, and keep its red run: which assertion fails and
  at which landmark (predicted: assertion 1, the record aborting on 4813 at trap ~979). The red
  run is the milestone's baseline.
- **T2 — wall 1** (§3f): the native protections probe, the codec with golden tests, the `Box_`
  alias, both arms, `vmremap_dyn` + `vmremap_e2e` red-then-green, then `crash.py` re-recorded
  and the new stopping point named.
- **T3…Tn — the walk** (§3b), one task per further wall, each with its guard and its pre-fix
  red run, until `cpython_crash_e2e` is green or the ceiling halts it.
- **T(n+1) — the close.** README ("What works today" rung 8 + transcript; "Known limits" for
  anything re-parked), CLAUDE.md's e2e list, status-log section, one sweep (the regression check
  on any `arg_kinds` change), the chunked gate reconciled file-by-file against 618/0/9 over 135,
  then one push.

The format bump, if any, lands in the task that needs it, first thing in that task, so every
evidence trace after it is the new magic.

## 5. Envelope

M38's envelope, with the wall ceiling added:

- **Push once at the close.** Nothing else is pushed.
- **`TRACE_MAGIC` bump conditionally pre-authorised** (§3d).
- **Halt** — stop with the branch intact and a written explanation, never a best guess, never a
  silent narrowing — on: a red gate surviving one fix round; any **new** `#[ignore]` (a gate not
  ignored at `832a1cb`; re-parking one of the nine with a rewritten reason is not new); a class-E
  sweep row; any task needing scope this spec lacks; **the seventh wall**.

A measurement that contradicts this spec's premise is a §8 ruling and a re-scope, written down.

## 6. Acceptance

- `cpython_crash_e2e` green on this machine with all four assertions (§3c), un-`#[ignore]`d; or,
  if the walk halts at the ceiling, parked `#[ignore]` at a named wall with its evidence — the
  honest-gate rule, and a regression of nothing.
- One repo-owned guard per mechanism fix, each verified red-before-green in its task report.
- Every `arg_kinds` row added carries the landmark that reached it; none added speculatively.
- `cpython_e2e` (rung 7) still green; the launcher test unchanged.
- Sweep: PASS count ≥ 44 (M38's), every moved row explained by name, no row moved by anything
  this spec does not name.
- Gate: chunked, `--no-fail-fast`, exit codes captured before any pipe, `--bins` chunk run,
  `--doc` beside any per-target library split; total reconciled file-by-file against 618/0/9
  over 135.
- README "What works today" (rung 8 + transcript) / "Known limits", CLAUDE.md (`verify_thread`
  count, `TRACE_MAGIC`, the e2e list), and the status-log section all describe the new reality.

## 7. What this milestone deliberately does not do

- **No exec-in-place.** The interpreter is run directly from its framework path, as rung 7 does;
  the launcher shim stays a pinned gap.
- **No threads, no subprocess in the script.** Threads hit the cooperative scheduler under the
  GIL — a real question, but a different milestone's; `subprocess` is `fork`/`posix_spawn`, a v1
  non-goal and refused since M38.
- **No lldb-remote stub, no async-signal injection** — the other two v1 gaps.
- **No symbolication of runtime-loaded dylibs.** The store is asserted by effect (§3c.4). If the
  walk shows M19's seam reaches `_ctypes.so` for free, the transcript may show it; nothing is
  asserted on it.
- **No sweep-only `arg_kinds` rows.** {461, 468, 464, 345, 374} land here only if `crash.py`
  reaches them; the rest stay owed by name.
- **No `faulthandler`.** CPython's default disposition for SIGSEGV is the kernel's; the script
  does not enable `faulthandler`, so the fault is `Event::Crash`, not a `SignalDelivery` into a
  handler (the distinction CLAUDE.md's last paragraph draws). A successor may want the
  handler-installed variant as its own rung.

## 8. Rulings (made while writing this spec)

- **R1 — the fixture is Python source in the repo, not a `build.rs` product.** Nothing to compile;
  path constants are enough; the data file is reachable at replay because it is a repo file.
- **R2 — the store is asserted by effect, not by symbol** (§3c.4): a symbol assertion would tie
  the gate to M19's reach into runtime dylibs, which is not what rung 8 makes different.
- **R3 — the bad address is `crashy.c`'s** (`0x4000_DEAD_0000`): a known-unmapped L1 index whose
  fault class three fixtures already pin. Choosing a new one would add a measurement for no
  assertion.
- **R4 — the wall ceiling is six, wall 1 included**: counted as walls — the measured one in §3f
  plus each one named under §3b.1 — not as tasks.
- **R5 — a guest that reaches the crash but replays with a divergence is a wall**, not a halt:
  M26's class. It counts against the ceiling like any other.
- **R6 — the remap is an alias, never a copy** (§3f): measured libffi asks for `copy = FALSE`;
  a copy is a different operation and asserts by name. The alias is followed by
  `flush_guest_tlb` unconditionally — cheaper than proving the guest never translated the
  target, and M9's rule is what makes the exec-over-formerly-RW page sound.
- **R7 — the remap reply's protections come from a native measurement**, not from this spec:
  the model reproduces the kernel's answer, and the task report carries the probe that got it.
- **R8 — a new mach_msg2 route adds no `verify_thread` site.** The route match sits inside the
  generic `Syscall` arm after its oracle call (measured at `crates/retrace-core/src/lib.rs`
  ~1756/1878), so the count stays **7**; the close checks it rather than assuming it.

## 9. Gate prediction

618/0/9 over 135 at the branch point. Predicted: +1 binary for `cpython_crash_e2e` (its tests
counted as passed on this machine, skipped-loud elsewhere), +1 binary for `vmremap_e2e`, +1
binary per further mechanism guard, +k `#[test]`s in `retrace-guest`'s parse/constant tests,
`retrace-core`'s `machmsg` codec tests (decode/encode/route for 4813, the 4811 set's shape) and
`retrace-arch`'s `arg_kinds` census per row added. Ignored stays 9 unless the walk halts (then
+1, named). The exact figure is the close's to measure and reconcile; this is the shape.

## 10. Outcome

*Filled at the close, 2026-09-21. Written by Task 7a; the gate figures and the demo transcript are
Task 7b's and are marked.*

**Rung 8 reached, with one wall.** `cpython_crash_e2e` is green and was never `#[ignore]`d: the
real interpreter runs `crash.py`, dies on the ctypes deref, records, replays byte-identically
twice, and a scripted `reverse-continue` from the crash lands on the store of the pointer. All
four §3c assertions hold, measured on `123cb97` (Task 5's Step 3 run, 12,364.35 s, log
`task-5-crash-run.log`).

### The measured figures

| figure | value |
|---|---|
| traps in the rung-8 record, start to crash (`RETRACE_TRACE=1`, `^\[trap\]` lines) | 1,146 |
| t0's `ctypes`-first probe, start to the 4813 abort (measurements doc) | 979 |
| t0's `ctypes`-last bisection variant, start to the 4813 abort (§2's "1069") | 1,069 |
| rung 7's whole `print(1)` run (§2) | 834 |
| trace file | 97,449,874 bytes |
| fault | `pc=0xa017e2e60 esr=0x92000005 far=0x4000dead0000 ec=0x24` |
| record / replay / `debug --script "continue"` | 8.585 s / 8.822 s / 32.643 s |
| the whole `cpython_crash_e2e` gate | 12,364.35 s |
| gate totals | ⟨GATE⟩ *(Task 7b)* |

The three counts have three different endpoints — 1,146 runs to the crash, the two probe figures
stop where the recorder aborted at 4813, and 834 is a different script — so they are a scale, not a
subtraction. The committed fixture is the `ctypes`-first form, so 979 is its own predecessor.

### Walls: one, against a ceiling of six (§3b, R4)

**Wall 1 — `mach_vm_remap` (msgh_id 4813).** Exactly as §3f designed it, and it is the only one.
`Route::ServiceVmRemap`, `decode_vm_remap` (92 bytes, golden-tested against the captured request),
`encode_vm_remap_reply` (60 bytes, id 4913), `Box_::guest_vm_remap`, and mirror arms in both
dispatch loops before the generic forward. Guards, each red before green in its task report:
`crates/retrace-box/tests/vmremap.rs` (three unit tests on the alias itself),
`crates/retrace/tests/vmremap_e2e.rs` (the repo-owned end-to-end guard), and four `machmsg` codec
tests. **No second wall.** Task 6 — the walk's re-record-and-instantiate loop — is **empty**: Task
5's first run past the route reached the crash, the replay, and the reverse-continue with nothing
in between. The walk procedure §3b describes was never entered.

### Every prediction, confirmed or corrected

- **DFSC (§3c.2, Ruling 1) — CONFIRMED.** The spec wrote the class `0x04..=0x07`; Ruling 1 narrowed
  it to `== 0x05` before the first run, on `crashy.c`'s measured `0x92000005` at the same VA. The
  first record that reached the crash measured `esr=0x92000005`, so `esr & 0x3f == 0x05` — the
  strict form is what the gate asserts and it held. No loosening was needed.
- **The thread tag (§3c.2) — CONFIRMED.** The terminal `Event::Crash`'s tag equals the tag on the
  last `write` to fd 1 before it (the marker). The relative form needed no prediction about how
  many threads CPython starts, and the assertion passes.
- **The second wall (§2 "Unmeasured", §3b, R4) — CORRECTED to none.** The spec budgeted six walls
  and described a walk procedure for finding them. There was one. Everything §2 listed as unmeasured
  past `import ctypes` — the `cast`, the marker, the deref, `Event::Crash`, its DFSC and thread tag,
  replay, reverse-continue — worked on the first run past the route.
- **The remap's protections (§3f, R7) — CORRECTED in shape, confirmed in method.** §3f predicted
  `cur = READ|EXECUTE (5)` with `max` unknown, and told the task to pin the model to a native probe.
  The probe (Task 2 Step 5, `vmremap_dyn` run natively three times, stable) measured **two different
  answers**: `SELF kr=0 cur=5 max=5 call=42` for the program aliasing its own `r-x` text, and
  `FFI kr=0 cur=5 max=7 same=1` for the `dlopen`'d `libffi-trampolines.dylib`, whose `__TEXT` is
  mapped with an elevated max so it can hand out writable JIT sub-mappings under W^X. `cur = 5` held
  for both; a single constant pair could not serve both, which the spec's singular
  "`VM_REMAP_CUR_PROT`/`VM_REMAP_MAX_PROT`" phrasing had assumed. **Ruling 4** replaced the constants
  with derivation, before Tasks 3–4 were dispatched: `cur` from the source page's live stage-1
  attribute (`ATTR_CODE` → 5, `ATTR_DATA` → 3, `ATTR_NONE` → 0, anything else panics by name), `max`
  from the source's band (`src < NANO_BAND_START` → 5 for kernel-placed images, else 7 for
  guest-allocated memory). That is a pure function of the address and the live tables, so it is
  identical on record, on replay-from-restore and after a checkpoint seek, adds no `Box_` state and
  incurs no M24/M31 parity debt — and it reproduces both measured lines. R7's *method* (measure, do
  not choose) is what produced the correction, so the ruling stands confirmed even though its
  predicted value did not. Two shapes the band rule would answer wrongly are documented at the
  method and unmodelled: a guest `FIXED` mmap below the nano band (kernel 7, model 5) and a
  read-only `MAP_SHARED` source above it (kernel 5, model 7). Neither is measured; no known caller
  issues either.
- **`verify_thread` stays 7 (§3d, R8) — CONFIRMED.** The 4813 route sits inside the existing
  generic `mach_msg2` arm, after its oracle call, so it adds no returning arm and no new site.
  `grep -c 'self.verify_thread(' crates/retrace-core/src/lib.rs` prints **7** at the close, the
  same as at the branch point.
- **`TRACE_MAGIC` (§3d) — did not move.** The bump was conditionally pre-authorised; no wall needed
  a trace-shape change, so the magic stays `RT\x00\x0a`.
- **The gate prediction (§9) — short by one binary.** §9 named +1 binary for `cpython_crash_e2e`,
  +1 for `vmremap_e2e`, +1 per further mechanism guard, and `+k` codec and parse tests. The
  mechanism guard §3f itself asked for at the box level, `crates/retrace-box/tests/vmremap.rs`, is a
  third new test target that §9 did not count. Measured: **+11 `#[test]` attributes over five files
  and +3 test binaries**, 135 → 138. Per-file deltas are in the status-log section; the measured
  totals are ⟨GATE⟩ *(Task 7b)*.
- **`arg_kinds` rows (§6) — none added.** §2 measured that no row was missing on the path and §7
  forbade sweep-only rows; nothing on the rung-8 path reached an unclassified syscall, so the owed
  set {461, 468, 464, 345, 374} is untouched and still owed.

### Acceptance (§6)

| item | outcome |
|---|---|
| `cpython_crash_e2e` green, all four assertions, un-`#[ignore]`d | **met** (never ignored — the wall fell inside the milestone) |
| one repo-owned guard per mechanism fix, red-before-green in its task report | **met** (`vmremap.rs` unit tests; `vmremap_e2e`) |
| every `arg_kinds` row carries the landmark that reached it | **vacuous** — none added |
| `cpython_e2e` (rung 7) still green, launcher test unchanged | **met** (Task 5 Step 2: 2 passed) |
| sweep PASS ≥ 44, every moved row explained, no row moved by anything unnamed | **met** — `pass=44 fail=10 skip=0`, no row changed its label; nine rows differ only in a panic-string thread id, a source line number from M38's own post-sweep fix commit, and the `csh`/`tcsh` landmark spread M37 measured. `docs/sweep-evidence/2026-09-17-m39/README.md` |
| gate chunked, `--no-fail-fast`, exit codes before any pipe, `--bins` run, reconciled file-by-file | ⟨GATE⟩ *(Task 7b)* |
| README, CLAUDE.md, status-log section describe the new reality | **met** (this close) |

### What the milestone found that it did not set out to find

**`reverse-continue` on a rung-8 recording costs about 3.42 hours.** The gate's 12,364.35 s minus
the 50.05 s that record, replay and `continue`-to-the-crash account for leaves 12,314.30 s for
`watch; reverse-continue; x; stepi; x`, and `watch`, `x` and `stepi` are each O(1) or one
instruction. Ruling 9 made this a finding to **report, not to fix**: the spec's halt conditions are
red gates, new `#[ignore]`s, class-E rows, missing scope and the seventh wall, and a slow green is
none of them, while making `reverse-continue` fast is scope this spec lacks. The mechanism is a
**code read, not a profile**, and is stated as inference in the README and the status log:
`cmd_reverse_continue` rescans forward from landmark 1 and restarts a fresh `checkpointed_seek` per
intervening watch hit, each restart resolving its sub-landmark offset with one single-instruction
trap per guest instruction. It is on the owed list.

**A stage-1 alias is invisible to readers that go by address.** `Box_::read_guest` resolves against
the backings list rather than by walking the tables — every mapping before M39 was identity, so the
two agreed by construction. The debugger's `x` therefore reads an aliased range's old backing. §3f
foresaw none of this; the implementer documented it at the method, and it is a Known limit. Nothing
on rung 8's path reads a trampoline page by VA, so it costs nothing measured.
