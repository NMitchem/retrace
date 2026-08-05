# M10-fdtable Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the box a real guest file-descriptor table, so a guest descriptor is a guest descriptor
— never one of retrace's own — and pin rung 3 (`jq` over a file), which already passes.

**Architecture:** A *split* table. The guest-visible half (`FdSlot = Free|Open|Closed`, allocated
lowest-free from 3, 0/1/2 pre-seeded as console) exists identically on record and replay and decides
`EBADF`. The host half (`guest_fd → host_fd`) is record-only, because replay executes no syscall and
opens no host fd. One translation function is consulted by the **two** places that consume a guest fd:
`forward_and_diff` and `guest_mmap_file`. The guest-visible half rides in `BoxState` so seeks don't
resurrect closed fds.

**Tech Stack:** Rust 1.95.0 (pinned), `aarch64-apple-darwin`, Hypervisor.framework, `just` for the gate.

## Global Constraints

Copied from `CLAUDE.md` and the spec. Every task's requirements implicitly include this section.

- **macOS 26.x on Apple Silicon required.** Non-root; SIP may stay enabled.
- **`--test-threads=1` is mandatory.** HVF allows one VM per process. A bare `cargo test` flakes with
  `HV_BUSY`. `just gate` sets it.
- **`just gate` is THE exit gate:** `cargo test --workspace` + `cargo clippy -D warnings`. It must end
  green with **zero `#[ignore]`** — current baseline **185 passed / 0 failed / 0 ignored**.
- **Codesigning:** any test that spawns `CARGO_BIN_EXE_retrace` itself must sign it first — use
  `util::bin()` (`crates/retrace/tests/util/mod.rs:12`). Never hand-roll this.
- **W^X:** executing a writable guest page hangs the vCPU. Code pages are RO+exec, data RW+non-exec.
- **SPTM / anon-only memory:** a file-backed `hv_vm_map` hard-panics macOS 26. File bytes are `pread`
  into anon pages.
- **Drop order:** `Box_`'s `vcpu` field must stay declared before `vm`. Do not reorder struct fields.
- **Never reimplement Apple's PAC.**
- **`clippy.toml` bans `Instant::now`/`SystemTime::now`/`std::thread`.** Load-bearing, not style.
- **Symmetry rule 1:** a special case in record's `match stop` needs a mirror in replay's dispatch, and
  both must recompute identical bytes. **Rule 2:** deterministic emulation belongs below the trace in
  `Box_::run()`.
- **Honest-gate discipline:** a new wall gets a NEW parked gate, never a regression of an existing one.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/retrace-arch/src/lib.rs` (modify) | New syscall constants; `fd_operands()` and `allocates_fd()` — the pure `(syscall → fd operand)` mapping and its tests. Zero-dependency crate; no state. |
| `crates/retrace-box/src/lib.rs` (modify) | `FdSlot`/`FdTable` types; translation in `forward_and_diff` and `guest_mmap_file`; `BoxState` carriage + `from_checkpoint` restore. |
| `crates/retrace-core/src/lib.rs` (modify) | Allocation-on-return in record dispatch; replay-side recompute + compare. |
| `crates/retrace-guest/c/fdtable_dyn.c` (create) | Guest that observes fd 3 (not 17), `EBADF` after close, and a `dup` alias. |
| `crates/retrace-guest/build.rs`, `src/lib.rs` (modify) | Compile + export `FDTABLE_DYN`. |
| `crates/retrace/tests/fdtable_e2e.rs` (create) | The semantics gate. |
| `crates/retrace/tests/jq_file_e2e.rs` (create) | The rung-3 gate. |
| `crates/retrace/tests/fixtures/rung3.json` (create) | Repo-owned JSON input. |
| `README.md`, `CLAUDE.md` (modify) | Status section and the honest close. |

**Task order is dependency order.** Tasks 1–2 are pure and independent of the VM. Task 3 is the single
atomic integration — it cannot be split without a red gate, because a table that allocates nothing
makes every guest fd `Free` and therefore `EBADF`.

---

### Task 1: The `(syscall → fd operand)` mapping in `retrace-arch`

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (constants near line 41; functions near `is_console_close`
  at line 37; tests in the existing `#[cfg(test)]` module around line 120)

**Interfaces:**
- Consumes: nothing (zero-dependency crate).
- Produces:
  - `pub fn fd_operands(num: u64) -> &'static [usize]` — operand indices holding a guest fd; empty
    slice if none.
  - `pub fn allocates_fd(num: u64) -> bool` — true if the syscall's **return value** is a new fd.
  - Constants: `SYS_SOCKET`=97, `SYS_CONNECT`=98, `SYS_SENDTO`=133, `SYS_IOCTL`=54, `SYS_DUP`=41,
    `SYS_DUP2`=90, `SYS_FCNTL`=92, `SYS_FSTAT64`=339, `SYS_READ_NOCANCEL`=396,
    `SYS_OPEN_NOCANCEL`=398, `SYS_OPENAT`=463, `SYS_AT_FDCWD`=-2 (as `i64`).

**Background the implementer needs:** the surface below was measured from a real `jq` run, but the
spec's risk register records that a first pass over a **truncated** histogram missed six rows. Do not
trust this table blindly — Step 0 re-derives it.

- [ ] **Step 0: Re-derive the fd surface from a full histogram (spec R1 mitigation)**

```bash
cd /Users/noahmitchem/Documents/GitHub/retrace
printf '{"name":"retrace","rung":3}\n' > /tmp/rung3.json
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn /opt/homebrew/bin/jq \
  -o /tmp/m10.bin -- '.name' /tmp/rung3.json > /tmp/m10.trace 2>&1
# FULL histogram — no `head`, no truncation. The tail is where count-1 syscalls live.
grep -ao 'num=[0-9-]*' /tmp/m10.trace | sort | uniq -c | sort -rn
```

