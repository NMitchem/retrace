# M33-readerenum Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the five per-syscall argument tables in `retrace-arch` with one `arg_kinds` table
the five become views of, prove the views equivalent to their predecessors modulo a reviewed
difference list, make a syscall with no row fail loud at the forward point, and give every syscall
the repo's corpora dispatch a row with its reader arguments classified.

**Architecture:** `retrace-arch` gains `ArgKind`/`Ret`/`Shape` and `arg_kinds(num) ->
Option<&'static Shape>`; `fd_operands`, `allocates_fd`, `dest_buffer`, `writes_via_nested_pointer`
and `reads_guest_buffer` become one-line derivations. `forwarded_shape(num)` panics on `None` and
`Box_::translate_fds` — the first statement of `forward_and_diff` — goes through it. A verbatim copy
of the five legacy bodies lives in a test as the equivalence oracle, with an explicit
`EXPECTED_DIFFS` list that must match the disagreements exactly in both directions. Rows are seeded
from a census of every syscall number the corpora dispatch, taken before any edit.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, `clang` for the asm
guest. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-12-retrace-m33-readerenum-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test` invocation (one VM per process).
- **Target is pinned** by `.cargo/config.toml`; do not override it or the codesigning `runner`.
- **A row is written from the C prototype and never bent to match a legacy table** (spec §2.2).
  Every disagreement goes into `EXPECTED_DIFFS` with a reason; the sweep fails on an unlisted
  difference *and* on a listed one that no longer differs.
- **Classification rules are spec §3b, applied in order.** A bound that cannot be cited is not a
  bound: the argument is `Source` (listing wins). Every `Ptr` row comment names its bound.
- **No new `Dest` rows** beyond the ten legacy members (`proc_info`, `getattrlist`, `csops` are
  M34's). **No per-argument canary fill.** **No `TRACE_MAGIC` bump** — if an edit changes a
  recorded byte, stop: charter §5 halt.
- **Do not push.** Commit and merge locally only (charter §5).
- **Measure before editing.** Task 1 runs to completion before Task 2 starts; spec §4.
- Every long-running command runs in the **foreground under a bounded timeout** and its output is
  read back (M32's 8.5-idle-hour lesson). Chunk anything that could exceed 10 minutes.
- During Tasks 1–5 run targeted tests only; the full chunked gate runs once, in Task 6.
- Branch: `git checkout -b m33-readerenum` from `main` at `e13eb17` (or later) before Task 1.

---

### Task 1: The census — measure before any edit

**Files:**
- Create: `crates/retrace-arch/tests/census.rs`
- Scratch (not committed): `<scratchpad>/census/census.sh` and its outputs; the raw outputs are
  copied into `.superpowers/sdd/2026-09-12-retrace-m33-readerenum/task-1-census/` for the ledger.

**Interfaces:**
- Produces: `pub const CENSUS: &[i64]` — sorted, deduplicated, every syscall number any corpus guest
  dispatched (mach traps negative). Tasks 3 and 5 read it to label rows `unexercised` or not;
  Task 5's `every_census_number_has_a_row` iterates it.
- Produces (ledger only): the ioctl request-code set and the sysctl `newp` findings (spec §4b, §4c).

- [ ] **Step 1: Build the binary and the guests**

```sh
cd /Users/noahmitchem/Documents/GitHub/retrace
cargo build -p retrace -p retrace-guest 2>&1 | tail -3
ls -td target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1
```
Expected: `Finished`, and one `out` directory printed (the newest; it holds every guest binary
named by `crates/retrace-guest/src/lib.rs`'s path constants).

- [ ] **Step 2: Write the census script**

Save as `<scratchpad>/census/census.sh`. It streams each recorder's stderr through `awk`, so a
guest like `/usr/bin/yes` (millions of `[trap]` lines in 30 s) never lands on disk; `perl`'s
`alarm` is the watchdog because macOS ships no `timeout(1)`.

```sh
#!/bin/sh
# M33 Task 1 census: every syscall number any corpus guest dispatches (record side only).
# One line per guest: "<mode> <guest> <argv> -> <N> numbers". Outputs under $S.
set -u
ROOT=/Users/noahmitchem/Documents/GitHub/retrace
S=${S:-$(dirname "$0")}
RAW=$ROOT/target/aarch64-apple-darwin/debug/retrace
BIN=$S/retrace-census
cp "$RAW" "$BIN" || exit 2
codesign -s - -f --entitlements "$ROOT/retrace.entitlements" "$BIN" >/dev/null 2>&1 || exit 2
GUESTS=$(ls -td "$ROOT"/target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)

