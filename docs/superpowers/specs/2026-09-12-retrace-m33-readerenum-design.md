# M33-readerenum — one table, five views, and a syscall that cannot be forwarded unclassified

**Charter entry:** `docs/superpowers/specs/2026-09-09-retrace-m32-m38-program-charter-design.md`
§3, "M33 — `readerenum`". **Re-scoped by M32's closing ruling** (status-log, "The finding that
replaces the deliverable"): the successor to M32 is a *unification* of the argument tables, not
another view. This spec reconciles the two: the unification is the mechanism, and the charter's two
halves — enumerate the reader syscalls, make an unlisted syscall fail loud — are what the mechanism
delivers. Written autonomously from the charter under §5's authority; §9's contract is met in §1
(wall), §4 (measurement), §8 (symmetry), §6 (positive controls) and §7 (deliberately not done).

## 1. The wall, located

Five functions in `crates/retrace-arch/src/lib.rs` answer one question — *what does this syscall do
with each of its arguments?* — in five incompatible shapes:

| function | line at `e13eb17` | shape | keyed by |
|---|---|---|---|
| `fd_operands` | 120 | `&'static [usize]` | argument index |
| `allocates_fd` | 148 | `bool` | the return value |
| `dest_buffer` | 176 | `Option<(usize, DestLen)>` | argument index + length source |
| `writes_via_nested_pointer` | 242 | `bool` | whole syscall |
| `reads_guest_buffer` | 308 | `bool` | whole syscall |

Two consequences, both already paid for:

- **Silence.** `fd_operands` returns `&[]` for a syscall it has never heard of, and
  `Box_::translate_fds` (`crates/retrace-box/src/lib.rs:3104`, the first thing
  `forward_and_diff` does) then forwards the raw guest descriptor, which acts on *retrace's own*
  descriptor of that number. Nothing fails; the guest reads the wrong file. M10's doc comment
  states the rule this breaks ("absence must mean provably takes no fd, never not gotten to yet"),
  and the rule has been broken three times since — M9 (console), M25 (`getdirentries64`), M27
  (`pread_nocancel`) — each caught by a guest that happened to hit it, never by the table.
- **Contradiction between the tables.** M30 added `pwrite`, `writev`, `sendmsg`, `sendto_nocancel`,
  `sendfile` and the `readv`/`recvmsg` family to `reads_guest_buffer` /
  `writes_via_nested_pointer` from their prototypes. Every one of them takes a descriptor in `x0`;
  **none is in `fd_operands`.** So the tree today says, of one syscall, both "the kernel reads a
  guest buffer through it" and "it takes no fd" — the second being false. This is the defect in the
  *schema* M32 named: two tables keep the argument index and two threw it away, so nothing can
  check one against another. The count is 16 syscalls; §4 fixes the number by measurement and §5's
  equivalence sweep lists every one.

## 2. Scope

**In:**

1. One table, `arg_kinds(num) -> Option<&'static Shape>`, where a `Shape` is a slice of per-argument
   `ArgKind`s plus a return kind. The five functions above become **views** derived from it, with
   their names and (except `fd_operands`) their signatures unchanged, so no consumer moves.
2. An **equivalence sweep** proving each view still answers, for every syscall number, exactly what
   its hand-written predecessor answered — *modulo an explicit, reviewed list of differences*, each
   naming its reason. A row is written from the C prototype; it is never bent to match a legacy
   table. The sweep's job is to make every disagreement loud, not to forbid it.
3. **Fail loud at the forward point.** `forwarded_shape(num)` panics on `None`, and
   `translate_fds` goes through it. A syscall with no row cannot be forwarded. The charter's second
   half, done structurally rather than by adding a check to one table.
4. **The reader enumeration.** A row for every syscall number any guest in this repo's corpora is
   measured to dispatch (§4), each pointer argument classified `Source` / `Path` / `Ptr` / … under
   §3's rules, with the bound cited where a bound is the argument. The charter's first half.
5. The Apple sweep re-run and re-baselined; the gate; README, status-log, memory.