For every number that appears, resolve its name and check whether it takes an fd:

```bash
grep -E "[[:space:]]+<NUM>$" "$(xcrun --show-sdk-path)/usr/include/sys/syscall.h"
```

If you find an fd-taking syscall not in the table below, **add it** and note it in the commit message.
Pay particular attention to `_nocancel` variants: `jq` calls `read`(3) zero times and
`read_nocancel`(396) twice — tabling only the plain form forwards a raw guest fd and nothing looks
wrong.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `crates/retrace-arch/src/lib.rs`:

```rust
#[test]
fn fd_operands_covers_the_measured_surface() {
    // x0 holders.
    for num in [SYS_CLOSE, SYS_CLOSE_NOCANCEL, SYS_READ, SYS_READ_NOCANCEL, SYS_PREAD,
                SYS_WRITE, SYS_WRITE_NOCANCEL, SYS_FCNTL, SYS_FSTAT, SYS_FSTAT64,
                SYS_LSEEK, SYS_IOCTL, SYS_DUP, SYS_CONNECT, SYS_SENDTO] {
        assert_eq!(fd_operands(num), &[0], "syscall {num} holds its fd in x0");
    }
    // mmap is the exception that makes a single choke point insufficient: x4, consumed by
    // guest_mmap_file, which never reaches forward_and_diff.
    assert_eq!(fd_operands(SYS_MMAP), &[4]);
    assert_eq!(fd_operands(SYS_DUP2), &[0, 1]);
    // openat's x0 is a dirfd (AT_FDCWD passes through untranslated — see translate_fd).
    assert_eq!(fd_operands(SYS_OPENAT), &[0]);
    // Path-only and fd-free syscalls must NOT be translated.
    for num in [SYS_OPEN, SYS_OPEN_NOCANCEL, SYS_SOCKET, SYS_EXIT, SYS_MUNMAP, SYS_SYSCTL] {
        assert_eq!(fd_operands(num), &[] as &[usize], "syscall {num} has no fd operand");
    }
}

#[test]
fn allocates_fd_covers_every_fd_producing_call() {
    for num in [SYS_OPEN, SYS_OPEN_NOCANCEL, SYS_OPENAT, SYS_DUP, SYS_DUP2, SYS_SOCKET] {
        assert!(allocates_fd(num), "syscall {num} returns a NEW fd");
    }
    for num in [SYS_CLOSE, SYS_READ, SYS_PREAD, SYS_MMAP, SYS_FCNTL, SYS_EXIT] {
        assert!(!allocates_fd(num), "syscall {num} does not return a new fd");
    }
}

// The M9 defect, generalized: every _nocancel variant must be tabled beside its plain form.
// jq reaches the kernel through 396/397/398/399 and never through 3/4/5/6.
#[test]
fn nocancel_variants_are_tabled_beside_their_plain_forms() {
    assert_eq!(fd_operands(SYS_READ), fd_operands(SYS_READ_NOCANCEL));
    assert_eq!(fd_operands(SYS_WRITE), fd_operands(SYS_WRITE_NOCANCEL));
    assert_eq!(fd_operands(SYS_CLOSE), fd_operands(SYS_CLOSE_NOCANCEL));
    assert_eq!(allocates_fd(SYS_OPEN), allocates_fd(SYS_OPEN_NOCANCEL));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-arch fd_operands -- --test-threads=1`
Expected: FAIL — `cannot find function 'fd_operands' in this scope`.

- [ ] **Step 3: Write the implementation**

Add the constants beside the existing ones (after line 51), then:

```rust
/// `AT_FDCWD` — `openat`'s "relative to cwd" sentinel. Negative, and NOT a descriptor: it must pass
/// through translation untouched.
pub const AT_FDCWD: i64 = -2;

/// Which operand indices of `num` hold a **guest** file descriptor.
///
/// This is the M10 analogue of `is_console_write`: one shared table rather than a condition spelled
/// out at each call site, because a forgotten entry does not diverge loudly — it forwards a raw guest
/// fd to the host kernel, which acts on RETRACE's descriptor of that number. A syscall absent here is
/// simply not translated, so absence must mean "provably takes no fd", never "not gotten to yet".
///
/// `_nocancel` variants are listed beside their plain forms deliberately. macOS libc routinely takes
/// ONLY the `_nocancel` path — measured: `jq` calls `read`(3) zero times and `read_nocancel`(396)
/// twice — so a plain-only table fails silently. That is exactly how M9's console bug survived.
pub fn fd_operands(num: u64) -> &'static [usize] {
    match num {
        SYS_CLOSE | SYS_CLOSE_NOCANCEL | SYS_READ | SYS_READ_NOCANCEL | SYS_PREAD
        | SYS_WRITE | SYS_WRITE_NOCANCEL | SYS_FCNTL | SYS_FSTAT | SYS_FSTAT64
        | SYS_LSEEK | SYS_IOCTL | SYS_DUP | SYS_CONNECT | SYS_SENDTO | SYS_OPENAT => &[0],
        SYS_DUP2 => &[0, 1],
        SYS_MMAP => &[4],
        _ => &[],
    }
}

/// Does `num`'s RETURN value need binding to a fresh guest fd slot?
///
/// `socket` is here for the same reason `open` is: guest fds are not files-only. The measured `jq`
/// run creates a socket and `connect`/`sendto`s on it.
pub fn allocates_fd(num: u64) -> bool {
    matches!(num, SYS_OPEN | SYS_OPEN_NOCANCEL | SYS_OPENAT | SYS_DUP | SYS_DUP2 | SYS_SOCKET)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: PASS, all tests in the crate.

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-arch/src/lib.rs
git commit -m "M10 t1: the (syscall -> fd operand) mapping, _nocancel variants tabled pair-wise"
```

---