# one <record|record-dyn> <guest> [argv...]: 30 s watchdog; stderr -> awk; stdout discarded.
one() {
  mode=$1; guest=$2; shift 2
  tag=$(printf '%s' "$mode-$guest-$*" | tr -c 'A-Za-z0-9' '_')
  if [ $# -gt 0 ]; then set -- -- "$@"; fi
  RETRACE_TRACE=1 perl -e 'alarm shift; exec @ARGV' 30 \
      "$BIN" "$mode" "$guest" -o "$S/t.bin" "$@" 2>&1 >/dev/null \
  | awk -v out="$S/$tag" '
      /^\[trap\] num=/ {
        split($2, a, "="); n = a[2]
        if (!seen[n]++) print n > (out ".nums")
        if (n == 54)              { s = $0; sub(/.*args=\[/, "", s); split(s, f, ","); print f[2] > (out ".ioctl") }
        if (n == 202 || n == 274) { s = $0; sub(/.*args=\[/, "", s); split(s, f, ","); print n "," f[5] "," f[6] > (out ".sysctl") }
      }'
  echo "$mode $guest $* -> $(wc -l < "$S/$tag.nums" 2>/dev/null || echo 0) numbers"
}

# 1. Repo-owned guests: static (no LC_LOAD_DYLINKER) via record, dynamic via record-dyn.
for g in "$GUESTS"/*; do
  [ -f "$g" ] && [ -x "$g" ] || continue
  case "$g" in *.txt|*.bin|*.s|*.o) continue ;; esac
  if otool -l "$g" 2>/dev/null | grep -q LC_LOAD_DYLINKER; then one record-dyn "$g"; else one record "$g"; fi
done
# 2. The e2e invocations that are not repo artifacts (skip loudly if absent).
JQ=/opt/homebrew/bin/jq
PY=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
PYL=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/bin/python3.14
[ -x "$JQ" ]  && { one record-dyn "$JQ" --version; echo '{"a":1}' > "$S/in.json"; one record-dyn "$JQ" . "$S/in.json"; } || echo "SKIP jq (not installed)"
[ -x "$PY" ]  && one record-dyn "$PY" -c 'print(1)' || echo "SKIP CPython interpreter"
[ -x "$PYL" ] && one record-dyn "$PYL" -c 'print(1)' || echo "SKIP CPython launcher"
one record-dyn /bin/ps
# 3. The Apple sweep corpus, record side only.
while read -r b; do
  case "$b" in ''|'#'*) continue ;; esac
  one record-dyn "$b"
done < "$ROOT/tools/apple-sweep-binaries.txt"

cat "$S"/*.nums | sort -n -u > "$S/census.txt"
cat "$S"/*.ioctl 2>/dev/null | sort -u > "$S/ioctl_requests.txt"
cat "$S"/*.sysctl 2>/dev/null | sort -u > "$S/sysctl_newp.txt"
echo "CENSUS $(wc -l < "$S/census.txt") distinct numbers; $(wc -l < "$S/ioctl_requests.txt") ioctl requests; $(wc -l < "$S/sysctl_newp.txt") sysctl rows"
```

- [ ] **Step 3: Run it in the foreground, chunked**

The sweep corpus is 54 binaries under a 30 s cap each; the whole script can exceed the 10-minute
tool ceiling. Run part 1+2 first, then part 3 split in half (edit the `while read` to
`head -27` / `tail -n +28` of the list, or run twice with a `SKIP_TO` counter), each invocation
under `timeout: 600000`. Confirm each chunk's per-guest lines were printed and read back.

```sh
sh <scratchpad>/census/census.sh 2>&1 | tail -80
```
Expected: one `-> N numbers` line per guest, `SKIP` lines only for tools genuinely absent, a
final `CENSUS <count> distinct numbers; …` line. `<count>` is expected near 120 (calibration: 75
over jq + both CPython paths).

- [ ] **Step 4: Read the three findings and write them down**

```sh
cat <scratchpad>/census/census.txt | tr '\n' ' '; echo
cat <scratchpad>/census/ioctl_requests.txt
cat <scratchpad>/census/sysctl_newp.txt
```
For every ioctl request code: decode `IOCPARM_LEN = (req >> 16) & 0x1fff`, `IOC_IN = req & 0x80000000`,
`IOC_OUT = req & 0x40000000`, and name it from `sys/ttycom.h` / `sys/filio.h` / `sys/sockio.h`
(`grep -rn "0x$(printf %x $((req & 0xffff)))"` is the fast way, or match the low byte against
`_IO('t', n)` etc.). Record: does any carry a nested pointer (`ifconf`, `ifreq` with a pointer
member)? For sysctl: does any row have `args[4] != 0x0`? Write all three findings into the task
report, with the raw files copied to the ledger directory. **These findings decide Task 5's
`ioctl` and `sysctl` rows (spec §4b, §4c); do not decide them here.**

- [ ] **Step 5: Commit the census as a test fixture**

Create `crates/retrace-arch/tests/census.rs`. Fill the slice from `census.txt` (mach traps are
already negative in the file; keep them so). Fill the counts from the run.

```rust
//! M33 Task 1 census — every syscall number a guest in this repo's corpora dispatched, measured
//! 2026-09-12 from `RETRACE_TRACE=1`'s `[trap] num=` lines (`crates/retrace-core/src/lib.rs`,
//! `record_box`), record side. That line prints for EVERY syscall stop, before routing, so this
//! OVER-approximates what reaches `forward_and_diff` — the safe direction: a row for an emulated
//! syscall is documentation, a missing row for a forwarded one is a gate guest that panics.
//!
//! Corpora: <N_repo> repo-owned guests (static via `record`, dynamic via `record-dyn`, bare argv),
//! `jq --version`, `jq . <file>`, the CPython interpreter and its launcher, `/bin/ps`, and all 54
//! of `tools/apple-sweep-binaries.txt`. Raw per-guest outputs: `.superpowers/sdd/…/task-1-census/`
//! and the M33 section of `docs/status-log.md`.
//!
//! `i64` because mach traps are negative; convert with `as u64` to look one up.
//! `every_census_number_has_a_row` (Task 5) is what keeps M33's loud failure from firing on
//! anything the corpora dispatch.
pub const CENSUS: &[i64] = &[
    // paste census.txt here, one number per line, ascending
];

#[test]
fn census_is_sorted_and_deduplicated() {
    assert!(CENSUS.windows(2).all(|w| w[0] < w[1]), "CENSUS must be strictly ascending");
    assert!(CENSUS.len() > 60, "a census this small means a corpus was skipped: {}", CENSUS.len());
}
```

```sh
cargo test -p retrace-arch --test census -- --test-threads=1
git add crates/retrace-arch/tests/census.rs
git commit -m "M33 t1: census of every syscall number the corpora dispatch"
```
Expected: 1 passed.

---

### Task 2: The equivalence oracle, before the refactor

**Files:**
- Create: `crates/retrace-arch/tests/legacy_equivalence.rs`

**Interfaces:**
- Produces: `legacy_fd_operands`, `legacy_allocates_fd`, `legacy_dest_buffer`,
  `legacy_writes_via_nested_pointer`, `legacy_reads_guest_buffer` (verbatim copies of the bodies
  at `e13eb17`), `enum View`, `const EXPECTED_DIFFS: &[(u64, View, &str)]`, and the sweep
  `every_view_reproduces_its_legacy_table`. Task 3 changes exactly one line (the `FdOperands`
  comparison) and fills `EXPECTED_DIFFS`.

- [ ] **Step 1: Write the oracle and the sweep**

The five bodies are copied from `git show e13eb17:crates/retrace-arch/src/lib.rs` (lines 120–147,
148–150, 176–240, 242–244, 308–360). Code only; the doc comments stay in the production file.

```rust
//! The five hand-written tables as they stood at `e13eb17`, copied VERBATIM as the equivalence
//! oracle for M33's unification. This is a test fixture, not production: `retrace_arch`'s views
//! derive from `arg_kinds` and these are what they must still answer, entry for entry, except
//! where `EXPECTED_DIFFS` says why not.
//!
//! The sweep checks BOTH directions: a difference with no entry fails, and an entry with no
//! difference fails. A one-directional check would let the difference list rot.
use retrace_arch::*;

pub fn legacy_fd_operands(num: u64) -> &'static [usize] {
    match num {
        SYS_CLOSE | SYS_CLOSE_NOCANCEL | SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD
        | SYS_PREAD_NOCANCEL
        | SYS_WRITE | SYS_WRITE_NOCANCEL | SYS_FCNTL | SYS_FCNTL_NOCANCEL
        | SYS_FSTAT | SYS_FSTAT64 | SYS_LSEEK | SYS_IOCTL | SYS_DUP
        | SYS_CONNECT | SYS_SENDTO | SYS_RECVFROM | SYS_RECVFROM_NOCANCEL | SYS_FGETATTRLIST
        | SYS_OPENAT | SYS_FSTATAT64
        | SYS_FSTATFS64 | SYS_GETDIRENTRIES64 => &[0],
        SYS_DUP2 => &[0, 1],
        SYS_MMAP => &[4],
        _ => &[],
    }
}

pub fn legacy_allocates_fd(num: u64) -> bool {
    matches!(num, SYS_OPEN | SYS_OPEN_NOCANCEL | SYS_OPENAT | SYS_DUP | SYS_SOCKET | SYS_SHM_OPEN)
}

pub fn legacy_dest_buffer(num: u64) -> Option<(usize, DestLen)> {
    match num {
        SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD | SYS_PREAD_NOCANCEL => Some((1, DestLen::Reg(2))),
        SYS_SYSCTL => Some((2, DestLen::DerefU64(3))),
        SYS_GETDIRENTRIES64 => Some((1, DestLen::Reg(2))),
        SYS_GETFSSTAT64 => Some((0, DestLen::Reg(1))),
        SYS_RECVFROM | SYS_RECVFROM_NOCANCEL => Some((1, DestLen::Reg(2))),
        SYS_SYSCTLBYNAME => Some((2, DestLen::DerefU64(3))),
        _ => None,
    }
}

pub fn legacy_writes_via_nested_pointer(num: u64) -> bool {
    matches!(num, 120 | 411 | 27 | 401 | 540 | 480)
}

pub fn legacy_reads_guest_buffer(num: u64) -> bool {
    matches!(num,
        SYS_WRITE | SYS_WRITE_NOCANCEL | 154 | 415
        | 121 | 412 | 541
        | SYS_SENDTO | 413 | 28 | 402 | 481
        | 337
        | 65 | 405
        | 0xffff_ffff_ffff_ffd1 // -47
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View { FdOperands, AllocatesFd, DestBuffer, NestedPointer, ReadsGuestBuffer }
const ALL_VIEWS: [View; 5] =
    [View::FdOperands, View::AllocatesFd, View::DestBuffer, View::NestedPointer, View::ReadsGuestBuffer];

/// Every `(num, view)` where the derived view is KNOWN to disagree with its legacy table, and why.
/// Task 2 leaves this empty (the views ARE the legacy tables); Task 3 fills it.
pub const EXPECTED_DIFFS: &[(u64, View, &str)] = &[];

/// The BSD numbers, the mach traps (negative, as the two's-complement `u64` the trap carries), and
/// the `MAC_SYSCALL_MAGIC` band retrace-core recognises.
fn domain() -> impl Iterator<Item = u64> {
    (0..=1023u64)
        .chain((1..=128i64).map(|n| (-n) as u64))
        .chain(0x8000_0000u64..=0x8000_000f)
}

fn differs(num: u64, view: View) -> bool {
    match view {
        View::FdOperands => fd_operands(num) != legacy_fd_operands(num),
        View::AllocatesFd => allocates_fd(num) != legacy_allocates_fd(num),
        View::DestBuffer => dest_buffer(num) != legacy_dest_buffer(num),
        View::NestedPointer => writes_via_nested_pointer(num) != legacy_writes_via_nested_pointer(num),
        View::ReadsGuestBuffer => reads_guest_buffer(num) != legacy_reads_guest_buffer(num),
    }
}

#[test]
fn every_view_reproduces_its_legacy_table() {
    let mut unlisted = Vec::new();
    let mut stale = Vec::new();
    for num in domain() {
        for view in ALL_VIEWS {
            let d = differs(num, view);
            let listed = EXPECTED_DIFFS.iter().any(|(n, v, _)| *n == num && *v == view);
            if d && !listed { unlisted.push((num as i64, view)); }
            if !d && listed { stale.push((num as i64, view)); }
        }
    }
    assert!(unlisted.is_empty(),
        "views disagree with the legacy tables and no EXPECTED_DIFFS entry says why: {unlisted:?}");
    assert!(stale.is_empty(),
        "EXPECTED_DIFFS entries that no longer differ (stale — delete or explain): {stale:?}");
}

#[test]
fn expected_diffs_name_only_numbers_in_the_domain() {
    let dom: Vec<u64> = domain().collect();
    for (n, v, why) in EXPECTED_DIFFS {
        assert!(dom.contains(n), "EXPECTED_DIFFS entry {n:#x} {v:?} ({why}) is outside the sweep domain");
        assert!(!why.is_empty(), "EXPECTED_DIFFS entry {n:#x} {v:?} has no reason");
    }
}
```

- [ ] **Step 2: Run it — it must be green by identity**

```sh
cargo test -p retrace-arch --test legacy_equivalence -- --test-threads=1
```
Expected: 2 passed. (The views still ARE the legacy bodies; a red here means the copy is wrong.)

- [ ] **Step 3: Prove the sweep can go red before trusting it**

Temporarily change `legacy_dest_buffer`'s `SYS_GETFSSTAT64` arm to `Some((0, DestLen::Reg(2)))`, rerun,
expect RED naming `(347, DestBuffer)` under "unlisted"; revert. Record the red output in the task
report.

- [ ] **Step 4: Commit**

```sh
git add crates/retrace-arch/tests/legacy_equivalence.rs
git commit -m "M33 t2: verbatim legacy tables as the equivalence oracle, sweep green by identity"
```

---

### Task 3: `arg_kinds` — the table, the views, the legacy rows, and control 1

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (lines 100–360: the five tables and their doc comments;
  the unit tests at 748–900)
- Modify: `crates/retrace-arch/tests/legacy_equivalence.rs` (one comparison line; `EXPECTED_DIFFS`)

**Interfaces:**
- Produces: `pub enum ArgKind { Scalar, Fd, Path, Source, NestedSource, Dest(DestLen), NestedDest, Ptr }`,
  `pub enum Ret { Plain, Fd }`, `pub struct Shape { pub args: &'static [ArgKind], pub ret: Ret }`,
  `pub fn arg_kinds(num: u64) -> Option<&'static Shape>`, `pub fn forwarded_shape(num: u64) ->
  &'static Shape` (panics on `None` with a message containing `has no arg_kinds row`),
  `impl Shape { fd_operands(&self) -> impl Iterator<Item = usize> + '_; dest_buffer(&self) ->
  Option<(usize, DestLen)>; reads_guest_buffer(&self) -> bool; writes_via_nested_pointer(&self)
  -> bool; allocates_fd(&self) -> bool }`.
- Changes: `pub fn fd_operands(num: u64) -> impl Iterator<Item = usize>` (was `&'static [usize]`).
  The other four views keep their signatures.
- Consumes: `CENSUS` (Task 1) to label each `EXPECTED_DIFFS` reason `unexercised` or not.

- [ ] **Step 1: Write the failing tests first** — add to the `mod tests` in `lib.rs`:

```rust
    // M33: the table behind the five views.
    #[test]
    fn arg_kinds_reproduces_the_read_family_shape() {
        use ArgKind::*;
        let s = arg_kinds(SYS_READ).expect("read has a row");
        assert_eq!(s.args, &[Fd, Dest(DestLen::Reg(2)), Scalar]);
        assert_eq!(s.ret, Ret::Plain);
        assert_eq!(arg_kinds(SYS_READ), arg_kinds(SYS_READ_NOCANCEL), "the _nocancel spelling shares the row");
        assert_eq!(arg_kinds(SYS_OPEN).unwrap().ret, Ret::Fd);
    }

    #[test]
    #[should_panic(expected = "has no arg_kinds row")]
    fn an_unenumerated_syscall_panics_by_name() {
        // 8 is the kernel's `nosys` slot (old creat): no syscall lives there, so no row ever will.
        let _ = forwarded_shape(8);
    }

    // `dest_buffer` returns ONE destination and the clamp/window consult it — a row with two
    // `Dest` arguments would silently pick the first. The schema forbids it.
    #[test]
    fn no_row_has_more_than_one_dest_argument_or_more_than_eight_arguments() {
        for num in (0..=1023u64).chain((1..=128i64).map(|n| (-n) as u64)) {
            if let Some(s) = arg_kinds(num) {
                assert!(s.args.len() <= 8, "syscall {} has {} arguments", num as i64, s.args.len());
                let dests = s.args.iter().filter(|k| matches!(k, ArgKind::Dest(_))).count();
                assert!(dests <= 1, "syscall {} has {dests} Dest arguments", num as i64);
            }
        }
    }
```

```sh
cargo test -p retrace-arch -- --test-threads=1 arg_kinds 2>&1 | tail -5
```
Expected: compile error (`ArgKind`, `arg_kinds`, `forwarded_shape` undefined).

- [ ] **Step 2: Add the types, the table and the views** — replace the five functions (keep the
`SYS_*` constants and their doc comments above them; move each table's doc comment as described
after the code):

```rust
/// What the kernel does with ONE register argument of a syscall.
///
/// M33's unification. Five functions used to answer "what does this syscall do with each of its
/// arguments" in five incompatible shapes — `fd_operands` and `dest_buffer` keyed by argument
/// index, `reads_guest_buffer` and `writes_via_nested_pointer` by whole syscall (so they lost the
/// index), `allocates_fd` by return value — and nothing could check one against another. M30
/// tabled `pwrite`, `writev`, `sendmsg` and `sendfile` as readers from their prototypes while
/// `fd_operands` still said each "takes no fd". They are VIEWS over this table now
/// (`Shape::fd_operands` etc.), and `tests/legacy_equivalence.rs` proves each still answers what
/// it answered at `e13eb17`, entry for entry, except where its `EXPECTED_DIFFS` says why.
///
/// **Load-bearing kinds:** `Fd`, `Source`, `NestedSource`, `Dest`, `NestedDest` and `Ret::Fd`
/// each change what the box does. **`Scalar`, `Path` and `Ptr` change nothing at runtime** —
/// `forward_and_diff` probes `host_span` on all eight registers regardless — and are
/// documentation until a later milestone consults them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// Not a memory reference: a count, a flag word, an offset, a signal number, a port name.
    Scalar,
    /// A GUEST file descriptor — `translate_fds` rewrites it to the host's before forwarding.
    /// A forgotten `Fd` does not diverge loudly: it forwards a raw guest number that the host
    /// kernel resolves against RETRACE's descriptor table (M10). `dirfd` positions are `Fd` too;
    /// `AT_FDCWD` is negative and passes through translation untouched.
    Fd,
    /// A NUL-terminated path. The kernel stops at `PATH_MAX` (1024), far inside the flat 64 KiB
    /// window, so a path is never a `Source`: the bound is the argument, not "paths are short".
    Path,
    /// The kernel READS a caller-sized buffer through it, and no kernel-side cap below the window
    /// can be cited. (M30's membership rule.) The M30 guard-band canary is withheld from the WHOLE
    /// call when any argument is `Source` or `NestedSource`: a canary in a buffer the kernel reads
    /// reaches the kernel as data — the guest's output is silently wrong on record while record and
    /// replay stay bit-identical, the one failure a determinism oracle cannot see. REPRODUCED at
    /// M30 Task 4: a 128 KiB `write` produced 64 corrupt bytes at `0x10080`, planted by a STALE
    /// register pointing into the buffer — so the exclusion is per call, not per declared argument.
    /// Under-including corrupts silently; over-including costs destination-side coverage on the
    /// same call. Where the two compete, listing wins.
    Source,
    /// The kernel reads through pointers INSIDE the pointed-to struct (`iovec.iov_base`,
    /// `msghdr.msg_iov`, `sf_hdtr`, `posix_spawn`'s `argv`). Not translated — a guest IPA reaches
    /// the kernel as a host address, so the read returns wrong bytes or `EFAULT`. A fidelity
    /// hazard, not a wild write; forwarded exactly as before M33. Counts as a reader for the
    /// canary decision.
    NestedSource,
    /// The kernel WRITES through it and the length is where `DestLen` says. The forwarded-count
    /// clamp and the diff window BOTH consult this: a disagreement is the M26 defect (kernel writes
    /// past what the diff inspects; replay restores stale bytes there, invisibly). Seeded only with
    /// what is measured or SDK-verified (M26, M29); other structurally-capable destinations
    /// (`proc_info`, `getattrlist`, `csops`) stay `Ptr` until M34 measures each, and the M27 guard
    /// band is what makes that safe.
    Dest(DestLen),
    /// The kernel writes through pointers INSIDE the pointed-to struct. `forward_and_diff`
    /// translates only top-level registers, so a guest IPA would reach the kernel AS A HOST
    /// ADDRESS — a potential wild write into retrace's own process, not a fidelity gap. Refused by
    /// value in retrace-core (M27), never forwarded; translating it needs the
    /// `translate_mwl_regions` treatment plus a measurement of the struct layout.
    NestedDest,
    /// A pointer modelled no further than the flat window and the guard band: a read the kernel
    /// itself bounds far inside the window (a `sockaddr`, an `ioctl` parameter), a fixed struct it
    /// writes (`struct stat`), or an in/out scalar. **The row comment names the bound and its
    /// citation.** A bound that cannot be cited is not a bound — then the argument is `Source`.
    Ptr,
}

/// What the return value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ret {
    Plain,
    /// A NEW guest descriptor, bound to a fresh guest slot by `bind_returned_fd`. `dup2` is
    /// deliberately `Plain`: it names its own target slot instead of taking the lowest free one,
    /// so binding its return like the others would put the mapping in the wrong slot; retrace-core
    /// asserts on it rather than modelling it wrong (measured: zero `dup2` in the jq run).
    Fd,
}

/// One syscall's argument kinds, in register order, plus its return kind.
#[derive(Debug, PartialEq, Eq)]
pub struct Shape {
    pub args: &'static [ArgKind],
    pub ret: Ret,
}

impl Shape {
    /// Which operand indices hold a GUEST descriptor (view: the old `fd_operands`).
    pub fn fd_operands(&self) -> impl Iterator<Item = usize> + '_ {
        self.args.iter().enumerate().filter(|(_, k)| **k == ArgKind::Fd).map(|(i, _)| i)
    }
    /// The destination buffer as `(argument index, where its length lives)` (view: `dest_buffer`).
    pub fn dest_buffer(&self) -> Option<(usize, DestLen)> {
        self.args.iter().enumerate().find_map(|(i, k)| match k {
            ArgKind::Dest(len) => Some((i, *len)),
            _ => None,
        })
    }
    /// Does the kernel read guest memory through this call in an amount no window bounds?
    pub fn reads_guest_buffer(&self) -> bool {
        self.args.iter().any(|k| matches!(k, ArgKind::Source | ArgKind::NestedSource))
    }
    /// Does the kernel write through a pointer inside a guest struct? (Refused by value.)
    pub fn writes_via_nested_pointer(&self) -> bool { self.args.contains(&ArgKind::NestedDest) }
    /// Does the return value need binding to a fresh guest fd slot?
    pub fn allocates_fd(&self) -> bool { self.ret == Ret::Fd }
}

/// The table. `None` means UNENUMERATED — no guest in this repo's corpora has been measured to
/// dispatch `num` and no legacy table listed it. `forwarded_shape` turns that into a panic at the
/// forward point; the views below turn it into their empty answer, because they are also consulted
/// on replay for events of syscalls the box emulates above the trace.
///
/// Every row's comment is its C prototype. Rows are written from the prototype and never bent to
/// match a legacy table; `tests/legacy_equivalence.rs` lists each disagreement with its reason.
/// Mach traps are keyed by the two's-complement `u64` the trap carries (`-47` is mach_msg2).
pub fn arg_kinds(num: u64) -> Option<&'static Shape> {
    use ArgKind::*;
    use DestLen::{DerefU64, Reg};
    const P: Ret = Ret::Plain;
    const F: Ret = Ret::Fd;
    macro_rules! row {
        ($ret:expr, [$($k:expr),* $(,)?]) => { Some(&Shape { args: &[$($k),*], ret: $ret }) };
    }
    match num {
        // ---- the read/write families -------------------------------------------------------
        // read(int fd, void *buf, size_t nbyte). `_nocancel` beside its plain form deliberately:
        // macOS libc routinely takes ONLY the _nocancel path (measured in one jq run: read ×0,
        // read_nocancel ×2), and a plain-only table fails silently — how M9's console bug survived.
        SYS_READ | SYS_READ_NOCANCEL => row!(P, [Fd, Dest(Reg(2)), Scalar]),
        // pread(int fd, void *buf, size_t nbyte, off_t offset). 414 was missing from THREE tables
        // at once before M27 (fd, clamp, window); one row cannot be missing from one and not another.
        SYS_PREAD | SYS_PREAD_NOCANCEL => row!(P, [Fd, Dest(Reg(2)), Scalar, Scalar]),
        // write(int fd, const void *buf, size_t nbyte): x1 read for x2 bytes, x2 the caller's —
        // M30's reproduced canary case.
        SYS_WRITE | SYS_WRITE_NOCANCEL => row!(P, [Fd, Source, Scalar]),
        // pwrite(int fd, const void *buf, size_t nbyte, off_t offset) / pwrite_nocancel. M30 tabled
        // both as readers; fd_operands never had either (EXPECTED_DIFFS).
        154 | 415 => row!(P, [Fd, Source, Scalar, Scalar]),
        // writev(int fd, const struct iovec *iov, int iovcnt) / writev_nocancel: the kernel reads
        // each iov_base for iov_len — nested, caller-sized, untranslated (M30).
        121 | 412 => row!(P, [Fd, NestedSource, Scalar]),
        // pwritev(int fd, const struct iovec *iov, int iovcnt, off_t offset)
        541 => row!(P, [Fd, NestedSource, Scalar, Scalar]),
        // readv(int fd, struct iovec *iov, int iovcnt) / readv_nocancel: the kernel WRITES through
        // iov_base — refused by value in retrace-core before translate_fds ever runs (M27).
        120 | 411 => row!(P, [Fd, NestedDest, Scalar]),
        // preadv(int fd, struct iovec *iov, int iovcnt, off_t offset)
        540 => row!(P, [Fd, NestedDest, Scalar, Scalar]),
        // ---- sockets ------------------------------------------------------------------------
        // recvmsg(int s, struct msghdr *msg, int flags) / recvmsg_nocancel: msg_iov is nested.
        27 | 401 => row!(P, [Fd, NestedDest, Scalar]),
        // recvmsg_x(int s, struct msghdr_x *msgp, u_int cnt, int flags)
        480 => row!(P, [Fd, NestedDest, Scalar, Scalar]),
        // sendmsg(int s, const struct msghdr *msg, int flags) / sendmsg_nocancel
        28 | 402 => row!(P, [Fd, NestedSource, Scalar]),
        // sendmsg_x(int s, const struct msghdr_x *msgp, u_int cnt, int flags)
        481 => row!(P, [Fd, NestedSource, Scalar, Scalar]),
        // sendto(int s, const void *buf, size_t len, int flags, const struct sockaddr *to,
        //        socklen_t tolen). `to`: the kernel rejects tolen > SOCK_MAXADDRLEN (255,
        // sys/socket.h) — a cited bound, so Ptr. 413 is the _nocancel spelling: it was in
        // reads_guest_buffer and not in fd_operands — the _nocancel trap a fourth time.
        SYS_SENDTO | 413 => row!(P, [Fd, Source, Scalar, Scalar, Ptr, Scalar]),
        // recvfrom(int s, void *buf, size_t len, int flags, struct sockaddr *from,
        //          socklen_t *fromlen). `from` is capped by the kernel at the real address size
        // (≤ sockaddr_storage, 128 B), NOT at *fromlen; *fromlen is 4 bytes — both far inside the
        // flat window (M29).
        SYS_RECVFROM | SYS_RECVFROM_NOCANCEL => row!(P, [Fd, Dest(Reg(2)), Scalar, Scalar, Ptr, Ptr]),
        // connect(int s, const struct sockaddr *name, socklen_t namelen): namelen > SOCK_MAXADDRLEN
        // (255) is rejected — the cited bound.
        SYS_CONNECT => row!(P, [Fd, Ptr, Scalar]),
        // socket(int domain, int type, int protocol) → a NEW descriptor: guest fds are not files-only.
        SYS_SOCKET => row!(F, [Scalar, Scalar, Scalar]),
        // sendfile(int fd, int s, off_t offset, off_t *len, struct sf_hdtr *hdtr, int flags).
        // No guest in this repo issues 337 (M32 §2, by grep) — every kind here is header truth,
        // unexercised. *len is in-out, 8 bytes; hdtr's iovecs are the nested read M30 listed.
        337 => row!(P, [Fd, Fd, Scalar, Ptr, NestedSource, Scalar]),
        // ---- memory -------------------------------------------------------------------------
        // msync(void *addr, size_t len, int flags) / msync_nocancel: listed by M30 on INFERENCE
        // (the kernel reads the range to flush it); all guest memory is anonymous, which probably
        // makes the read moot, but listing costs nothing and a silent corruption follows if the
        // inference is wrong.
        65 | 405 => row!(P, [Source, Scalar, Scalar]),
        // mmap(void *addr, size_t len, int prot, int flags, int fd, off_t offset): the fd is x4,
        // consumed by guest_mmap_file, which translates for itself and never reaches
        // forward_and_diff — the exception that makes a single choke point insufficient.
        SYS_MMAP => row!(P, [Scalar, Scalar, Scalar, Scalar, Fd, Scalar]),
        // ---- mach ---------------------------------------------------------------------------
        // mach_msg2_trap(void *msg, u64 options, u64 bits_and_send_size, u64 remote_and_local,
        //   u64 voucher_and_id, u64 desc_count_and_rcv_name, u64 rcv_size_and_priority, u64 timeout)
        // The kernel reads the message at x0; retrace-core bounds send_size to SEND_SIZE_MAX (4 KiB)
        // by assert before route() runs. The receive buffer is the SAME pointer and a live
        // destination (3405 task_info, 412 host_get_special_port), unwidened — M32 walked 35 real
        // landmarks across hello_dyn, jq and CPython: 13 were Route::Forward, their maximum `avail`
        // was 24,672 bytes against the 65,536 a band needs to exist, zero bands. The entry stays on
        // that measurement, not on the hazard it used to cite (the "nothing relates a band to the
        // message's extent" sentence, disproved at M32 and left named per CLAUDE.md's rule). See
        // spec 2026-09-09-retrace-m32-dirtable-design.md §9. Two's-complement of -47.
        0xffff_ffff_ffff_ffd1 => row!(P, [Source, Scalar, Scalar, Scalar, Scalar, Scalar, Scalar, Scalar]),
        // ---- descriptors --------------------------------------------------------------------
        // close(int fd) / close_nocancel
        SYS_CLOSE | SYS_CLOSE_NOCANCEL => row!(P, [Fd]),
        // dup(int fd) → a NEW descriptor
        SYS_DUP => row!(F, [Fd]),
        // dup2(int fd, int fd2): both are descriptors; the return is NOT bound (see Ret::Fd).
        SYS_DUP2 => row!(P, [Fd, Fd]),
        // fcntl(int fd, int cmd, ...) / fcntl_nocancel. The third argument is cmd-dependent: an int
        // for F_GETFL/F_SETFD/F_DUPFD, a pointer for F_GETPATH (writes ≤ MAXPATHLEN 1024) and
        // F_PREALLOCATE (a 32-byte fstore_t, in-out) — every pointer case far inside the window.
        SYS_FCNTL | SYS_FCNTL_NOCANCEL => row!(P, [Fd, Scalar, Ptr]),
        // fstat(int fd, struct stat *buf) / fstat64: a fixed 144-byte struct (sys/stat.h).
        SYS_FSTAT | SYS_FSTAT64 => row!(P, [Fd, Ptr]),
        // fstatfs64(int fd, struct statfs *buf): a fixed 2168-byte struct (measured at M29 Task 7).
        SYS_FSTATFS64 => row!(P, [Fd, Ptr]),
        // lseek(int fd, off_t offset, int whence)
        SYS_LSEEK => row!(P, [Fd, Scalar, Scalar]),
        // ioctl(int fd, unsigned long request, void *arg): the kernel copies IOCPARM_LEN(request)
        // bytes in and/or out, at most IOCPARM_MASK = 0x1fff (sys/ioccom.h:74) — the cited bound
        // on the DIRECT parameter. Task 5 decides the nested-pointer residual from the census's
        // request codes (spec §4b).
        SYS_IOCTL => row!(P, [Fd, Scalar, Ptr]),
        // fgetattrlist(int fd, struct attrlist *alist, void *attrbuf, size_t bufsize, u_long opts):
        // attrbuf is a destination of bufsize bytes — M34's row to widen (spec §7); Ptr until then.
        SYS_FGETATTRLIST => row!(P, [Fd, Ptr, Ptr, Scalar, Scalar]),
        // getdirentries64(int fd, char *buf, u_int bufsize, off_t *basep): x3 gets 8 bytes —
        // unmodelled by decision, far inside the window (M29). Its fd position is MEASURED, not
        // header-derived: the call is not in the SDK (M25 Finding 3).
        SYS_GETDIRENTRIES64 => row!(P, [Fd, Dest(Reg(2)), Scalar, Ptr]),
        // ---- paths --------------------------------------------------------------------------
        // open(const char *path, int flags, mode_t mode) / open_nocancel → a NEW descriptor
        SYS_OPEN | SYS_OPEN_NOCANCEL => row!(F, [Path, Scalar, Scalar]),
        // openat(int dirfd, const char *path, int flags, mode_t mode) → a NEW descriptor
        SYS_OPENAT => row!(F, [Fd, Path, Scalar, Scalar]),
        // fstatat64(int dirfd, const char *path, struct stat *buf, int flag)
        SYS_FSTATAT64 => row!(P, [Fd, Path, Ptr, Scalar]),
        // shm_open(const char *name, int oflag, mode_t mode) → a NEW descriptor
        SYS_SHM_OPEN => row!(F, [Path, Scalar, Scalar]),
        // ---- sysctl -------------------------------------------------------------------------
        // sysctl(int *name, u_int namelen, void *oldp, size_t *oldlenp, void *newp, size_t newlen)
        // name: namelen ints, and the kernel rejects namelen > CTL_MAXNAME (12, sys/sysctl.h).
        // oldp: the destination, *oldlenp its length IN GUEST MEMORY (M26: /bin/ps's KERN_PROC_ALL
        // buffer runs far past the 64 KiB window). oldlenp: 8 bytes in-out. newp: read for newlen
        // bytes — PROVISIONALLY Ptr; Task 5 cites xnu's per-handler newlen check or flips it to
        // Source under rule 6 (spec §4c).
        SYS_SYSCTL => row!(P, [Ptr, Scalar, Dest(DerefU64(3)), Ptr, Ptr, Scalar]),
        // sysctlbyname — the RAW 6-argument shape (name, namelen, oldp, oldlenp, newp, newlen),
        // measured at M29 fix round 1 against the live kernel: indices IDENTICAL to sysctl's, not
        // "one lower" as the libc prototype suggests. name: a string of namelen bytes —
        // PROVISIONALLY Ptr, same Task 5 rule as newp.
        SYS_SYSCTLBYNAME => row!(P, [Ptr, Scalar, Dest(DerefU64(3)), Ptr, Ptr, Scalar]),
        // getfsstat64(struct statfs *buf, int bufsize, int flags): bufsize is BYTES, not a mount
        // count — the reason this entry is easy to get wrong. Structurally able to cross the window
        // at 31 mounts (2168 B each); this machine has 16 (M29).
        SYS_GETFSSTAT64 => row!(P, [Dest(Reg(1)), Scalar, Scalar]),
        _ => None,
    }
}

/// The shape of a syscall about to be FORWARDED — loud on an unenumerated one.
///
/// `translate_fds` calls this first, and `translate_fds` is the first statement of
/// `forward_and_diff`, so no syscall reaches the host kernel through the generic forward path
/// without a row: the M10 class ("a forgotten entry forwards a raw guest fd, silently") is
/// structurally closed. Every other view consulted inside `forward_and_diff` is downstream of
/// this check. Record-only by construction — replay never forwards.
pub fn forwarded_shape(num: u64) -> &'static Shape {
    arg_kinds(num).unwrap_or_else(|| panic!(
        "M33: syscall {num} ({}) has no arg_kinds row in crates/retrace-arch/src/lib.rs — it \
         cannot be forwarded unclassified (an untranslated guest fd would act on retrace's own \
         descriptor of that number). Classify each argument from the SDK prototype under the \
         rules in ArgKind's docs and add the row; if a guest in the corpora dispatches it, add the \
         number to tests/census.rs too.", num as i64))
}

/// Which operand indices of `num` hold a GUEST file descriptor. View over `arg_kinds`; empty for
/// an unenumerated syscall (the loud check is `forwarded_shape`, upstream of every caller in the
/// forward path).
pub fn fd_operands(num: u64) -> impl Iterator<Item = usize> {
    arg_kinds(num).into_iter().flat_map(Shape::fd_operands)
}
/// Does `num`'s RETURN value need binding to a fresh guest fd slot? View over `arg_kinds`.
pub fn allocates_fd(num: u64) -> bool { arg_kinds(num).is_some_and(Shape::allocates_fd) }
/// The destination buffer `num` fills, as `(argument index, where its length lives)`. View.
pub fn dest_buffer(num: u64) -> Option<(usize, DestLen)> { arg_kinds(num)?.dest_buffer() }
/// Refused-by-value family: a destination behind a nested guest pointer. View.
pub fn writes_via_nested_pointer(num: u64) -> bool {
    arg_kinds(num).is_some_and(Shape::writes_via_nested_pointer)
}
/// Does the host kernel READ guest memory through this call, in an amount no window bounds? View.
pub fn reads_guest_buffer(num: u64) -> bool { arg_kinds(num).is_some_and(Shape::reads_guest_buffer) }
```

**Doc-comment migration (do not drop history):** the M10 paragraph on `fd_operands` → `ArgKind::Fd`
and the `read` row; the `allocates_fd` `dup2` paragraph → `Ret::Fd`; the M26/M29 `dest_buffer`
paragraphs → `ArgKind::Dest` and the rows; the M27 nested-pointer text → `ArgKind::NestedDest`;
the whole M30 `reads_guest_buffer` comment → `ArgKind::Source` (the reproduction, the membership
rule, the asymmetry, the path argument) with the `sendfile`/`mach_msg2` "over-including is not
free" paragraph on those two rows, and the M32 measurement paragraph on the `mach_msg2` row (as
above). Keep `DestLen` where it is. If `rustc` rejects the `&Shape { … }` promotion to `'static`
inside `row!` (it should not — every field is a constant expression), switch the macro to emit a
`static` per row: `{ static S: Shape = Shape { … }; Some(&S) }`.

- [ ] **Step 3: Adapt the existing unit tests to the iterator** — in `mod tests`, every
`assert_eq!(fd_operands(X), &[..])` becomes a `Vec` comparison, and `&[] as &[usize]` becomes a
count:

```rust
        // in fd_operands_covers_the_measured_surface:
            assert_eq!(fd_operands(num).collect::<Vec<_>>(), [0], "syscall {num} holds its fd in x0");
        assert_eq!(fd_operands(SYS_MMAP).collect::<Vec<_>>(), [4], "mmap's fd is x4, consumed by guest_mmap_file");
        assert_eq!(fd_operands(SYS_DUP2).collect::<Vec<_>>(), [0, 1]);
            assert_eq!(fd_operands(num).count(), 0, "syscall {num} has no fd operand");
        assert_eq!(fd_operands(SYS_MAP_WITH_LINKING_NP).count(), 0);
        assert_eq!(fd_operands(427).count(), 0, "fsgetpath (427) takes no descriptor");
        // in pread_nocancel_is_treated_exactly_like_pread and nocancel_variants_are_tabled_beside_their_plain_forms:
        assert_eq!(fd_operands(SYS_PREAD_NOCANCEL).collect::<Vec<_>>(), fd_operands(SYS_PREAD).collect::<Vec<_>>());
        // (same shape for READ/READ_NOCANCEL, WRITE/WRITE_NOCANCEL, CLOSE/CLOSE_NOCANCEL, FCNTL/FCNTL_NOCANCEL)
        // in recvfrom_translates_its_socket_fd:
        assert_eq!(fd_operands(SYS_RECVFROM).collect::<Vec<_>>(), [0]);
        assert_eq!(fd_operands(SYS_RECVFROM_NOCANCEL).collect::<Vec<_>>(), [0]);
        assert_eq!(fd_operands(SYS_GETFSSTAT64).count(), 0);
        assert_eq!(fd_operands(SYS_SYSCTLBYNAME).count(), 0);
```

- [ ] **Step 4: Run the unit tests**

```sh
cargo test -p retrace-arch --lib -- --test-threads=1 2>&1 | tail -15
```
Expected: all pass, including the three new ones. If `arg_kinds_reproduces_the_read_family_shape`
fails on `assert_eq!(arg_kinds(SYS_READ), arg_kinds(SYS_READ_NOCANCEL))`, `Shape` needs
`PartialEq` (it has it above).

- [ ] **Step 5: Run the equivalence sweep — expect RED, then list every difference**

```sh
cargo test -p retrace-arch --test legacy_equivalence -- --test-threads=1 2>&1 | tail -8
```
Expected: compile error first — change the one comparison in `differs`:
```rust
        View::FdOperands => fd_operands(num).collect::<Vec<_>>() != legacy_fd_operands(num),
```
then RED naming exactly these 16 `(num, FdOperands)` pairs as unlisted: `154, 415, 121, 412, 541,
413, 28, 402, 481, 337, 120, 411, 27, 401, 540, 480`. Any other pair is a row that does not match
its prototype — fix the row, not the list. Then fill `EXPECTED_DIFFS`. For each entry, check
`CENSUS` (Task 1): if the number is absent, the reason ends in `— unexercised`; if present, it
ends in `— exercised by the census: a live M10-class fix`.

```rust
pub const EXPECTED_DIFFS: &[(u64, View, &str)] = &[
    // M33 finding 1: M30 tabled these as readers from their prototypes, and every one takes a
    // descriptor in x0 that `fd_operands` never translated — the M10 class, present in the tree
    // since M30 and never hit because no corpus guest issues them. The row is header truth; the
    // legacy table was wrong.
    (154, View::FdOperands, "pwrite(fd, …) — unexercised"),
    (415, View::FdOperands, "pwrite_nocancel(fd, …) — unexercised"),
    (121, View::FdOperands, "writev(fd, …) — unexercised"),
    (412, View::FdOperands, "writev_nocancel(fd, …) — unexercised"),
    (541, View::FdOperands, "pwritev(fd, …) — unexercised"),
    (413, View::FdOperands, "sendto_nocancel(s, …): the _nocancel trap a fourth time — unexercised"),
    (28,  View::FdOperands, "sendmsg(s, …) — unexercised"),
    (402, View::FdOperands, "sendmsg_nocancel(s, …) — unexercised"),
    (481, View::FdOperands, "sendmsg_x(s, …) — unexercised"),
    (337, View::FdOperands, "sendfile(fd, s, …): TWO descriptors — unexercised"),
    // M33 finding 1, moot half: the refused family. retrace-core's writes_via_nested_pointer
    // assert fires BEFORE translate_fds runs, so translation never happens; listed because the row
    // is header truth and the sweep must not be taught to lie.
    (120, View::FdOperands, "readv(fd, …) — refused upstream; moot"),
    (411, View::FdOperands, "readv_nocancel(fd, …) — refused upstream; moot"),
    (27,  View::FdOperands, "recvmsg(s, …) — refused upstream; moot"),
    (401, View::FdOperands, "recvmsg_nocancel(s, …) — refused upstream; moot"),
    (540, View::FdOperands, "preadv(fd, …) — refused upstream; moot"),
    (480, View::FdOperands, "recvmsg_x(s, …) — refused upstream; moot"),
];
```

```sh
cargo test -p retrace-arch --test legacy_equivalence -- --test-threads=1 2>&1 | tail -4
```
Expected: 2 passed.

- [ ] **Step 6: Positive control 1, both halves — run, record, revert**

(a) Change `SYS_DUP2`'s row to `row!(P, [Fd, Scalar])`. Run the sweep. Expected RED: unlisted
`[(90, FdOperands)]`. Revert.
(b) Delete the `(154, View::FdOperands, …)` entry. Run the sweep. Expected RED: unlisted
`[(154, FdOperands)]`. Restore.
Paste both red outputs into the task report.

- [ ] **Step 7: Clippy on the crate, then commit**

```sh
cargo clippy -p retrace-arch --all-targets -- -D warnings 2>&1 | tail -3
git add crates/retrace-arch/src/lib.rs crates/retrace-arch/tests/legacy_equivalence.rs
git commit -m "M33 t3: arg_kinds — one table, five views, 52 legacy rows; 16 untranslated fds surfaced"
```

---

### Task 4: Loud at the forward point, and the guest that proves it

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:3104` (`translate_fds`)
- Modify: `crates/retrace-box/tests/fdxlat.rs:13,106`
- Create: `crates/retrace-guest/asm/unenum.s`
- Modify: `crates/retrace-guest/build.rs` (after the `failsys` block, ~line 147)
- Modify: `crates/retrace-guest/src/lib.rs:143` (add `UNENUM` after `FAILSYS`)
- Create: `crates/retrace/tests/unenum_e2e.rs`

**Interfaces:**
- Consumes: `retrace_arch::forwarded_shape`, `Shape::fd_operands` (Task 3); `util::record` (existing).
- Produces: `retrace_guest::UNENUM`.

- [ ] **Step 1: Write the failing e2e**

`crates/retrace-guest/asm/unenum.s`:
```asm
.section __TEXT,__text
.global _start
_start:
    mov  x16, #8              // the kernel's nosys slot (old creat): no syscall, so no row — ever
    svc  #0x80
    mov  x0, #0
    mov  x16, #1              // SYS_exit(0)
    svc  #0x80
```

`crates/retrace-guest/build.rs`, after the `failsys` block:
```rust
    // unenum: issues syscall 8 — the kernel's `nosys` slot — then exits 0. M33's fail-loud e2e
    // fixture: the recorder must refuse to forward a syscall with no arg_kinds row, BY NAME.
    let src = format!("{}/asm/unenum.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/unenum");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang unenum");
    assert!(status.success(), "unenum guest build failed");
```

`crates/retrace-guest/src/lib.rs`, after `FAILSYS`:
```rust
pub const UNENUM: &str = concat!(env!("OUT_DIR"), "/unenum");
```

`crates/retrace/tests/unenum_e2e.rs`:
```rust
mod util;
use retrace_guest::UNENUM;

// M33: a syscall with no `arg_kinds` row must not reach the host kernel. The guest issues 8 — the
// kernel's `nosys` slot (`old creat`), the one number that can never legitimately gain a row —
// and the recorder must refuse it BY NAME.
//
// Asserted on the MESSAGE, never the exit code. Under spec §6 control 2 (`forwarded_shape` made
// silent) the call forwards, the kernel answers ENOSYS, the guest ignores it and exits 0, and so
// does `record`: an exit-code assertion would be red for the wrong reason, and "any panic" would
// be green for any panic at all. The message is the difference this milestone makes.
#[test]
fn an_unenumerated_syscall_is_refused_by_name() {
    let (out, _trace) = util::record(UNENUM);
    assert!(out.stderr.contains("M33: syscall 8 (8) has no arg_kinds row"),
        "recorder did not refuse syscall 8 by name; code={} stderr=\n{}", out.code, out.stderr);
}
```

```sh
cargo test -p retrace --test unenum_e2e -- --test-threads=1 2>&1 | tail -8
```
Expected: FAIL — the guest records to exit 0 (syscall 8 forwarded, ENOSYS ignored) and stderr has
no M33 line.

- [ ] **Step 2: Make `translate_fds` loud**

`crates/retrace-box/src/lib.rs`, in `translate_fds`, replace the loop head and add the comment:
```rust
    pub fn translate_fds(&self, num: u64, args: &mut [u64; 8]) -> Result<(), u64> {
        // M33: a syscall with no arg_kinds row cannot be forwarded. This is the first statement
        // forward_and_diff executes, so the panic sits upstream of every other view consulted
        // there (diff_window's dest_buffer, the canary decision, bind_returned_fd).
        for i in retrace_arch::forwarded_shape(num).fd_operands() {
```
(the loop body is unchanged: `let v = args[i]; …`).

`crates/retrace-box/tests/fdxlat.rs`: line 13 `for &i in fd_operands(num)` → `for i in fd_operands(num)`;
line 106 → `assert_eq!(fd_operands(retrace_arch::SYS_MAP_WITH_LINKING_NP).count(), 0);`.

- [ ] **Step 3: Run the e2e and the box's fd tests**

```sh
cargo test -p retrace --test unenum_e2e -- --test-threads=1 2>&1 | tail -4
cargo test -p retrace-box --test fdxlat -- --test-threads=1 2>&1 | tail -4
cargo test -p retrace-box --test failsys -- --test-threads=1 2>&1 | tail -4
```
Expected: all pass. (`failsys` forwards `open`, which has a row — it is the check that the loud
path did not become loud for enumerated syscalls.)

- [ ] **Step 4: Positive control 2 — run, record, revert**

In `forwarded_shape`, replace the body with
`arg_kinds(num).unwrap_or(&Shape { args: &[], ret: Ret::Plain })`. Run:
```sh
cargo test -p retrace-arch --lib -- --test-threads=1 an_unenumerated 2>&1 | tail -4
cargo test -p retrace --test unenum_e2e -- --test-threads=1 2>&1 | tail -6
```
Expected: BOTH red — the unit test "did not panic as expected", the e2e "did not refuse syscall 8
by name; code=0". Paste both into the task report; revert.

- [ ] **Step 5: Commit**

```sh
cargo clippy -p retrace-box -p retrace-guest -p retrace --all-targets -- -D warnings 2>&1 | tail -3
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/fdxlat.rs crates/retrace-guest/asm/unenum.s \
        crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace/tests/unenum_e2e.rs
git commit -m "M33 t4: forward_and_diff refuses an unenumerated syscall by name; unenum e2e"
```

---

### Task 5: Rows for the census, the reader classification, and control 3

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (`arg_kinds`: the census rows)
- Modify: `crates/retrace-arch/tests/census.rs` (add `every_census_number_has_a_row`)
- Modify: `crates/retrace-arch/tests/legacy_equivalence.rs` (`EXPECTED_DIFFS` if any census row
  gains a load-bearing kind beyond legacy)
- Possibly modify: `crates/retrace-core/src/lib.rs:1159` area (an ioctl refuse-by-value assert —
  only if §4b measured a nested-pointer request)

**Interfaces:**
- Consumes: `CENSUS`, the Task 1 findings, `arg_kinds` (Task 3).

- [ ] **Step 1: Write the failing test** — add to `census.rs`:

```rust
use retrace_arch::arg_kinds;

/// The guard that keeps M33's loud failure from firing on anything the corpora dispatch. A number
/// here with no row is a gate guest — or a sweep binary — that panics at its first forward.
#[test]
fn every_census_number_has_a_row() {
    let missing: Vec<i64> = CENSUS.iter().copied().filter(|&n| arg_kinds(n as u64).is_none()).collect();
    assert!(missing.is_empty(), "census numbers with no arg_kinds row: {missing:?}");
}
```

```sh
cargo test -p retrace-arch --test census -- --test-threads=1 2>&1 | tail -4
```
Expected: RED, listing every census number Task 3 did not cover (≈70).

- [ ] **Step 2: Add a row for every missing number**

Sources of truth, in order: `$(xcrun --show-sdk-path)/usr/include/sys/syscall.h` for BSD
number→name; the SDK's `unistd.h` / `sys/*.h` / `mach/mach_traps.h` for prototypes; xnu's
`osfmk/mach/syscall_sw.h` (open source — the SDK does not ship it) for mach-trap number→name,
cross-checked against the constants retrace-core already has (`-10 vm_allocate`, `-12
vm_deallocate`, `-14 vm_protect`, `-15 vm_map`, `-28 task_self`, `-29 host_self`, `-33/-36`
semaphores, `-47 mach_msg2`); `crates/retrace-core/src/lib.rs` for numbers retrace emulates
(`MAC_SYSCALL_MAGIC = 0x8000_0000`, the thread/signal/workq families). Apply spec §3b in order;
name every `Ptr` bound. Group rows under `// ---- ` headers like Task 3's.

The calibration census (jq + both CPython paths) already fixes these; the Task 1 census adds
numbers beyond them, classified the same way:

```rust
        // ---- process / identity (all scalar) ------------------------------------------------
        // exit(int) — emulated above the trace; the row is documentation.
        SYS_EXIT => row!(P, [Scalar]),
        // getpid(void) / getuid / geteuid / getgid / getegid / issetugid / gettid / thread_selfid
        SYS_GETPID | 24 | 25 | 47 | 43 | 327 | 286 | SYS_THREAD_SELFID => row!(P, []),
        // getrlimit(int resource, struct rlimit *rlp): a fixed 16-byte struct — serviced above the
        // trace for RLIMIT_STACK (M8), forwarded otherwise.
        SYS_GETRLIMIT => row!(P, [Scalar, Ptr]),
        // gettimeofday(struct timeval *tp, struct timezone *tzp): 16 + 8 bytes.
        116 => row!(P, [Ptr, Ptr]),
        // getentropy(void *buf, size_t buflen): the kernel rejects buflen > 256 (sys/random.h) —
        // the cited bound.
        500 => row!(P, [Ptr, Scalar]),
        // sigaction(int sig, const struct sigaction *act, struct sigaction *oact): serviced above
        // the trace (M11); two fixed structs.
        SYS_SIGACTION => row!(P, [Scalar, Ptr, Ptr]),
        // bsdthread_register(threadstart, wqthread, pthsize, dummy, targetconc, dispatchqueue_off):
        // pointers to CODE the kernel records and never dereferences for data — emulated (M14).
        SYS_BSDTHREAD_REGISTER => row!(P, [Scalar, Scalar, Scalar, Scalar, Scalar, Scalar]),
        // ---- paths (kernel stops at PATH_MAX) ----------------------------------------------
        // access(const char *path, int mode)
        33 => row!(P, [Path, Scalar]),
        // readlink(const char *path, char *buf, size_t bufsize): buf is a destination of bufsize
        // bytes, but the kernel writes at most the link's length, ≤ PATH_MAX (1024) — the bound.
        58 => row!(P, [Path, Ptr, Scalar]),
        // stat64(const char *path, struct stat *buf) / lstat64: a fixed 144-byte struct.
        338 | 340 => row!(P, [Path, Ptr]),
        // getattrlist(const char *path, struct attrlist *alist, void *attrbuf, size_t bufsize,
        //             u_long options): attrbuf is M34's destination to widen; Ptr until measured.
        220 => row!(P, [Path, Ptr, Ptr, Scalar, Scalar]),
        // fsgetpath(char *buf, size_t bufsize, fsid_t *fsid, uint64_t objid): buf gets a path,
        // ≤ PATH_MAX; fsid names a VOLUME, 8 bytes read — not a descriptor (M25 census).
        427 => row!(P, [Ptr, Scalar, Ptr, Scalar]),
        // shared_region_check_np(uint64_t *start_address): 8 bytes out — serviced above the trace.
        SYS_SHARED_REGION_CHECK_NP => row!(P, [Ptr]),
        // ---- memory (emulated above the trace; rows are documentation) --------------------
        // munmap(void *addr, size_t len) / mprotect(addr, len, prot) / madvise(addr, len, advice)
        SYS_MUNMAP => row!(P, [Scalar, Scalar]),
        SYS_MPROTECT | 75 => row!(P, [Scalar, Scalar, Scalar]),
        // map_with_linking_np(const struct mwl_region *regions, uint32_t count,
        //                     const struct mwl_file *files, uint32_t nfiles): the fd is INSIDE
        // regions[i].mwlr_fd — translate_mwl_regions handles it; the array is const (read, bounded
        // by MWL_MAX_REGION_COUNT × 32 bytes).
        SYS_MAP_WITH_LINKING_NP => row!(P, [Ptr, Scalar, Ptr, Scalar]),
        // ---- code signing / policy ----------------------------------------------------------
        // csops(pid_t pid, uint32_t ops, void *useraddr, size_t usersize) / csops_audittoken(…,
        // audit_token_t *): useraddr is M34's destination to widen; Ptr until measured.
        169 => row!(P, [Scalar, Scalar, Ptr, Scalar]),
        170 => row!(P, [Scalar, Scalar, Ptr, Scalar, Ptr]),
        // csrctl(uint32_t op, void *useraddr, size_t usersize): op 0 writes a 4-byte config.
        483 => row!(P, [Scalar, Ptr, Scalar]),
        // __mac_syscall(const char *policy, int call, void *arg): policy is a name (≤ MAC_MAX_POLICY_NAME
        // + 1, security/mac.h); arg is policy-defined — Sandbox's is a fixed struct. Ptr on those bounds.
        381 => row!(P, [Path, Scalar, Ptr]),
        // proc_info(int callnum, int pid, uint32_t flavor, uint64_t arg, void *buffer, int buffersize):
        // buffer is M34's destination to widen; Ptr until measured.
        336 => row!(P, [Scalar, Scalar, Scalar, Scalar, Ptr, Scalar]),
        // ---- spawn --------------------------------------------------------------------------
        // posix_spawn(pid_t *pid, const char *path, const struct _posix_spawn_args_desc *desc,
        //             char *const argv[], char *const envp[]): argv/envp are read through nested
        // pointers (the strings). Forwarded today — the launcher-shim gap cpython_e2e pins.
        244 => row!(P, [Ptr, Path, Ptr, NestedSource, NestedSource]),
        // ---- mach traps (two's-complement u64; numbers per xnu osfmk/mach/syscall_sw.h) -------
        // _kernelrpc_mach_vm_allocate_trap(target, mach_vm_offset_t *addr, size, flags): 8 in-out
        MACH_VM_ALLOCATE_TRAP => row!(P, [Scalar, Ptr, Scalar, Scalar]),
        // _kernelrpc_mach_vm_deallocate_trap(target, address, size)
        MACH_VM_DEALLOCATE_TRAP => row!(P, [Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_vm_protect_trap(target, address, size, set_maximum, new_protection)
        MACH_VM_PROTECT_TRAP => row!(P, [Scalar, Scalar, Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_vm_map_trap(target, mach_vm_offset_t *address, size, mask, flags, prot)
        MACH_VM_MAP_TRAP => row!(P, [Scalar, Ptr, Scalar, Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_port_deallocate_trap(target, name) / mod_refs_trap(target, name, right, delta)
        MACH_PORT_DEALLOCATE_TRAP => row!(P, [Scalar, Scalar]),
        MACH_PORT_MOD_REFS_TRAP => row!(P, [Scalar, Scalar, Scalar, Scalar]),
        // _kernelrpc_mach_port_construct_trap(target, mach_port_options_t *options, context,
        //                                     mach_port_name_t *name): a fixed options struct read,
        // 4 bytes written.
        MACH_PORT_CONSTRUCT_TRAP => row!(P, [Scalar, Ptr, Scalar, Ptr]),
        // mach_reply_port() / thread_self_trap() / task_self_trap() / host_self_trap() /
        // thread_get_special_reply_port()
        MACH_REPLY_PORT_TRAP | MACH_THREAD_SELF_TRAP | MACH_TASK_SELF_TRAP | MACH_HOST_SELF_TRAP
        | MACH_THREAD_GET_SPECIAL_REPLY_PORT_TRAP => row!(P, []),
        // host_create_mach_voucher_trap(host, mach_voucher_attr_raw_recipe_array_t recipes,
        //   recipes_size, mach_port_name_t *voucher): recipes read for recipes_size, which the
        // kernel rejects above MACH_VOUCHER_ATTR_MAX_RAW_RECIPE_ARRAY_SIZE (5120, mach/mach_voucher_types.h).
        MACH_HOST_CREATE_MACH_VOUCHER_TRAP => row!(P, [Scalar, Ptr, Scalar, Ptr]),
        // mach_timebase_info_trap(mach_timebase_info_t info): 8 bytes out — retrace answers this
        // from its synthetic timebase.
        MACH_TIMEBASE_INFO_TRAP => row!(P, [Ptr]),
        // MAC_SYSCALL_MAGIC: the platform-syscall band retrace-core recognises and services.
        0x8000_0000 => row!(P, []),
```
**A cast is not a pattern** (`(-10i64) as u64 => …` does not compile), so the mach traps are
matched by name. Define the constants beside the existing `MACH_SEMAPHORE_WAIT`/`_SIGNAL` at
`crates/retrace-arch/src/lib.rs:511`, in the style they use, one per trap the census contains
(numbers per xnu `osfmk/mach/syscall_sw.h`, cross-checked against retrace-core's private
`MACH_VM_ALLOCATE`(-10)/`_DEALLOCATE`(-12)/`_PROTECT`(-14)/`_MAP`(-15)/`MACH_TASK_SELF`(-28)):
```rust
/// Mach traps the corpora dispatch (M33 census), keyed as the two's-complement `u64` the trap
/// carries. Numbers per xnu `osfmk/mach/syscall_sw.h` — the SDK does not ship that header.
pub const MACH_VM_ALLOCATE_TRAP: u64 = (-10i64) as u64;
pub const MACH_VM_DEALLOCATE_TRAP: u64 = (-12i64) as u64;
pub const MACH_VM_PROTECT_TRAP: u64 = (-14i64) as u64;
pub const MACH_VM_MAP_TRAP: u64 = (-15i64) as u64;
pub const MACH_PORT_DEALLOCATE_TRAP: u64 = (-18i64) as u64;
pub const MACH_PORT_MOD_REFS_TRAP: u64 = (-19i64) as u64;
pub const MACH_PORT_CONSTRUCT_TRAP: u64 = (-24i64) as u64;
pub const MACH_REPLY_PORT_TRAP: u64 = (-26i64) as u64;
pub const MACH_THREAD_SELF_TRAP: u64 = (-27i64) as u64;
pub const MACH_TASK_SELF_TRAP: u64 = (-28i64) as u64;
pub const MACH_HOST_SELF_TRAP: u64 = (-29i64) as u64;
pub const MACH_THREAD_GET_SPECIAL_REPLY_PORT_TRAP: u64 = (-50i64) as u64;
pub const MACH_HOST_CREATE_MACH_VOUCHER_TRAP: u64 = (-70i64) as u64;
pub const MACH_TIMEBASE_INFO_TRAP: u64 = (-89i64) as u64;
```
(`MACH_SEMAPHORE_WAIT` and `MACH_SEMAPHORE_SIGNAL` already exist; if the census contains -33/-36,
their rows are `[Scalar]` and use those names.)

**One census row changes a load-bearing view and is pre-listed here:** `posix_spawn` (244) is
`NestedSource` on `argv`/`envp`, so `reads_guest_buffer(244)` becomes `true` where the legacy
table said `false`. That is a correct listing under M30's rule (the kernel reads every argv/envp
string, caller-sized, through nested pointers) and it withholds the canary from a call whose only
destination is a 4-byte `pid_t` — nothing lost. Add to `EXPECTED_DIFFS`:
```rust
    // M33 finding 2: posix_spawn reads argv/envp strings through nested pointers — a reader the
    // M30 list never had. Exercised by the census (the CPython launcher). Its only destination is
    // the 4-byte *pid, so withholding the canary costs nothing.
    (244, View::ReadsGuestBuffer, "posix_spawn(pid, path, desc, argv, envp) — exercised"),
```

For every number in the Task 1 census that is not above: same procedure, same rule order. A
number whose prototype cannot be found in the SDK is measured from its `[trap]` args the way M25
did `getdirentries64`, and the row comment says so.

- [ ] **Step 3: The `ioctl` and `sysctl` decisions (spec §4b, §4c), from Task 1's findings**

- `ioctl`: if no census request code carries a nested pointer, leave the Task 3 row and rewrite
  its comment to cite the finding: "census 2026-09-12: N distinct requests, all `_IOR`/`_IOW`
  with `IOCPARM_LEN ≤ …`, none with a pointer member". If one does, add beside the
  `writes_via_nested_pointer` assert in `crates/retrace-core/src/lib.rs` (~line 1159):
  ```rust
                assert!(!(num == retrace_arch::SYS_IOCTL && retrace_arch::IOCTL_NESTED_REQUESTS.contains(&args[1])),
                    "ioctl request {:#x} carries a pointer inside its parameter struct, which \
                     forward_and_diff never translates (M27 class). Refused; measure the struct \
                     before modelling it.", args[1]);
  ```
  with `pub const IOCTL_NESTED_REQUESTS: &[u64] = &[…]` in retrace-arch, and record a
  `Ruling:` in the ledger. If a gate guest *needs* that request to reach its exit: **halt**.
- `sysctl`/`sysctlbyname` `newp`: if no census row has `args[4] != 0`, keep `Ptr` and replace
  "PROVISIONALLY" with the citation: xnu `bsd/kern/kern_newsysctl.c`, `sysctl_root`'s handlers
  reject a `newlen` that is not the type's size (`sysctl_handle_int`/`_quad`/`_string`/`_opaque`),
  plus "census 2026-09-12: 0 of N calls passed a non-null newp". If one does: `Source`, add
  `(202, View::ReadsGuestBuffer, "…")` to `EXPECTED_DIFFS`, and write the coverage cost into the
  ledger as spec §4c says — the first non-inert case of M32's dropped mechanism, NOT scope here.
  `sysctlbyname`'s `name`: cite `sys_sysctlbyname`'s `namelen` check the same way or flip to
  `Source` under rule 6.

- [ ] **Step 4: Run the three retrace-arch test targets**

```sh
cargo test -p retrace-arch -- --test-threads=1 2>&1 | grep -a 'test result'
```
Expected: three `ok` lines (lib, census, legacy_equivalence) plus doc-tests. If
`every_view_reproduces_its_legacy_table` reds on a census row, that row gave a load-bearing kind to
a number the legacy tables had an opinion on — either the row is wrong or it is a new finding for
`EXPECTED_DIFFS`; decide by the prototype and say which in the entry.

- [ ] **Step 5: The forward path on real guests**

```sh
cargo test -p retrace --test hello_dyn_e2e --test jq_e2e --test jq_file_e2e --test cpython_e2e --test bigread_e2e --test bigwrite_e2e --test thread_rust_e2e -- --test-threads=1 2>&1 | grep -a 'test result\|panicked\|M33'
```
Expected: every target `ok`, no `M33:` line. An `M33:` line names a number the census missed — add
its row AND its census entry (the census file's doc comment says how it was measured; the
addition's comment says it was found by the gate instead).

Then the in-process record loops, which forward syscalls without going through the CLI and so
were not in the census's `[trap]` output (they print no trace lines):
```sh
cargo test -p retrace-core --no-fail-fast -- --test-threads=1 2>&1 | grep -a 'test result\|M33'
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 2>&1 | grep -a 'test result\|M33'
```
Each in the foreground under `timeout: 600000`; split `retrace-box` per-target if it exceeds that
(and note the split — its `Doc-tests` target then needs `cargo test -p retrace-box --doc`
beside it, per CLAUDE.md). Expected: all `ok`. An `M33:` line here is a box/core test that
forwards a syscall no CLI-driven guest issued — same remedy: row + census entry, comment says
which test found it.

- [ ] **Step 6: Positive control 3 — run, record, revert**

Delete the `500 => …` (`getentropy`) row. Run:
```sh
cargo test -p retrace-arch --test census -- --test-threads=1 2>&1 | tail -4
cargo test -p retrace --test cpython_e2e -- --test-threads=1 2>&1 | grep -a 'M33\|test result'
```
Expected: `census` RED naming `[500]`; `cpython_e2e` RED with `M33: syscall 500 (500) has no
arg_kinds row` on its stderr. (The calibration census showed CPython issuing 500. If CPython is
not installed, say so loudly and instead delete the row of a number the Task 1 per-guest outputs
show a repo-owned guest issuing, and run that guest's e2e — pick from the `.nums` files, do not
guess which guest issues what.) Paste both; restore the row.

- [ ] **Step 7: Commit**

```sh
cargo clippy -p retrace-arch -p retrace-core --all-targets -- -D warnings 2>&1 | tail -3
git add crates/retrace-arch crates/retrace-core
git commit -m "M33 t5: a row for every census number; ioctl and sysctl newp decided by measurement"
```

---

### Task 6: The sweep re-baseline, the gate, the documents, the merge

**Files:**
- Modify: `README.md` ("What works today", "Known limits", the gate line)
- Modify: `docs/status-log.md` (append `## Status: M33-readerenum — …`)
- Modify: `docs/superpowers/specs/2026-09-12-retrace-m33-readerenum-design.md` (append an
  outcome section only if a Ruling re-scoped anything; otherwise untouched)
- Memory: `~/.claude/projects/-Users-noahmitchem-Documents-GitHub-retrace/memory/` — update
  `retrace-argkinds-successor.md` to RESOLVED, add `retrace-m33-readerenum.md`, index line.

- [ ] **Step 1: Re-run the Apple sweep against a fresh build, foreground, chunked**

```sh
cargo build -p retrace 2>&1 | tail -1
tools/apple-sweep.sh 2>&1 | tee <scratchpad>/sweep-after.log | grep -a 'PASS\|FAIL\|SKIP\|TALLY'
```
If it exceeds the tool ceiling, split `tools/apple-sweep-binaries.txt` into two temporary lists
and run the script's loop body on each (the script takes only a binary path argument; copy it to
the scratchpad and point `LIST` at each half), then sum the tallies by hand and say so.
Expected: `TALLY pass=46 fail=8 skip=0` with the same PASS and FAIL sets as the README lists.
Apply spec §9 to any movement: a `recorder panicked` with an `M33:` line → row + census + one
re-run; a `replay diverged` → PASS → a `Ruling:` naming the syscall now translated; anything else
→ `Ruling:` and an M36 row. Record the before/after sets in the ledger.

- [ ] **Step 2: The full chunked gate**

Every chunk `--no-fail-fast`, exit code captured before any pipe, logs read with `grep -a`:
```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 > <scratchpad>/gate-1.log 2>&1; echo "exit=$?"
cargo test -p retrace-box --no-fail-fast -- --test-threads=1 > <scratchpad>/gate-2.log 2>&1; echo "exit=$?"
for t in $(ls crates/retrace/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//'); do
  cargo test -p retrace --test $t --no-fail-fast -- --test-threads=1 > <scratchpad>/gate-3-$t.log 2>&1; echo "$t exit=$?"
done
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 > <scratchpad>/gate-4.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets -- -D warnings > <scratchpad>/gate-clippy.log 2>&1; echo "exit=$?"
grep -ah 'test result' <scratchpad>/gate-*.log | awk '{p+=$4; f+=$6; i+=$8} END {print "passed="p" failed="f" ignored="i}'
grep -ah 'Running\|Doc-tests' <scratchpad>/gate-*.log | wc -l
```
Chunk 3 will exceed 10 minutes in total — run it in halves. **The `--bins` chunk is not optional**
(11 tests in `crates/retrace/src/debug.rs` run nowhere else). Chunk 1 is a whole-package run for
`retrace-arch`, so its `Doc-tests` target is included; if any library crate is split per-target,
run `cargo test -p <crate> --doc` beside it.

Expected: `failed=0`, `ignored=2`, binaries = **124** (121 + `legacy_equivalence` + `census` +
`unenum_e2e`), passed = 556 + the new tests (Task 2: 2, Task 3: 3, Task 4: 1, Task 1/5: 2 → **564**
if no row-count test was added beyond these). Reconcile file-by-file: `grep -c '#\[test\]'` per file
against `git show e13eb17:<file>` for every file touched, and account for every delta. A red that
survives one diagnose-edit-rerun cycle is a halt (charter §5).

- [ ] **Step 3: README**

- "What works today": a paragraph after the M32 one — the five tables are views over `arg_kinds`,
  the equivalence sweep, the 16 surfaced fds (named), the loud forward, the census (count,
  corpora), the `ioctl`/`sysctl` findings, the new gate count/binaries.
- "Known limits": rewrite the `fd_operands` complaint (now structurally closed — say how) and the
  `ioctl` "named hole" paragraph per the §4b finding; update the sweep tally paragraph if it moved;
  strike the "shape of the owed work changed" sentence — that work is done — and say what is still
  owed: the per-argument fill (with the §4c status), the three M34 destinations, nested-pointer
  translation, and that `Scalar`/`Ptr` are reviewer-verified only.
- The gate line: the Task 6 numbers, "measured at M33".

- [ ] **Step 4: `docs/status-log.md`** — append, never edit above:

`## Status: M33-readerenum — one table, five views, and a syscall that cannot be forwarded unclassified`
with subsections: *What was measured* (census counts per corpus; ioctl requests; sysctl newp;
the sweep before/after), *What was found* (the 16 fds, with the M10-class framing; any census
number a gate caught that the census missed; any Ruling), *What landed* (types, views, loud site,
the three positive controls with their red outputs quoted), *What this milestone does not do*
(spec §7, verbatim), *The gate* (numbers + reconciliation), *What stays owed* (per-argument fill
and M32's Control 1; M34's three; nested translation; `Scalar`/`Ptr` unverified; the corpus bias
carried from M32 unchanged; the `/bin/ps` and launcher rows if any needed a measured prototype).
Every Ruling from Tasks 1, 5 and 6 appears here verbatim.

- [ ] **Step 5: Memory** — update `retrace-argkinds-successor.md` (frontmatter description → "RESOLVED
by M33"; body: keep the measurement, add "landed as `arg_kinds` at <merge sha>"), write
`retrace-m33-readerenum.md` (type: project; the one-line lesson this milestone taught, the gate
numbers, the merge sha), and add one index line to `MEMORY.md`.

- [ ] **Step 6: Commit the close, merge locally, do not push**

```sh
git add README.md docs/status-log.md docs/superpowers/specs/2026-09-12-retrace-m33-readerenum-design.md
git commit -m "M33-readerenum: close the milestone — status-log section and README"
git checkout main && git merge --no-ff m33-readerenum -m "Merge M33-readerenum: one table, five views, a loud forward"
git log --oneline -1
```
Never `git push` (charter §5).

---

## Self-Review

**Spec coverage.** §1 wall → Tasks 3, 4. §2.1 table → Task 3. §2.2 sweep both directions → Task 2
(+ Task 3 fills, Task 5 may extend). §2.3 loud forward → Task 4. §2.4 enumeration → Tasks 1, 5.
§2.5 sweep/gate/docs → Task 6. §3a kinds and load-bearing note → Task 3 docs. §3b rules → Global
Constraints + Task 5 Step 2. §3c silent views / loud site → Task 3 views + Task 4. §4a census →
Task 1. §4b ioctl → Task 1 Step 4 + Task 5 Step 3. §4c sysctl → same. §5a–g → Tasks 3, 2, 1, 4,
(5e: nothing), 4, 6. §6 controls 1/2/3 → Task 3 Step 6, Task 4 Step 4, Task 5 Step 6. §7 → Global
Constraints + Task 6 docs. §8 → no task changes retrace-core dispatch; Task 5 Step 3's optional
assert is record-arm-only and refuses rather than forwards, which is rule 2's shape. §9 → Task 6
Step 1. §10 → Task 6 Step 2. No gaps found.

**Placeholders.** Task 5's "for every number in the Task 1 census that is not above" is a
procedure, not a placeholder — the census is not known until Task 1 runs, and the rules, sources
and format are all given. Task 1's `CENSUS` body says "paste census.txt here" — that is the data
the task produces, not a deferral.

**Type consistency.** `arg_kinds(u64) -> Option<&'static Shape>`, `forwarded_shape(u64) ->
&'static Shape`, `Shape::fd_operands(&self) -> impl Iterator<Item = usize> + '_`,
`fd_operands(u64) -> impl Iterator<Item = usize>`; `View` and `EXPECTED_DIFFS: &[(u64, View, &str)]`
in Task 2, used by the same names in Tasks 3 and 5; `CENSUS: &[i64]` in Task 1, indexed `as u64`
in Task 5; `UNENUM` in Task 4. The `#[should_panic(expected = "has no arg_kinds row")]` string is a
substring of the `forwarded_shape` message; `unenum_e2e` matches `"M33: syscall 8 (8) has no
arg_kinds row"`, also a substring (`{num} ({})` with `num = 8` → `8 (8)`). Consistent.