**Out** (§7 has the reasons): any new `Dest` row beyond the ten legacy ones (M34's job); the
per-argument canary fill M32 dropped; translating nested pointers; any `TRACE_MAGIC` change.

## 3. Design

### 3a. The kinds

```rust
pub enum ArgKind {
    Scalar,             // not a memory reference
    Fd,                 // a GUEST descriptor: translated before forwarding      (view: fd_operands)
    Path,               // NUL-terminated; the kernel stops at PATH_MAX (1024)
    Source,             // kernel READS a caller-sized buffer through it         (view: reads_guest_buffer)
    NestedSource,       // kernel reads through pointers INSIDE the struct       (view: reads_guest_buffer)
    Dest(DestLen),      // kernel WRITES; length where DestLen says              (view: dest_buffer)
    NestedDest,         // kernel writes through pointers INSIDE the struct      (view: writes_via_nested_pointer)
    Ptr,                // a pointer modelled no further than the flat window + guard band
}
pub enum Ret { Plain, Fd /* view: allocates_fd */ }
pub struct Shape { pub args: &'static [ArgKind], pub ret: Ret }
```

`DestLen` is reused unchanged. `NestedSource` is new and deliberate: M30's doc comment already
separates "the kernel reads each `iov_base`" (an untranslated read — wrong data or `EFAULT`) from
"the kernel writes through `iov_base`" (a wild write into retrace's process, refused by value). Two
hazards, two kinds; a schema that collapsed them would be a third view of a fourth question.

**Which kinds are load-bearing.** `Fd`, `Source`, `NestedSource`, `Dest`, `NestedDest` and
`Ret::Fd` each change what the box does. `Scalar`, `Path` and `Ptr` change **nothing** at runtime —
`forward_and_diff` probes `host_span` on all eight registers regardless — and are documentation
until a later milestone consults them. A misclassification among those three is a documentation
error; a misclassification of a load-bearing kind is a behaviour change, and the equivalence sweep
(§5) forces every such change through the reviewed difference list.

### 3b. The classification rules

Per pointer argument, in this order:

1. The kernel follows a pointer *inside* the pointed-to struct (`iovec`, `msghdr`, `sf_hdtr`):
   `NestedDest` if it writes through it, `NestedSource` if it reads. `posix_spawn`'s `argv`/`envp`
   are `NestedSource`.
2. The kernel writes through it and the length is an argument (`DestLen::Reg`) or a guest `u64`
   (`DestLen::DerefU64`) **and the row is one of the ten legacy `dest_buffer` members**: `Dest`.
   No other argument becomes `Dest` in M33 — see §7.
3. NUL-terminated and stopped at `PATH_MAX`: `Path`.
4. The kernel reads it for a length the **caller** chooses and **no kernel-side cap below 64 KiB
   can be cited**: `Source`. This is M30's membership rule verbatim.
5. A citable bound exists — a fixed struct (`struct stat`, 144 bytes), a kernel cap
   (`SOCK_MAXADDRLEN` 255 for a `sockaddr`, `IOCPARM_MASK` 8191 for `ioctl`'s copy-in/out,
   `CTL_MAXNAME` for a sysctl name), or an in/out scalar: `Ptr`, **with the bound and its
   citation in the row comment.** "The bound is the argument" — the same argument
   `reads_guest_buffer`'s doc comment makes for paths.
6. **A bound that cannot be cited is not a bound.** Then rule 4 applies and the argument is
   `Source`. Listing wins — M30's asymmetry, unchanged — and the row says so.

Register arguments that are not pointers are `Scalar`; a descriptor is `Fd` (including `dirfd`
positions, where `AT_FDCWD` passes through translation untouched as today); a return that is a new
guest descriptor is `Ret::Fd` (`dup2` stays `Ret::Plain`: it names its own slot and is asserted
unmodelled in retrace-core, exactly as `allocates_fd` documents).

**Every row carries its C prototype as its comment.** That is the answer to the memory note's "18
of 38 memberships are bare integers": the number is named where it is used, and no new `SYS_*`
constants are invented for rows that only the table references.

### 3c. Where the loudness lives, and why only there

```rust
pub fn forwarded_shape(num: u64) -> &'static Shape {
    arg_kinds(num).unwrap_or_else(|| panic!("M33: syscall {num} ({}) has no arg_kinds row …", num as i64))
}
```

`translate_fds` calls it; `translate_fds` is the first statement of `forward_and_diff`; so no
syscall reaches the host kernel through the generic forward path without a row. Every *other*
consultation of a view inside `forward_and_diff` (`diff_window`'s `dest_buffer`, the canary
decision's `reads_guest_buffer`, the return binding's `allocates_fd`) is downstream of that check
and therefore sees only enumerated numbers.

The views themselves stay **silent** on `None` — they return the empty answer their predecessors
returned — because they are also consulted *outside* the forward path: `allocates_fd` on every
replayed `Event::Syscall` in `ReplaySession` and in `fdtable_e2e`/`fdreplay`, including events for
syscalls the box emulates above the trace (`mmap`, the thread and signal families). Those never
pass through `translate_fds`, need no row, and must not start panicking on replay of a recording
the recorder accepted. One loud site, upstream of everything it protects; nothing loud on replay.

## 4. Measurement — task 1, before any edit

Three measurements, all recorded in the status-log section. The charter's generalisable lesson
applies: no row is seeded for a number nothing dispatches, *except* the legacy members, which
exist today and whose rows are what the equivalence sweep checks.

**4a. The census.** Every syscall number dispatched, as the union over:

- every repo-owned guest (`crates/retrace-guest/src/lib.rs`'s path constants), static ones via
  `record`, dynamic ones via `record-dyn`, with the argv the e2e tests give them;
- `jq --version`, `jq . <file>`, the real CPython interpreter and its launcher (the four e2e
  invocations), `/bin/ps`;
- all 54 binaries of `tools/apple-sweep-binaries.txt`, record side only, under the sweep's 30 s
  watchdog.

Taken from `RETRACE_TRACE=1`'s `[trap] num=` lines (`crates/retrace-core/src/lib.rs:150`), which
print for **every** syscall stop before routing, so the census over-approximates the set that
reaches `forward_and_diff`. Over-approximation is harmless — a row for an emulated syscall is
documentation — and it is the safe direction: an under-approximation is a gate guest that panics
in Task 6. A calibration taken while writing this spec (jq + both CPython paths, the pre-M32
debug binary) found **75 distinct numbers, 15 of them mach traps**; the full census is expected
near 120.

The census is committed as `crates/retrace-arch/tests/census.rs` — a `const` slice with its
provenance (date, guests, counts) — and the test *every census number has a row* is what makes
the loud failure unable to fire on anything the corpora dispatch.

**4b. `ioctl` request codes.** For every `[trap] num=54` line, `args[1]`. Decode each with
`IOCPARM_LEN` / `IOC_IN` / `IOC_OUT` (`sys/ioccom.h:74-76`). The question is whether any request
the corpora issue carries a *nested* pointer (`SIOCGIFCONF`'s `ifc_buf`, the class M27 refuses) —
if one does, that is a `Ruling:` and the request code joins a refuse-by-value assert in
retrace-core beside `writes_via_nested_pointer`'s; if none does, `ioctl`'s third argument is
`Ptr` with the `IOCPARM_MASK` bound, and the README's "named hole" paragraph is rewritten to say
what the residual actually is (driver-specific nested pointers, unmeasured), which is narrower
than "no rule over `num` can size it".

**4c. `sysctl` / `sysctlbyname` `newp`.** For every `[trap] num=202` and `num=274` line, whether
`args[4] != 0` and `args[5]`'s value. This decides whether the per-argument case M32 measured
inert for `mach_msg2` is *live* for `sysctl`: a `Source` on `newp` withholds the canary from the
whole call, including its `Dest` at `oldp`, whose backing is measured to exceed 64 KiB
(`KERN_PROC_ALL`, 205,416 bytes, M26). If the corpora never pass a non-null `newp`, the argument
stays `Ptr` with xnu's per-handler `newlen` check as the cited bound (rule 5); if they do, it is
`Source` under rule 6 and the status-log records the coverage cost as the first non-inert case
of M32's dropped mechanism — a re-scope candidate for a later milestone, **not** scope for this
one.

## 5. What must change

### 5a. `crates/retrace-arch/src/lib.rs`

- Add `ArgKind`, `Ret`, `Shape`, `arg_kinds`, `forwarded_shape`, and the five views as thin
  derivations. `fd_operands` changes signature to `impl Iterator<Item = usize>` — a
  `&'static [usize]` cannot be derived at runtime without a lookup table of every index subset.
  Its three consumers (`translate_fds`, `crates/retrace-box/tests/fdxlat.rs:13,106`, the unit
  tests in this file) adapt with `for i in` / `.collect::<Vec<_>>()`.
- The five doc comments are load-bearing history (M10, M26, M27, M29, M30, M32) and **move, not
  vanish**: each goes onto the `ArgKind` variant that now carries the notion, and the per-syscall
  reasoning goes onto the row. The M32 measurement paragraph on `mach_msg2` goes onto its row.
- Rows are written in two waves so each has its own test cycle: the 52 legacy members first (the
  union of the five tables), then the census remainder.

### 5b. `crates/retrace-arch/tests/legacy_equivalence.rs` (new)

The five bodies at `e13eb17`, copied verbatim as `legacy_*` — a test fixture, not production. The
sweep runs every view against its legacy over the domain `0..=1023` ∪ `-1..=-128` (as `u64`) ∪
`0x8000_0000..=0x8000_000f`, and asserts `derived == legacy` **unless** `(num, view)` is in
`EXPECTED_DIFFS: &[(u64, View, &str)]`, in which case it asserts `derived != legacy` (a stale
entry fails too). Each entry's string is its reason, and entries for numbers the census does not
contain are marked `unexercised` so no one reads them as measured fixes. The list is not only
`fd_operands`: a census row can add a *reader* the M30 list lacked — `posix_spawn` reads every
`argv`/`envp` string through nested pointers — and that goes on the same list under the same rule.

### 5c. `crates/retrace-arch/tests/census.rs` (new) — §4a.

### 5d. `crates/retrace-box/src/lib.rs`

`translate_fds` (`:3104`): `for i in retrace_arch::forwarded_shape(num).fd_operands()`. Nothing
else in the box changes — `diff_window`, the canary decision and `bind_returned_fd` keep calling
the views by their existing names.

### 5e. `crates/retrace-core/src/lib.rs`

Nothing. The `writes_via_nested_pointer` assert at `:1159` keeps its name and its semantics; only
its body's provenance changed.

### 5f. `crates/retrace-guest/asm/unenum.s` + `crates/retrace/tests/unenum_e2e.rs` (new)

A guest that issues syscall **8** — the kernel's `nosys` slot (`old creat`), the one number that
can never legitimately gain a row — then exits. The e2e asserts the recorder's stderr carries the
M33 message. It asserts on the *message*, never on the exit code: under positive control 2 the
call forwards, the kernel returns `ENOSYS`, the guest exits with it, and `record` exits 0 — an
exit-code assertion would then be red for the wrong reason, and a panic-exit assertion would be
green for any panic at all.

### 5g. Docs

README "What works today" / "Known limits" (the `fd_operands` complaint, the `ioctl` hole, the
sweep tally, the gate line, the M32 "shape of the owed work" paragraph — now done); a new
status-log section; the `retrace-argkinds-successor` memory note marked resolved.

## 6. Positive controls

Each is run, its red output recorded in the task report, and the mutation reverted before commit.

1. **The sweep can see a row change.** In `arg_kinds`, change `dup2`'s row from `[Fd, Fd]` to
   `[Fd, Scalar]`. `legacy_equivalence::every_view_reproduces_its_legacy_table` must go RED naming
   `num=90 view=FdOperands`. Second half of the same control: delete one `EXPECTED_DIFFS` entry
   (`pwrite`, 154) — the test must go RED naming 154 as an *unlisted* difference. A sweep that
   only checks one direction would let the difference list rot.
2. **The loudness is wired to the forward point.** Make `forwarded_shape` return a static empty
   `Shape` instead of panicking. `unenum_e2e` must go RED (no message on stderr), and the
   retrace-arch unit test `an_unenumerated_syscall_panics_by_name` (`#[should_panic]`) must go RED.
   Both, because the unit test proves the function and the e2e proves the *call site*: a
   `forwarded_shape` nobody calls passes the first and fails the second.
3. **The census guards the gate.** Remove one census number's row (e.g. `getentropy`, 500).
   `census::every_census_number_has_a_row` must go RED naming 500, **and** — run once, recorded,
   because it costs a real recording — `cpython_e2e` must go RED with the M33 message. The first
   is the cheap guard; the second is the proof that the cheap guard is guarding the right thing.

## 7. What this milestone deliberately does not do

- **No new `Dest` rows.** `proc_info`, `getattrlist`/`fgetattrlist`, `csops` stay `Ptr` with a
  comment naming them as M34's. `dest_buffer` widens the forwarded-count clamp *and* the diff
  window; a wrong length is the M26 truncation class, and each of those three is a measurement
  the charter assigns to M34, "the milestone most exposed to right-conclusion-unmeasured-fact".
- **No per-argument canary fill.** The schema can now say "fill past argument 2, not argument 4"
  — that is the field M32 wanted — but nothing consults it. M32's Control 1 stays unexecuted and
  owed with the mechanism; §4c says when it stops being inert.
- **No nested-pointer translation.** `NestedSource` rows are forwarded exactly as today (the
  untranslated-read hazard M30 named and did not fix); `NestedDest` rows are refused exactly as
  today.
- **Rows only for what is dispatched or already tabled.** An unenumerated syscall is not a bug
  in this table; it is the loud failure working. The sweep re-run (§9) is where that shows.
- **`Scalar` vs `Ptr` is not verified by anything but the reviewer.** Stated in §3a; repeated here
  so the row count is not mistaken for a coverage claim.

## 8. Symmetry obligation

No trap-handling arm is added or changed. The views are pure functions of `num`, so where a view
is consulted on both sides (`writes_via_nested_pointer` in the record arm, `allocates_fd` in both
dispatch loops) it computes the same answer on both — rule 2, below the trace. The loud failure is
record-only *by construction*: replay never calls `forward_and_diff`, so there is no replay mirror
to keep in step and no recorded byte changes. `TRACE_MAGIC` is untouched; if any edit changes a
recorded byte, that is the charter's halt condition, not a judgment call.

The one behavioural change on the record side — descriptors now translated for the 16 syscalls in
§1's second bullet — changes what the *host kernel* sees, not what is recorded: the trace carries
guest descriptors before and after (M10's contract, `forward_and_diff`'s doc comment).

## 9. The sweep re-baseline, and rulings

After Task 5, `tools/apple-sweep.sh` runs against a fresh build and its `TALLY` line is compared
with `pass=46 fail=8 skip=0`. Because the census included all 54 binaries, no binary should move
for lack of a row. Then:

- A binary that moves **PASS → `recorder panicked`** with the M33 message means the census missed
  a number (the README notes one binary moves between runs). Its number gets a row under §3b and
  the sweep is re-run once; if it moves again it is an M36 row, per the charter.
- A binary that moves **`replay diverged` → PASS** is one of the five M23-group divergences whose
  cause "stands unmeasured to this day": a newly translated descriptor would be that cause. Record
  it as a `Ruling:` with the syscall named; it is the charter's own hypothesis (M26's `/bin/ps`)
  paying again.
- Any other movement is a `Ruling:` and an M36 row, not an M33 fix.

**Halt conditions** (charter §5): a red gate surviving one fix round; any `TRACE_MAGIC` bump; any
need for scope this spec lacks — specifically, a corpus `ioctl` with a nested pointer (§4b) is a
*ruling plus an assert*, not a halt, but a corpus guest that *needs* that ioctl to reach its exit
is a halt.

## 10. Gate

The full chunked gate, `--bins` included, `cargo test -p retrace-arch --doc` beside any per-target
split of a library crate, counts reconciled file-by-file against **556 / 0 / 2 over 121**. Expected
binaries: +3 (`legacy_equivalence`, `census`, `unenum_e2e`) → 124. The two `#[ignore]`s are
untouched.