### Task 2: `FdTable` — the guest-visible half

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (types near `BoxState` at line 401)
- Test: `crates/retrace-box/tests/fdtable.rs` (create)

**Interfaces:**
- Consumes: nothing from Task 1 yet (pure data structure).
- Produces:
  - `pub enum FdSlot { Free, Open, Closed }` — derives `Debug, Clone, Copy, PartialEq, Eq`.
  - `pub struct FdTable`
  - `pub fn FdTable::new() -> FdTable` — slots 0/1/2 pre-seeded `Open`.
  - `pub fn FdTable::alloc(&mut self) -> u64` — lowest `Free` index ≥ 3, marks it `Open`.
  - `pub fn FdTable::bind(&mut self, gfd: u64, host_fd: i32)`
  - `pub fn FdTable::host(&self, gfd: u64) -> Option<i32>`
  - `pub fn FdTable::is_open(&self, gfd: u64) -> bool`
  - `pub fn FdTable::close(&mut self, gfd: u64) -> bool` — `false` if it wasn't `Open`.
  - `pub fn FdTable::slots(&self) -> Vec<FdSlot>` / `pub fn FdTable::from_slots(&[FdSlot]) -> FdTable`
  - `pub const EBADF: u64 = 9;`

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-box/tests/fdtable.rs`:

```rust
// M10 t2. The guest-visible half of the fd table: allocation, close, dup aliasing, console seeding.
// Pure data structure — no VM, so this test needs no entitlement and no serialization.
use retrace_box::{FdSlot, FdTable};

#[test]
fn console_fds_are_preseeded_open() {
    let t = FdTable::new();
    for gfd in 0..=2 { assert!(t.is_open(gfd), "fd {gfd} is the console and starts open"); }
    assert!(!t.is_open(3), "fd 3 starts free");
}

#[test]
fn alloc_returns_lowest_free_starting_at_three() {
    let mut t = FdTable::new();
    // THE determinism property: a guest's first open is 3, not whatever the host had free.
    assert_eq!(t.alloc(), 3);
    assert_eq!(t.alloc(), 4);
    assert_eq!(t.alloc(), 5);
}

#[test]
fn close_frees_the_slot_for_reuse_and_reports_bad_closes() {
    let mut t = FdTable::new();
    let a = t.alloc();          // 3
    let b = t.alloc();          // 4
    assert!(t.close(a));
    assert!(!t.is_open(a));
    assert!(!t.close(a), "closing an already-closed fd reports failure (EBADF)");
    assert!(!t.close(99), "closing a never-opened fd reports failure (EBADF)");
    assert_eq!(t.alloc(), a, "the freed slot is the lowest free and is reused");
    assert!(t.is_open(b), "closing one fd must not disturb another");
}

#[test]
fn bind_and_host_map_the_guest_fd_to_a_host_fd() {
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);              // the host handed back 17; the guest must never see it
    assert_eq!(t.host(g), Some(17));
    assert_eq!(t.host(99), None, "an unallocated guest fd has no host mapping");
    t.close(g);
    assert_eq!(t.host(g), None, "closing clears the host mapping");
}

#[test]
fn dup_aliases_two_guest_fds_onto_one_host_fd() {
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    let d = t.alloc();
    t.bind(d, 17);              // dup: a second guest fd over the same host fd
    assert_ne!(g, d);
    assert_eq!(t.host(g), t.host(d));
}

#[test]
fn slots_round_trip_through_from_slots() {
    let mut t = FdTable::new();
    let a = t.alloc();
    let b = t.alloc();
    t.close(a);
    let restored = FdTable::from_slots(&t.slots());
    assert!(!restored.is_open(a), "a closed fd stays closed across a round trip");
    assert!(restored.is_open(b));
    for gfd in 0..=2 { assert!(restored.is_open(gfd)); }
    assert_eq!(restored.slots(), t.slots());
}

#[test]
fn from_slots_carries_no_host_mapping() {
    // The host half is record-only by construction: replay opens no host fd, so a restored table
    // must not claim one. This is what keeps host fd numbers out of the trace.
    let mut t = FdTable::new();
    let g = t.alloc();
    t.bind(g, 17);
    let restored = FdTable::from_slots(&t.slots());
    assert!(restored.is_open(g), "guest-visible state survives");
    assert_eq!(restored.host(g), None, "host mapping does NOT survive");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-box --test fdtable -- --test-threads=1`
Expected: FAIL — `unresolved import retrace_box::FdTable`.

- [ ] **Step 3: Write the implementation**

Add to `crates/retrace-box/src/lib.rs` above `BoxState`:

```rust
/// `EBADF` — returned for a guest fd that is `Free` or `Closed`, without forwarding anything.
pub const EBADF: u64 = 9;

/// One entry in the guest's descriptor space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdSlot { Free, Open, Closed }

/// The guest's file-descriptor table.
///
/// **Split by design.** `slots` is guest-visible state: a pure function of the guest's own
/// open/dup/close sequence, identical on record and replay, and the sole authority on `EBADF`.
/// `host` is the record-only `guest_fd -> host_fd` map — replay executes no syscall and opens no host
/// fd, so it has nothing to map, and host fd numbers (the one nondeterministic quantity here, since
/// they depend on how many files RETRACE happens to hold open) therefore never enter the trace.
///
/// Before M10 the guest's fds WERE retrace's: `forward_and_diff` issues a raw `svc` in retrace's own
/// process, so a guest `open` returned a host fd (measured: `jq` saw 17-22, because retrace holds
/// 0-16 open) and a guest `close(n)` closed retrace's `n`.
#[derive(Debug, Clone, Default)]
pub struct FdTable {
    slots: Vec<FdSlot>,
    host: Vec<Option<i32>>,
}

impl FdTable {
    /// Fresh table with 0/1/2 open as the console. They have no host mapping: M9 mirrors console
    /// writes into the trace and fakes the close rather than forwarding either.
    pub fn new() -> FdTable {
        FdTable { slots: vec![FdSlot::Open; 3], host: vec![None; 3] }
    }

    fn grow_to(&mut self, gfd: usize) {
        if self.slots.len() <= gfd {
            self.slots.resize(gfd + 1, FdSlot::Free);
            self.host.resize(gfd + 1, None);
        }
    }

    /// Lowest free descriptor >= 3, POSIX-style. Deterministic: this is what makes a recorded guest
    /// fd a function of the guest rather than of retrace's own open files.
    pub fn alloc(&mut self) -> u64 {
        let gfd = (3..self.slots.len()).find(|&i| self.slots[i] == FdSlot::Free)
            .unwrap_or(self.slots.len().max(3));
        self.grow_to(gfd);
        self.slots[gfd] = FdSlot::Open;
        gfd as u64
    }

    pub fn bind(&mut self, gfd: u64, host_fd: i32) {
        self.grow_to(gfd as usize);
        self.host[gfd as usize] = Some(host_fd);
    }

    pub fn host(&self, gfd: u64) -> Option<i32> {
        self.host.get(gfd as usize).copied().flatten()
    }

    pub fn is_open(&self, gfd: u64) -> bool {
        self.slots.get(gfd as usize) == Some(&FdSlot::Open)
    }

    /// Mark closed and drop the host mapping. `false` means the guest closed something it did not
    /// have open — the caller answers `EBADF` and forwards nothing.
    pub fn close(&mut self, gfd: u64) -> bool {
        if !self.is_open(gfd) { return false; }
        self.slots[gfd as usize] = FdSlot::Closed;
        self.host[gfd as usize] = None;
        true
    }

    pub fn slots(&self) -> Vec<FdSlot> { self.slots.clone() }

    /// Rebuild guest-visible state only — used by `from_checkpoint` and by replay. Deliberately
    /// carries NO host mapping.
    pub fn from_slots(slots: &[FdSlot]) -> FdTable {
        FdTable { slots: slots.to_vec(), host: vec![None; slots.len()] }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p retrace-box --test fdtable -- --test-threads=1`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/fdtable.rs
git commit -m "M10 t2: FdTable — guest-visible slots, record-only host map"
```

---

### Task 3: Wire the table into the box (the atomic integration)

**This task cannot be split.** A table that allocates nothing makes every guest fd `Free`, hence
`EBADF`, hence a dead dyld. Allocation, translation at both call sites, and the `EBADF` path must land
together or the gate is red.

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — `Box_` field, `forward_and_diff` (line 2054),
  `guest_mmap_file`
- Modify: `crates/retrace-core/src/lib.rs` — record dispatch (`close` arm near line 154)

**Interfaces:**
- Consumes: `retrace_arch::{fd_operands, allocates_fd, AT_FDCWD}` (Task 1); `FdTable`, `FdSlot`,
  `EBADF` (Task 2).
- Produces:
  - `Box_::fds: FdTable` — a public field or `pub fn fds_mut(&mut self) -> &mut FdTable`; pick one and
    use it consistently.
  - `pub fn Box_::translate_fds(&self, num: u64, args: &mut [u64; 8]) -> Result<(), u64>` — rewrites
    every guest fd operand to its host fd in place; `Err(EBADF)` if any operand is not open.
  - `pub fn Box_::bind_returned_fd(&mut self, num: u64, host_ret: u64) -> u64` — for an
    `allocates_fd` syscall, allocates a guest slot, binds it to `host_ret`, returns the **guest** fd.

**Critical:** `forward_and_diff` currently takes `args: [u64;8]` by value and translates pointers.
Translate fds **before** its pointer-window loop, so the fd operand is a host fd by the time
`host_svc` runs. `AT_FDCWD` (-2) and any other negative sentinel must pass through untouched — check
`(v as i64) < 0` before treating an operand as a descriptor.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-box/tests/fdxlat.rs`:

```rust
// M10 t3. Translation and EBADF, exercised through the box's public surface without running a guest.
use retrace_box::{Box_, EBADF};
use retrace_arch::{SYS_CLOSE, SYS_MMAP, SYS_OPENAT, SYS_PREAD, AT_FDCWD};

#[test]
fn translate_rewrites_the_guest_fd_to_its_host_fd() {
    let mut b = Box_::for_fd_tests();
    let g = b.fds_mut().alloc();
    b.fds_mut().bind(g, 17);
    let mut args = [0u64; 8];
    args[0] = g;
    assert!(b.translate_fds(SYS_PREAD, &mut args).is_ok());
    assert_eq!(args[0], 17, "the host kernel must see the HOST fd, never the guest's");
}

#[test]
fn translate_rejects_a_closed_or_never_opened_fd_with_ebadf() {
    let mut b = Box_::for_fd_tests();
    let g = b.fds_mut().alloc();
    b.fds_mut().bind(g, 17);
    b.fds_mut().close(g);
    let mut args = [0u64; 8];
    args[0] = g;
    assert_eq!(b.translate_fds(SYS_CLOSE, &mut args), Err(EBADF));
    args[0] = 42;   // never opened
    assert_eq!(b.translate_fds(SYS_CLOSE, &mut args), Err(EBADF));
}

#[test]
fn translate_uses_x4_for_mmap() {
    let mut b = Box_::for_fd_tests();
    let g = b.fds_mut().alloc();
    b.fds_mut().bind(g, 17);
    let mut args = [0u64; 8];
    args[4] = g;
    assert!(b.translate_fds(SYS_MMAP, &mut args).is_ok());
    assert_eq!(args[4], 17);
    assert_eq!(args[0], 0, "mmap's x0 is an address hint, not an fd — it must be untouched");
}

#[test]
fn at_fdcwd_passes_through_untranslated() {
    let mut b = Box_::for_fd_tests();
    let mut args = [0u64; 8];
    args[0] = AT_FDCWD as u64;
    assert!(b.translate_fds(SYS_OPENAT, &mut args).is_ok(),
        "AT_FDCWD is a sentinel, not a descriptor — it must not be rejected as EBADF");
    assert_eq!(args[0], AT_FDCWD as u64);
}

#[test]
fn bind_returned_fd_hands_the_guest_a_guest_number() {
    let mut b = Box_::for_fd_tests();
    // The host returned 17. The guest must see 3.
    let gfd = b.bind_returned_fd(retrace_arch::SYS_OPEN, 17);
    assert_eq!(gfd, 3, "the guest's first open is 3, not the host's 17");
    assert_eq!(b.fds_mut().host(gfd), Some(17));
}
```

`Box_::for_fd_tests()` is a minimal constructor for table-only tests — add it beside `FdTable`:

```rust
/// A `Box_` with only its fd table live, for testing translation without booting a guest.
/// Not used in production paths.
#[cfg(any(test, feature = "test-util"))]
pub fn for_fd_tests() -> Box_ { /* construct with Default-ish fields; see note below */ }
```

**Implementer note:** `Box_` owns a `Vm`/`Vcpu` and cannot be trivially default-constructed, and
creating a VM here would violate the one-VM-per-process rule under `--test-threads=1` in a crate whose
other tests already create one. **Prefer making `translate_fds`/`bind_returned_fd` free functions over
`&mut FdTable`** and have `Box_` delegate to them:

```rust
pub fn translate_fds(fds: &FdTable, num: u64, args: &mut [u64; 8]) -> Result<(), u64>
pub fn bind_returned_fd(fds: &mut FdTable, num: u64, host_ret: u64) -> u64
```

Then the tests above construct a bare `FdTable` instead of a `Box_`, and `Box_` keeps thin wrappers.
Rewrite the test bodies accordingly — this is the preferred shape; the `for_fd_tests` variant is
listed only so you recognize the trap.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-box --test fdxlat -- --test-threads=1`
Expected: FAIL to compile — `translate_fds` not found.

- [ ] **Step 3: Implement translation**

```rust
/// Rewrite every guest fd operand of `num` in `args` to its host fd, in place.
///
/// Called from the TWO places that consume a guest fd — `forward_and_diff` and `guest_mmap_file`.
/// It is deliberately not "inside forward_and_diff": file-backed mmap is special-cased upstream in
/// retrace-core and preads from its fd (x4) without ever reaching forward_and_diff, so a
/// single-choke-point design would leak exactly that one.
pub fn translate_fds(fds: &FdTable, num: u64, args: &mut [u64; 8]) -> Result<(), u64> {
    for &i in retrace_arch::fd_operands(num) {
        let v = args[i];
        // AT_FDCWD and friends are sentinels, not descriptors.
        if (v as i64) < 0 { continue; }
        match fds.host(v) {
            Some(h) => args[i] = h as u64,
            // Console fds (0/1/2) have no host mapping and never reach here: M9 mirrors their
            // writes and fakes their close in retrace-core before forwarding is considered.
            None => return Err(EBADF),
        }
    }
    Ok(())
}

/// Bind an `allocates_fd` syscall's host return value to a fresh guest slot, returning the GUEST fd.
pub fn bind_returned_fd(fds: &mut FdTable, num: u64, host_ret: u64) -> u64 {
    debug_assert!(retrace_arch::allocates_fd(num));
    let gfd = fds.alloc();
    fds.bind(gfd, host_ret as i32);
    gfd
}
```

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p retrace-box --test fdxlat -- --test-threads=1`
Expected: PASS (5 tests).

- [ ] **Step 5: Wire into `forward_and_diff`**

Add `fds: FdTable` to `Box_` (any position — the `vcpu`-before-`vm` drop-order rule concerns only
those two fields), initialize with `FdTable::new()` in every constructor. Then at the top of
`forward_and_diff` (`crates/retrace-box/src/lib.rs:2054`), **before** the pointer-window loop:

```rust
pub fn forward_and_diff(&self, num: u64, args: [u64;8]) -> (u64, bool, Vec<Region>) {
    let mut args = args;
    if let Err(e) = translate_fds(&self.fds, num, &mut args) {
        // EBADF: the guest named an fd it does not have open. Forward NOTHING — the whole point is
        // that this number may be a live descriptor of retrace's own.
        return (e, true, Vec::new());
    }
    // ... existing body, unchanged, now operating on host fds ...
```

Do the same in `guest_mmap_file` for its `fd` parameter (x4), before it `pread`s.

- [ ] **Step 6: Wire allocation-on-return in `retrace-core`**

In `record_box`'s dispatch, after a forwarded syscall returns successfully, convert the host fd to a
guest fd **before** the `Event` is appended, so the trace records the guest number:

```rust
// M10: an fd-producing syscall returns a HOST fd. Bind it to a guest slot and record the GUEST
// number, so the trace is a function of the guest and not of how many files retrace holds open.
let ret = if !err && retrace_arch::allocates_fd(num) {
    b.bind_returned_fd(num, ret)
} else { ret };
```

Place this immediately before the existing `w.append(&Event::Syscall { .. ret .. })` on the generic
forward path. Leave the M9 console-write and console-close arms untouched — they precede this path and
never forward.

- [ ] **Step 7: Run the full gate**

Run: `just gate`
Expected: **185 passed / 0 failed / 0 ignored**, clippy clean. dyld opens the shared cache and every
dylib, so this exercises translation heavily; a mistake fails immediately and loudly.

If `hello_dyn_e2e` or `jq_e2e` now fails, the most likely cause is a missing `fd_operands` row —
re-run Task 1 Step 0's histogram against the *failing* guest.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/fdxlat.rs crates/retrace-core/src/lib.rs
git commit -m "M10 t3: translate guest fds at both consumers; EBADF instead of retrace's descriptors"
```

---

### Task 4: Carry the table through checkpoints

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — `BoxState` (line 401), `checkpoint()` (line 2335),
  `from_checkpoint()` (line 2372)
- Test: `crates/retrace-box/tests/fdtable.rs` (extend)

**Interfaces:**
- Consumes: `FdTable::slots()` / `FdTable::from_slots()` (Task 2); `BoxState` (existing).
- Produces: `BoxState.fd_slots: Vec<FdSlot>`.

**Why this task exists:** this repo has paid for this exact bug three times — `pac_enabled` ("must be
carried instead", M7 t6), `stack_top`/`stack_size` (M8-stack), and `tlbi_stub_ready` (M9 t3, where
`from_checkpoint` reset a flag the restored backings contradicted). A mid-run capture cannot re-derive
the fd table; if it defaults to empty, every seeked session believes all fds are `Free`, so a post-seek
guest `pread` returns `EBADF` and reverse execution diverges from the forward run.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/fdtable.rs`:

```rust
// The M9 t3 regression shape: state a mid-run capture cannot re-derive must be CARRIED.
#[test]
fn fd_table_survives_checkpoint_restore() {
    use retrace_box::{BoxState, FdSlot};
    // Build the guest-visible state a mid-run box would hold: 3 open, 4 opened-then-closed.
    let mut t = FdTable::new();
    let a = t.alloc();          // 3, stays open
    let b = t.alloc();          // 4, gets closed
    t.close(b);

    // A BoxState carrying it must round-trip both the open AND the closed slot.
    let slots = t.slots();
    let restored = FdTable::from_slots(&slots);
    assert!(restored.is_open(a), "an open fd must survive the restore");
    assert!(!restored.is_open(b), "a CLOSED fd must stay closed — else a seek resurrects it");
    assert_eq!(restored.slots()[b as usize], FdSlot::Closed,
        "Closed must be distinguishable from Free, or the allocator reuses a number the guest holds");

    // And BoxState must actually have the field, defaulted to a console-seeded table when absent.
    let s = BoxState { fd_slots: slots.clone(), ..BoxState::empty_for_test() };
    assert_eq!(s.fd_slots, slots);
}
```

**Implementer note:** if `BoxState` has no cheap constructor, drop the last two lines and instead
assert the field exists by constructing the full struct in a `#[test]` inside
`crates/retrace-box/src/lib.rs`'s own test module, where private helpers are reachable.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-box --test fdtable fd_table_survives -- --test-threads=1`
Expected: FAIL — `BoxState` has no field `fd_slots`.

- [ ] **Step 3: Implement carriage**

Add to `BoxState` (after `stack_size`, line 428):

```rust
    // M10: carried for the same reason as `pac_enabled` and `stack_top` — a mid-run capture cannot
    // re-derive it. Guest-visible slots ONLY; the host map is record-only and a restored box has no
    // host fds (from_checkpoint is a replay-side operation). Defaulting this to empty would make a
    // seeked session believe every fd is Free, so a post-seek pread returns EBADF and reverse
    // execution silently diverges from the forward run — the M9 t3 failure shape.
    pub fd_slots: Vec<FdSlot>,
```

In `checkpoint()` (line 2343's struct literal): `fd_slots: self.fds.slots(),`

In `from_checkpoint()` (after the vcpu setup, where other captured fields are restored):
```rust
    // Derive from the captured slots, NOT reset — see the BoxState field comment.
    b.fds = FdTable::from_slots(&state.fd_slots);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p retrace-box -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Run the full gate** (checkpoints feed M4 seeks and M5 watchpoints)

Run: `just gate`
Expected: 185/0/0. Watch `seek`, `reverse_debug_e2e`, `debug_cli` in particular.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/fdtable.rs
git commit -m "M10 t4: carry the guest-visible fd table in BoxState (the M9 t3 shape)"
```

---

### Task 5: Replay-side recompute and compare

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` — replay dispatch (near the console-write mirror at
  line 583)

**Interfaces:**
- Consumes: `FdTable` (Task 2), `retrace_arch::allocates_fd` (Task 1).
- Produces: no new public API — a divergence check.

**The posture:** this is the **standard symmetric** posture of rule 1, not M2-xpcport's deliberate
verbatim-apply exception. Guest fd numbers are now deterministic (lowest-free over the guest's own
sequence), so replay can compute what the allocator *would* have produced and byte-compare against the
recording. A table bug then surfaces as a loud divergence instead of silent corruption. Replay keeps
**only** the guest-visible half — it opens no host fd.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-core/tests/fdreplay.rs`:

```rust
// M10 t5. Replay recomputes the guest fd a recording claims, and diverges loudly if they disagree.
// A hand-built trace is the cheapest way to state this: it needs no VM.
use retrace_trace::{Event, Writer};

#[test]
fn replay_diverges_when_a_recorded_fd_is_not_what_the_allocator_would_produce() {
    let dir = std::env::temp_dir().join(format!("m10-fdreplay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad-fd.bin");

    // Craft a trace whose open(5) claims to have returned 17 — a HOST fd number, which is exactly
    // what a pre-M10 recording contained. Post-M10 the allocator must produce 3, so replay must
    // reject 17 rather than faithfully reproducing it.
    let mut w = Writer::create(&path).unwrap();
    // ... snapshot event as the existing core tests build one ...
    w.append(&Event::Syscall { num: retrace_arch::SYS_OPEN, args: [0; 8], ret: 17, err: false, writes: vec![] }).unwrap();
    drop(w);

    let err = retrace_core::replay(&path).expect_err("replay must reject a host-shaped fd");
    assert!(err.contains("fd"), "divergence message should name the fd mismatch, got: {err}");
}
```

**Implementer note:** build the leading `Snapshot` event the way the existing `retrace-core` tests do —
read one of them first (`crates/retrace-core/tests/`) and copy the construction verbatim. If no
snapshot-only trace helper exists, this test is better expressed as a unit test of the comparison
function alone; do that rather than inventing a trace format by hand.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p retrace-core --test fdreplay -- --test-threads=1`
Expected: FAIL — replay accepts the recorded 17.

- [ ] **Step 3: Implement the recompute**

In the replay dispatch, alongside where recorded returns are applied:

```rust
// M10: recompute the guest fd the deterministic allocator would produce and compare it against the
// recording. Standard symmetric posture (rule 1) — NOT the M2-xpcport verbatim-apply exception:
// guest fd numbers are a pure function of the guest's own open/dup/close sequence, so both runs
// must agree. Replay holds guest-visible slots only; it opens no host fd.
if !err && retrace_arch::allocates_fd(num) {
    let expect = fds.alloc();
    if expect != ret {
        return Err(format!(
            "fd divergence at event {n}: recording says {num} returned fd {ret}, but the guest's \
             own open/close sequence yields {expect}. A recorded HOST fd (typically >= 16) means \
             the trace predates M10's fd table."));
    }
}
```

Mirror `close` by calling `fds.close(args[0])` on the replay side so the two tables stay in step.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p retrace-core -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Run the full gate**

Run: `just gate`
Expected: 185/0/0.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-core/tests/fdreplay.rs
git commit -m "M10 t5: replay recomputes the guest fd and diverges on mismatch"
```

---

### Task 6: The semantics guest — fd 3, not 17

**Files:**
- Create: `crates/retrace-guest/c/fdtable_dyn.c`
- Modify: `crates/retrace-guest/build.rs` (beside `closefd_dyn`, line 205), `crates/retrace-guest/src/lib.rs` (beside `CLOSEFD_DYN`, line 122)
- Create: `crates/retrace/tests/fdtable_e2e.rs`

**Interfaces:**
- Consumes: the whole record path (Tasks 1–5); `util::assert_rung_records_and_replays`.
- Produces: `retrace_guest::FDTABLE_DYN`.

- [ ] **Step 1: Write the guest**

Create `crates/retrace-guest/c/fdtable_dyn.c`:

```c
// M10. The fd-table semantics fixture. Before M10 the guest's descriptors WERE retrace's: this
// program's first open returned 17 on the measured host, because retrace holds 0-16 open. The whole
// point of the milestone is that it now returns 3 — a number that is a function of THIS program's
// own open/close sequence and nothing else.
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>

int main(void) {
    int a = open("/dev/null", O_RDONLY);
    printf("first=%d\n", a);            // must be 3, not 17

    int d = dup(a);
    printf("dup=%d\n", d);              // must be 4

    close(a);
    // Writing to a closed fd must now fail with EBADF. Before M10 retrace did not model a closed
    // fd at all, so this succeeded against retrace's own descriptor.
    char buf[1];
    int r = read(a, buf, 1);
    printf("closed_read=%d errno=%d\n", r, errno);   // must be -1 and EBADF(9)

    // The dup'd alias is independent and still usable.
    printf("dup_open=%d\n", read(d, buf, 1));        // 0 at EOF on /dev/null
    close(d);
    fflush(stdout);
    return 0;
}
```

- [ ] **Step 2: Wire it into the build**

In `crates/retrace-guest/build.rs`, after the `closefd_dyn` block (line 212):

```rust
    // fdtable_dyn: the M10 fd-table semantics fixture. Same recipe as hello_dyn.
    let src = format!("{}/c/fdtable_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/fdtable_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang fdtable_dyn");
    assert!(status.success(), "fdtable_dyn guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, beside line 122:

```rust
pub const FDTABLE_DYN: &str = concat!(env!("OUT_DIR"), "/fdtable_dyn");
```

- [ ] **Step 3: Write the failing test**

Create `crates/retrace/tests/fdtable_e2e.rs`:

```rust
// M10 gate. The guest's descriptors are ITS OWN — the determinism property made observable.
mod util;

#[test]
fn guest_sees_its_own_fd_numbers_and_ebadf_after_close() {
    let expect = b"first=3\ndup=4\nclosed_read=-1 errno=9\ndup_open=0\n";
    let out = util::assert_rung_records_and_replays(retrace_guest::FDTABLE_DYN, &[], expect);
    // Stated separately from the helper's equality so a failure names WHICH property broke.
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("first=3"),
        "the guest's first open must be 3, not a host fd (pre-M10 this was 17): got {s}");
    assert!(s.contains("errno=9"),
        "reading a closed fd must give EBADF(9), not succeed against retrace's descriptor: got {s}");
}
```

- [ ] **Step 4: Run it to verify it fails, then passes**

Run: `cargo test -p retrace --test fdtable_e2e -- --test-threads=1`
Expected before Tasks 1–5 land: FAIL. With them: PASS.

If `first=` reports 17, translation is not wired into the record path (Task 3 Step 6). If it reports 3
but `closed_read` succeeds, the `EBADF` short-circuit in `forward_and_diff` is missing (Task 3 Step 5).

- [ ] **Step 5: Commit**

```bash
git add crates/retrace-guest/c/fdtable_dyn.c crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/fdtable_e2e.rs
git commit -m "M10 t6: fdtable_dyn — the guest sees fd 3, not retrace's 17"
```

---

### Task 7: The rung-3 gate

**Files:**
- Create: `crates/retrace/tests/fixtures/rung3.json`, `crates/retrace/tests/jq_file_e2e.rs`

**Interfaces:**
- Consumes: `util::assert_rung_records_and_replays`.

**Note:** rung 3 already passed before M10 (measured). This test pins it; it is not expected to go from
red to green. Say so in the commit message rather than implying it was newly earned.

- [ ] **Step 1: Create the fixture**

`crates/retrace/tests/fixtures/rung3.json`:

```json
{"name":"retrace","rung":3}
```

- [ ] **Step 2: Write the test**

Create `crates/retrace/tests/jq_file_e2e.rs`:

```rust
// M10 rung-3 gate. `jq` reading a real file argument — the first guest that opens a path of its own
// and reads it, so the fd table is load-bearing rather than merely motivated.
//
// This capability already worked before M10 (the forward-and-record path captures the file's bytes as
// recorded kernel writes, and replay executes no syscall). The gate exists to PIN it.
//
// NOT a repo artifact: jq comes from Homebrew. When absent, announce the skip loudly rather than
// passing quietly — a silent skip reads as a green it did not earn.
mod util;

const JQ: &str = "/opt/homebrew/bin/jq";

#[test]
fn jq_reads_a_file_argument_and_replays() {
    if !std::path::Path::new(JQ).exists() {
        eprintln!("SKIPPED jq_reads_a_file_argument_and_replays: {JQ} not installed \
                   (`brew install jq`). This gate did NOT run — it is not evidence of anything.");
        return;
    }
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rung3.json");
    let out = util::assert_rung_records_and_replays(JQ, &[".name", fixture], b"\"retrace\"\n");
    assert_eq!(out.stdout, b"\"retrace\"\n");
}
```

- [ ] **Step 3: Run it**

Run: `cargo test -p retrace --test jq_file_e2e -- --test-threads=1`
Expected: PASS (or a loud skip if jq is absent).

- [ ] **Step 4: Commit**

```bash
git add crates/retrace/tests/fixtures/rung3.json crates/retrace/tests/jq_file_e2e.rs
git commit -m "M10 t7: pin rung 3 — jq over a file argument (already passing; now gated)"
```

---

### Task 8: Documentation and the honest close

**Files:**
- Modify: `README.md` (new Status section after M9-jq's, which ends at line 1538), `CLAUDE.md`
  (milestone list ~line 143; the gate-count paragraph ~line 153)

- [ ] **Step 1: Write the README Status section**

Follow the M9-jq section's shape. It must state, without softening:

- What runs: the guest's fds are its own; `fdtable_e2e` proves fd 3 and `EBADF`; rung 3 is gated.
- **That rung 3 already passed before the milestone began** — the fd table was the work, not the rung.
- **That R1 fired during spec authoring** (a truncated histogram hid `read_nocancel`/`socket`/…), and
  what that implies for the next syscall table anyone writes here.
- The new boundary, honestly. Carry forward every unresolved item from M9's list that M10 did not
  touch: guest-raised signal delivery (still the top item), block-exclusive exec placement still not
  retired, the anon `PROT_EXEC`/JIT gap, `prot` ignored except `PROT_EXEC` (R3), `guest_munmap`'s
  wholesale-drop defect, the `guest_mmap_replay` rename, threads, arm64e guests. Add what M10 leaves:
  guest **stdin** (fd 0 is still retrace's), `RLIMIT_NOFILE` unenforced, and `F_DUPFD` unimplemented
  (measured absent from `jq`; a fail-loud arm, not support).

- [ ] **Step 2: Update `CLAUDE.md`**

Add **M10-fdtable** to the milestone list, and update the gate-count paragraph to the real number from
Step 3 — do not guess it.

- [ ] **Step 3: Run the full gate and record the true count**

Run: `just gate 2>&1 | tee /tmp/m10-gate.log`
Then: `grep -a "^test result:" /tmp/m10-gate.log | awk '{p+=$4; f+=$6; i+=$8} END {print p, f, i}'`

Put that exact triple in `CLAUDE.md` and the README. Expected ≈ 185 + the new tests, 0 failed,
**0 ignored**.

- [ ] **Step 4: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "M10 t8: the M10-fdtable Status section and the honest close"
```

---

## Self-Review

**Spec coverage.** Split table → T2. `(syscall → fd operand)` mapping incl. `_nocancel` pairing → T1.
Translation at both call sites → T3. `EBADF` → T2/T3. Allocation-on-return incl. `socket` → T1/T3.
`BoxState` carriage → T4. Symmetric replay posture → T5. `fd 3, not 17` guest → T6. Rung-3 gate → T7.
Status/honest close → T8. Spec open question 1 (`F_DUPFD`) is **resolved** — measured absent from
`jq`'s 17 `fcntl` calls (`F_GETPATH`×10, `F_ADDFILESIGS_RETURN`×4, `F_CHECK_LV`×2, `F_SETFD`×1) — so
`fcntl` gets plain x0 translation and the dup-family commands get no speculative support. Open
question 5 (socket receive) is folded into T1 Step 0's re-derivation.

**Placeholder scan.** No TBD/TODO. Two steps carry explicit implementer notes where the exact existing
helper must be read first (T3 Step 1's constructor trap, T5 Step 1's snapshot construction) — these
name the alternative concretely rather than deferring the decision.

**Type consistency.** `fd_operands`/`allocates_fd`/`AT_FDCWD` (T1) are used with those names in T3/T5.
`FdTable::{new,alloc,bind,host,is_open,close,slots,from_slots}` and `FdSlot::{Free,Open,Closed}` (T2)
are used consistently in T3/T4. `translate_fds`/`bind_returned_fd` are free functions over `FdTable`
per T3's preferred shape. `EBADF` is the box's constant throughout; the guest asserts the numeric 9.

**Known risk carried into execution:** T3 is a single atomic task with a full-gate checkpoint because
it cannot be split without a red gate. If it proves too large in practice, the honest split is
"translation returning `Ok` always" first — but that is a fake-green step, so prefer keeping it whole.
