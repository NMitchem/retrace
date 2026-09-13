# M37-classb Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close M36's two class-B walls — model `dup2` in the M10 fd table (the console becomes a
slot kind so aliases of stdout/stderr stay mirrored), and stop `forward_and_diff` probing
`Scalar` registers (the M34 §4b pid mis-translation) — with the audit that fix requires, then
re-measure the sweep in three pid regimes and move the eight parked gates to their new walls.

**Architecture:** `FdSlot::Console(u8)` + `FdTable::dup2` (pure, both sides) with a record-only
host `dup` inside `forward_and_diff`; a replay mirror inside the generic arm (no new returning
arm); console predicates read the table through one `Box_` method. The probe skips positions the
`arg_kinds` row marks `Scalar` and nothing else; the audit is static + two full-corpus sweeps
(pre-fix baseline vs post-fix) at a non-colliding pid. Evidence under
`docs/sweep-evidence/2026-09-13-m37/`.

**Tech Stack:** Rust 1.95 (pinned), Hypervisor.framework, POSIX sh (`tools/apple-sweep.sh`),
freestanding arm64 asm + C guest fixtures via `crates/retrace-guest/build.rs`.

**Spec:** `docs/superpowers/specs/2026-09-13-retrace-m37-classb-design.md`

## Global Constraints

- **`--test-threads=1`** on every `cargo test` (one VM per process); target pinned by `.cargo/config.toml`.
- **No `TRACE_MAGIC` bump.** No `Event` shape changes; `FdSlot` is box state, never traced. A diff
  touching `crates/retrace-trace/src/lib.rs` is a defect.
- **Symmetry rule 1 / seven `verify_thread` sites**: no new returning arm in `record_box` or
  `ReplaySession::advance`. `dup2` is handled inside `forward_and_diff` (record) and inside the
  generic replay arm beside the `allocates_fd`/`close` mirrors (replay). `grep -c verify_thread`
  in `crates/retrace-core/src/lib.rs` is unchanged by this milestone.
- **Only `Scalar` positions skip the probe.** `Fd`, `Ptr`, `Path`, `Source`, `Dest`, `Nested*`
  and every position past a row's arity keep it (spec §3b, §7).
- **A displaced host mapping is closed iff `> 2`** — retrace's own 0/1/2 are never closed by the
  guest's `dup2` (the M9 hazard).
- **Every positive control in spec §4 is run red and its output pasted** in the task report.
- **The audit's three measurements** (spec §3b) are recorded in `docs/sweep-evidence/2026-09-13-m37/README.md`
  with the reader source pasted (as M36 pasted `sym2.c`); a finding is a table fix in this
  milestone, ledgered.
- **Commit messages** end with the two attribution lines the session mandates. **Never push.**
- **The Bash tool ceiling is 10 minutes**: every sweep runs `nohup … &` and is polled with bounded
  `until` loops; the full gate is chunked (the M36 `gate.sh` with paths changed).
- The pid counter is advanced with `/usr/bin/true` loops as M36 did; every ROW line's `recpid`
  is asserted inside its run's band with `awk` before a run's results are used.

---

### Task 1: The harness keeps everything, and the pre-fix baseline is taken

**Files:**
- Modify: `tools/apple-sweep.sh` (the `keep_row` condition + header + usage)
- Evidence (workspace, not committed until Task 4): `<sdd>/baseline/` — `sweep-base.log`, `keep-base/`

**Interfaces:**
- Produces: `RETRACE_SWEEP_KEEP_ALL=1` (with `RETRACE_SWEEP_KEEP=<dir>`: keep every row's
  `.bin`/`.rec.err`/`.rp.err`/`.rp.out`, PASS rows included). Task 4 consumes the baseline.

- [ ] **Step 1: The env.** In `tools/apple-sweep.sh`, after `KEEP=${RETRACE_SWEEP_KEEP:-}` add
  `KEEP_ALL=${RETRACE_SWEEP_KEEP_ALL:-}`; change `keep_row`'s condition from
  `[ -n "$KEEP" ] && { [ "$1" != "PASS" ] || [ -n "${2:-}" ]; }` to
  `[ -n "$KEEP" ] && { [ -n "$KEEP_ALL" ] || [ "$1" != "PASS" ] || [ -n "${2:-}" ]; }`, and copy
  `rp.out` too when it exists (`[ -e "$TMP/rp.out" ] && cp "$TMP/rp.out" "$KEEP/$b.rp.out"`).
  Header: one line under the `RETRACE_SWEEP_KEEP` usage line: `RETRACE_SWEEP_KEEP_ALL=1   with
  KEEP: keep PASS rows too (the M37 audit's full-corpus baseline)`. `sh -n tools/apple-sweep.sh`.

- [ ] **Step 2: Control — the three-binary list.** `RETRACE_SWEEP_LIST=<list of /usr/bin/true,
  /bin/csh, /bin/launchctl> RETRACE_SWEEP_KEEP=<dir> RETRACE_SWEEP_KEEP_ALL=1 tools/apple-sweep.sh`
  → `true.{bin,rec.err,rp.err,rp.out}` present (PASS kept), the other two as before. Without
  `KEEP_ALL`: `true.*` absent. Paste `ls` of both.

- [ ] **Step 3: Build the pre-fix binary once.** From `main` (`648d4cf`): `git worktree add
  <scratch>/m37-base 648d4cf && (cd <scratch>/m37-base && cargo build -p retrace)`, then copy
  `target/aarch64-apple-darwin/debug/retrace` to `<sdd>/baseline/retrace-648d4cf` and codesign it
  (`codesign -s - -f --entitlements retrace.entitlements <copy>`); remove the scratch worktree.
  Record `shasum -a 256` of the copy in the report.

- [ ] **Step 4: The baseline sweep, non-colliding pid.** Wrap the pid counter past 99998 (a
  `/usr/bin/true` loop; `sh -c 'echo $$'` must print `< 12000` before starting, leaving room for
  the sweep's ~2,200 spawns to stay `< 0x4000` = 16384). Then, detached:
  `RETRACE_SWEEP_KEEP=<sdd>/baseline/keep-base RETRACE_SWEEP_KEEP_ALL=1 tools/apple-sweep.sh
  <sdd>/baseline/retrace-648d4cf >> <sdd>/baseline/sweep-base.log 2>&1` with `pidstart=` echoed
  first and `SWEEP_EXIT=` last. Assert with `awk` over the ROW lines: 54 rows, every `recpid`
  `< 16384`, none empty. Expected tally 45/9/0 with M36 run L's nine labels (the six RCV-shape
  rows, `csh`/`tcsh` panics, `yes` timeout). 54 `.bin` files kept (`yes.bin` may be huge — it is
  the record of a killed run; keep it or note its size).

- [ ] **Step 5: Commit** `M37 t1: the sweep can keep every row, and the pre-fix baseline is taken`
  (only `tools/apple-sweep.sh` is committed; the baseline stays in the workspace for Task 4).

---

### Task 2: `dup2`, modelled

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs` (`SYS_DUP2` row `:468`; `Ret` rustdoc `:278–282`;
  tests `:1347`, `:1489–1491`; `is_console_write` `:22–24` → `is_write_syscall`; `is_console_close`
  `:37–39` → `is_close_syscall`)
- Modify: `crates/retrace-arch/tests/legacy_equivalence.rs` (`EXPECTED_DIFFS` + one entry)
- Modify: `crates/retrace-box/src/lib.rs` (`FdSlot` `:675`, `FdTable` `:691–775`,
  `forward_and_diff` `:3167`, new `guest_dup2`, `is_console_write`/`is_console_close` methods)
- Modify: `crates/retrace-box/tests/fdtable.rs` (+4 tests)
- Modify: `crates/retrace-core/src/lib.rs` (`:153`, `:235`, `:254`, `:1140` assert deleted,
  `:1817`, replay mirror at `:2340–2356`)
- Create: `crates/retrace-guest/c/dup2_dyn.c`; modify `crates/retrace-guest/build.rs` (recipe as
  `fdtable_dyn` `:310–318`) and `crates/retrace-guest/src/lib.rs` (`pub const DUP2_DYN`)
- Create: `crates/retrace/tests/dup2_e2e.rs`

**Interfaces:**
- Consumes: `FdTable::{new, alloc, bind, host, is_open, close, slots, from_slots}`;
  `util::assert_rung_records_and_replays(guest, argv, expect_stdout) -> RungOut { trace, stdout }`.
- Produces: `pub enum FdSlot { Free, Open, Closed, Console(u8) }`;
  `FdTable::dup2(&mut self, fd: u64, fd2: u64, host_fd2: Option<i32>) -> Result<u64, (u64, Option<i32>)>`
  — hmm, see Step 2 for the exact signature; `FdTable::console_of(gfd) -> Option<u8>`;
  `Box_::is_console_write(num, gfd) -> bool`, `Box_::is_console_close(num, gfd) -> bool`;
  `retrace_arch::{is_write_syscall, is_close_syscall}`.

- [ ] **Step 1: Failing unit tests for the table op** — append to `crates/retrace-box/tests/fdtable.rs`:

```rust
// M37. dup2: the target slot is the guest's own number, and the console is a slot KIND so an
// alias of stdout stays a console write (spec §3a).
#[test]
fn dup2_propagates_the_console_kind_to_the_target_slot() {
    let mut t = FdTable::new();
    let (ret, displaced) = t.dup2(1, 17, Some(41)).expect("dup2(1, 17) succeeds");
    assert_eq!(ret, 17);
    assert_eq!(displaced, None, "17 was free: nothing displaced");
    assert_eq!(t.slots()[17], FdSlot::Console(1), "17 is an alias of stdout");
    assert_eq!(t.console_of(17), Some(1));
    assert_eq!(t.host(17), Some(41));
    assert!(t.is_open(17));
    assert_eq!(t.alloc(), 3, "alloc still takes the lowest free slot; 17 is open, not in the way");
}

#[test]
fn dup2_onto_an_open_slot_displaces_it_and_hands_back_its_host_fd() {
    let mut t = FdTable::new();
    let a = t.alloc(); t.bind(a, 30);            // guest 3 -> host 30
    let (ret, displaced) = t.dup2(0, a, Some(31)).unwrap();
    assert_eq!(ret, a);
    assert_eq!(displaced, Some(30), "the caller closes the displaced host fd");
    assert_eq!(t.slots()[a as usize], FdSlot::Console(0));
    assert_eq!(t.host(a), Some(31));
}

#[test]
fn dup2_onto_a_console_slot_makes_it_a_plain_open_descriptor() {
    let mut t = FdTable::new();
    let f = t.alloc(); t.bind(f, 30);
    let (ret, displaced) = t.dup2(f, 1, Some(32)).unwrap();
    assert_eq!(ret, 1);
    assert_eq!(displaced, Some(1), "the identity mapping is handed back; the CALLER must not close it (<= 2)");
    assert_eq!(t.slots()[1], FdSlot::Open, "stdout's slot is now a plain descriptor of f's file");
    assert_eq!(t.console_of(1), None, "a write to 1 is no longer a console write");
    assert_eq!(t.host(1), Some(32));
}

#[test]
fn dup2_self_is_a_no_op_and_a_closed_source_is_ebadf() {
    let mut t = FdTable::new();
    let f = t.alloc(); t.bind(f, 30);
    assert_eq!(t.dup2(f, f, None).unwrap(), (f, None), "dup2(fd, fd) returns fd and changes nothing");
    assert_eq!(t.host(f), Some(30));
    assert!(t.close(f));
    assert_eq!(t.dup2(f, 9, Some(33)), Err(retrace_box::EBADF));
    assert!(!t.is_open(9), "a failed dup2 opens nothing");
    // replay's shape: no host fd, same guest-visible result
    let mut r = FdTable::from_slots(&t.slots());
    assert_eq!(r.dup2(2, 18, None).unwrap(), (18, None));
    assert_eq!(r.slots()[18], FdSlot::Console(2));
    assert_eq!(r.host(18), None, "replay carries no host mapping for an alias");
}
```

  Run: `cargo test -p retrace-box --test fdtable -- --test-threads=1` → 4 compile errors (no
  `Console`, no `dup2`, no `console_of`).

- [ ] **Step 2: The table.** In `crates/retrace-box/src/lib.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdSlot {
    Free, Open, Closed,
    /// M37: this slot is (an alias of) console descriptor `n` (0, 1 or 2). The console is a slot
    /// KIND, not a number: `dup2(1, 17)` makes 17 a console write that M9's mirror must catch, and
    /// `dup2(f, 1)` makes 1 a plain descriptor it must not. `FdTable::new` seeds 0/1/2 with it.
    Console(u8),
}
```

  `new()` → `slots: vec![FdSlot::Console(0), FdSlot::Console(1), FdSlot::Console(2)]`.
  `is_open` → `matches!(self.slots.get(gfd as usize), Some(FdSlot::Open | FdSlot::Console(_)))`.
  `alloc` → `.find(|&i| !matches!(self.slots[i], FdSlot::Open | FdSlot::Console(_)))`.
  `from_slots` → identity host for `slots[n] == FdSlot::Console(n as u8)` with `n < 3`
  (an alias slot elsewhere stays `None`). Add:

```rust
    /// M37: which console descriptor this slot stands for, if any — the fd-number test M9 made
    /// by `fd == 1 || fd == 2`, now a table lookup so `dup2` aliases and displacements are seen.
    pub fn console_of(&self, gfd: u64) -> Option<u8> {
        match self.slots.get(gfd as usize) { Some(FdSlot::Console(n)) => Some(*n), _ => None }
    }

    /// M37: `dup2(fd, fd2)` on the guest-visible table — identical on record and replay.
    ///
    /// `Err(EBADF)` if `fd` is not open. `Ok((fd2, displaced))`: `fd2` becomes a duplicate of `fd`
    /// — it takes `fd`'s KIND (a `Console(n)` source makes `fd2` a console alias; an `Open` source
    /// makes `fd2` plain, even when `fd2` was a console slot) and the host mapping `host_fd2`
    /// (record: the `dup`; replay: `None`). `displaced` is whatever host mapping `fd2` held before,
    /// for the CALLER to close — and the caller must close it only when it is > 2: a displaced
    /// identity mapping is retrace's own stdin/stdout/stderr. `dup2(fd, fd)` returns `(fd, None)`
    /// and changes nothing (POSIX).
    pub fn dup2(&mut self, fd: u64, fd2: u64, host_fd2: Option<i32>) -> Result<(u64, Option<i32>), u64> {
        if !self.is_open(fd) { return Err(EBADF); }
        if fd == fd2 { return Ok((fd2, None)); }
        self.grow_to(fd2 as usize);
        let kind = self.slots[fd as usize];
        let displaced = self.host[fd2 as usize];
        self.slots[fd2 as usize] = kind;
        self.host[fd2 as usize] = host_fd2;
        Ok((fd2, displaced))
    }
```

  (`grow_to` must also happen before reading `self.host[fd2]` — it does, above.) Step 1's tests
  pass: `cargo test -p retrace-box --test fdtable -- --test-threads=1`. Also run
  `cargo test -p retrace-box --test checkpointparity -- --test-threads=1` (`:359–361` still hold —
  those slots are `Open`/`Closed`).

- [ ] **Step 3: The predicates.** `crates/retrace-arch/src/lib.rs`: replace `is_console_write`
  with `pub fn is_write_syscall(num: u64) -> bool { num == SYS_WRITE || num == SYS_WRITE_NOCANCEL }`
  and `is_console_close` with `pub fn is_close_syscall(num: u64) -> bool { num == SYS_CLOSE || num == SYS_CLOSE_NOCANCEL }`,
  keeping each rustdoc's M9 argument and adding one sentence: "M37: the fd half of the test moved
  into the box (`Box_::is_console_write`), because after `dup2` the console is a slot kind, not a
  number." Fix any unit test in that file that called the old names. In `crates/retrace-box/src/lib.rs`
  (near `fds()`):

```rust
    /// M37: the ONE console-write predicate, shared by record's mirror arm, replay's mirror and
    /// the trace-log echo. A write to any slot the table marks `Console(1|2)` — fd 1/2 themselves
    /// until the guest `dup2`s over them, and every alias `dup2` created — is mirrored and faked.
    pub fn is_console_write(&self, num: u64, gfd: u64) -> bool {
        retrace_arch::is_write_syscall(num) && matches!(self.fds.console_of(gfd), Some(1 | 2))
    }
    /// M37: a close of a `Console(_)` slot is faked (M9); a displaced console slot is a real
    /// descriptor and closes through the generic path.
    pub fn is_console_close(&self, num: u64, gfd: u64) -> bool {
        retrace_arch::is_close_syscall(num) && self.fds.console_of(gfd).is_some()
    }
```

  `crates/retrace-core/src/lib.rs`: `:153` → `b.is_console_write(*num, args[0])`; `:235` →
  `Stop::Syscall { num, args } if b.is_console_write(num, args[0]) =>`; `:254` →
  `if b.is_console_close(num, args[0])`; `:1817` → `self.b.is_console_write(num, args[0])`.
  `cargo build --workspace` clean.

- [ ] **Step 4: Record side — `guest_dup2` inside `forward_and_diff`.** At the top of
  `forward_and_diff` (before `let gargs = args;`):

```rust
        // M37: dup2 names its own target slot; it is a TABLE operation with a host `dup` behind
        // it, never a forwarded dup2 — `dup2(h, fd2)` on the host would overwrite retrace's own
        // descriptor `fd2`. Kept inside this function so it still owns both halves of the fd
        // contract (M10). Replay mirrors the table half in `ReplaySession::advance`.
        if num == retrace_arch::SYS_DUP2 { return self.guest_dup2(args); }
```

  and the method:

```rust
    fn guest_dup2(&mut self, args: [u64; 8]) -> (u64, bool, Vec<Region>) {
        let (fd, fd2) = (args[0], args[1]);
        let Some(h) = self.fds.host(fd) else { return (EBADF, true, Vec::new()); };
        if fd == fd2 { return (fd2, false, Vec::new()); }
        let dup = unsafe { libc::dup(h) };
        if dup < 0 {
            let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(EBADF as i32) as u64;
            return (e, true, Vec::new());
        }
        match self.fds.dup2(fd, fd2, Some(dup)) {
            Ok((ret, displaced)) => {
                // A displaced identity mapping (<= 2) is retrace's own console; never closed.
                if let Some(d) = displaced { if d > 2 { unsafe { libc::close(d); } } }
                (ret, false, Vec::new())
            }
            Err(e) => { unsafe { libc::close(dup); } (e, true, Vec::new()) }
        }
    }
```

  Delete the `assert!(num != retrace_arch::SYS_DUP2, …)` block at `retrace-core/src/lib.rs:1136–1142`
  (comment included). `arg_kinds`: `SYS_DUP2 => row!(P, [Fd, Scalar]),` with the comment
  "dup2(int fd, int fd2): fd2 is the guest's own TARGET slot, never translated (M37 models it in
  `forward_and_diff::guest_dup2`); the return is the slot, not a fresh allocation (Ret::Plain)".
  Tests: `:1347` → `[0]`; `:1489–1491` → `assert!(!allocates_fd(SYS_DUP2), "dup2 returns its own target slot (M37), not a fresh allocation")`.
  Rustdoc `:278–282` → past tense + "M37 models it". `legacy_equivalence.rs` `EXPECTED_DIFFS`:
  `(90, View::FdOperands, "dup2(fd, fd2): fd2 is the guest's target slot, not a descriptor to translate — M37; exercised (/bin/csh, /bin/tcsh: dup2(0,16) (1,17) (2,18) (16,19))"),`.
  `cargo test -p retrace-arch -- --test-threads=1` green.

- [ ] **Step 5: Replay mirror.** In `ReplaySession::advance`'s generic arm, immediately before
  the `if !*err && retrace_arch::allocates_fd(num)` block (`:2340`):

```rust
                            // M37 dup2 mirror: the table half is a pure function of the guest's
                            // sequence, so recompute (ret, err) and byte-compare — the same posture
                            // as the fd mirror below. No host fd on replay.
                            if num == retrace_arch::SYS_DUP2 {
                                let (rret, rerr) = match self.b.fds_mut().dup2(args[0], args[1], None) {
                                    Ok((r, _)) => (r, false),
                                    Err(e) => (e, true),
                                };
                                if (rret, rerr) != (*ret, *err) {
                                    return Err(Divergence { landmark: self.idx, pc, detail: format!(
                                        "dup2 divergence: recording says dup2({}, {}) returned ({ret}, err={err}), \
                                         the guest's own table yields ({rret}, err={rerr})", args[0], args[1]) });
                                }
                            }
```

  `grep -c verify_thread crates/retrace-core/src/lib.rs` unchanged (record the number).

- [ ] **Step 6: The fixture.** `crates/retrace-guest/c/dup2_dyn.c`:

```c
// M37. The dup2 fixture: aliases of the console stay console writes, a console slot displaced by
// a file becomes a file write, dup2 onto an open slot displaces it, self-dup2 is a no-op, and a
// closed source is EBADF. argv[1] is a file path the test owns.
//
// Expected stdout (record == replay, bit for bit):   alias\nself=1\nebadf=1\n
// Expected file after record:                        file18\nfile17\nvia1\n
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    int f = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (f < 0) return 3;
    dup2(1, 17); write(17, "alias\n", 6);        /* console alias: mirrored, not forwarded */
    dup2(f, 18); write(18, "file18\n", 7);        /* plain duplicate: the file */
    dup2(f, 17); write(17, "file17\n", 7);        /* displaces the alias: the file now */
    int s = dup2(f, f);
    printf("self=%d\n", s == f);
    int e = dup2(40, 19);
    printf("ebadf=%d\n", e == -1 && errno == EBADF);
    fflush(stdout);
    dup2(f, 1);                                   /* stdout IS the file from here */
    printf("via1\n");
    fflush(stdout);
    close(17); close(18); close(f);
    return 0;
}
```

  `build.rs`: the `fdtable_dyn` recipe (`:310–318`) duplicated for `dup2_dyn` (`clang -arch arm64
  -o $out/dup2_dyn c/dup2_dyn.c`, `rerun-if-changed`); `src/lib.rs`: `pub const DUP2_DYN: &str =
  concat!(env!("OUT_DIR"), "/dup2_dyn");` beside `FDTABLE_DYN`, plus the parse test the siblings have.

- [ ] **Step 7: The gate.** `crates/retrace/tests/dup2_e2e.rs`:

```rust
// M37 gate. dup2 is modelled: the console is a slot KIND, so an alias of stdout is still mirrored
// into the trace (record == replay stdout), and a console slot displaced by a file is a file
// write on both sides. Asserts on the bytes, never on an exit code (CLAUDE.md's first gate rule).
mod util;

const EXPECT_STDOUT: &[u8] = b"alias\nself=1\nebadf=1\n";
const EXPECT_FILE: &[u8] = b"file18\nfile17\nvia1\n";

fn scratch_file() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("retrace-dup2-{}.txt", std::process::id()))
}

#[test]
fn console_aliases_are_mirrored_and_displaced_console_slots_write_the_file() {
    let path = scratch_file();
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUP2_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    // The file is written on RECORD only (replay forwards nothing); its bytes are the other half
    // of the model: 18 and 17 were plain duplicates of f, and 1 became one.
    let file = std::fs::read(&path).expect("the fixture created its file on record");
    assert_eq!(file, EXPECT_FILE, "file bytes: {:?}", String::from_utf8_lossy(&file));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with("alias\n"), "write(17) after dup2(1, 17) must be a mirrored console write. Got:\n{s}");
    assert!(!s.contains("via1"), "printf after dup2(f, 1) must reach the file, not the console. Got:\n{s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_trace_carries_dup2_as_a_plain_landmark_returning_the_guest_target() {
    let path = scratch_file();
    let _ = std::fs::remove_file(&path);
    let out = util::assert_rung_records_and_replays(retrace_guest::DUP2_DYN, &[path.to_str().unwrap()], EXPECT_STDOUT);
    let events = retrace_trace::Reader::open(&out.trace).unwrap();
    let mut seen = Vec::new();
    for e in events.iter() {
        if let retrace_trace::Event::Syscall { num, args, ret, err, writes, .. } = e {
            if *num == retrace_arch::SYS_DUP2 {
                assert!(writes.is_empty(), "dup2 writes no guest memory");
                seen.push((args[0], args[1], *ret, *err));
            }
        }
    }
    // (1,17) (f,18) (f,17) (f,f) (40,19)->EBADF (f,1): six landmarks; the successful ones return
    // their TARGET (a guest number < 16), never a host descriptor.
    assert_eq!(seen.len(), 6, "expected six dup2 landmarks, saw {seen:?}");
    for (fd, fd2, ret, err) in &seen {
        if *fd == 40 { assert!(*err && *ret == 9, "dup2(40, 19) is EBADF: {seen:?}"); }
        else { assert!(!*err && *ret == *fd2 && *fd2 < 20, "dup2({fd}, {fd2}) returned {ret}: {seen:?}"); }
    }
    let _ = std::fs::remove_file(&path);
}
```

  `cargo test -p retrace --test dup2_e2e -- --test-threads=1` → 2 passed. Also
  `cargo test -p retrace --test fdtable_e2e --test hello_dyn_e2e --test jq_e2e --test cpython_e2e -- --test-threads=1`
  (console-predicate consumers) green.

- [ ] **Step 8: Positive controls 1 and 2 (spec §4).** (1) Temporarily make `Box_::is_console_write`
  return `retrace_arch::is_write_syscall(num) && (gfd == 1 || gfd == 2)`; run `dup2_e2e` → the
  first test fails (paste the assertion: replay stdout lacks `alias\n`, or record stdout has it
  twice — record whichever shape appears); revert. (2) Temporarily delete the replay mirror's
  `dup2` call (leave the block empty); run → fails (replay stdout gains `via1\n`); revert. Confirm
  `git diff` is clean of both mutations before committing.

- [ ] **Step 9: Clippy + commit.** `cargo clippy --workspace --all-targets -- -D warnings`.
  Commit `M37 t2: dup2 is modelled — the console is a slot kind, and an alias of stdout stays mirrored`.

---

### Task 3: A `Scalar` is never a pointer — the probe skip, its control, and the static audit

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` (`forward_and_diff` loop `:3186–3204`)
- Modify: `crates/retrace-arch/src/lib.rs` (`ArgKind` rustdoc `:140–142`, `Scalar` variant doc)
- Create: `crates/retrace-guest/asm/scalarprobe.s`; modify `build.rs` (recipe as `failproc`
  `:96–104`) and `src/lib.rs` (`pub const SCALARPROBE`)
- Create: `crates/retrace-box/tests/scalarprobe.rs`
- Report: the static audit table (every `Scalar` position, with its prototype)

**Interfaces:**
- Consumes: `retrace_arch::forwarded_shape(num) -> &'static Shape` (`Shape.args: &[ArgKind]`).
- Produces: nothing new in code; the audit table for Task 4's README.

- [ ] **Step 1: The fixture.** `crates/retrace-guest/asm/scalarprobe.s`:

```asm
// M37: a guest whose lseek OFFSET is 0x4000 = TRAMPOLINE_IPA, a mapped guest IPA on every load
// path. Before M37, forward_and_diff's per-register probe rewrote any register holding a mapped
// IPA to a host pointer — including this offset, which is a NUMBER — and the host lseek returned
// the trampoline's host address. The arg_kinds row marks lseek's offset Scalar; a Scalar is never
// probed now, so the call returns 0x4000. (M34 §4b found the same rewrite on the recorder's pid.)
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // fd = open("/etc/hosts", O_RDONLY)
    adrp x0, path@PAGE
    add  x0, x0, path@PAGEOFF
    mov  x1, #0
    mov  x2, #0
    mov  x16, #5                // SYS_open
    svc  #0x80
    mov  x19, x0
    // lseek(fd, 0x4000, SEEK_SET) — the offset is the probe's bait
    mov  x0, x19
    mov  x1, #0x4000
    mov  x2, #0
    mov  x16, #199              // SYS_lseek
    svc  #0x80
    // exit(0)
    mov  x0, #0
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
path: .asciz "/etc/hosts"
```

  `build.rs`: the `failproc` recipe duplicated (`-nostdlib -static -Wl,-e,_start`); `lib.rs`:
  `pub const SCALARPROBE`.

- [ ] **Step 2: The failing test.** `crates/retrace-box/tests/scalarprobe.rs`:

```rust
use retrace_box::*;

// M37 positive control for the §4b fix (spec §4 item 3). `lseek`'s offset register holds 0x4000,
// TRAMPOLINE_IPA. Pre-fix, forward_and_diff's probe rewrote it to the trampoline's host address
// and the kernel returned THAT (a 47-bit number); post-fix a Scalar is forwarded verbatim and
// lseek returns 0x4000. Run red on the pre-fix tree first — the report pastes the value it saw.
#[test]
fn a_scalar_register_holding_a_mapped_ipa_is_forwarded_verbatim() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::SCALARPROBE).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == retrace_arch::SYS_LSEEK => {
                assert_eq!(args[1], 0x4000, "precondition: the guest asked for offset 0x4000");
                assert!(b.host_span_for_test(0x4000).is_some(), "precondition: 0x4000 is a mapped IPA (the trampoline)");
                let (ret, err, _w) = b.forward_and_diff(num, args);
                assert!(!err, "lseek failed: errno {ret}");
                assert_eq!(ret, 0x4000,
                    "lseek returned {ret:#x}: the offset register was rewritten to a host pointer — \
                     forward_and_diff probed a Scalar position (M34 §4b's class)");
                return;
            }
            Stop::Syscall { num, args } => {
                let (ret, err, _w) = b.forward_and_diff(num, args);
                b.set_x0_err_and_return(ret, err);
            }
            Stop::Other { esr } => panic!("unexpected exit esr=0x{esr:x}"),
            Stop::Fault { pc, esr, far } => panic!("guest crashed pc=0x{pc:x} esr=0x{esr:x} far=0x{far:x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
```

  Run: `cargo test -p retrace-box --test scalarprobe -- --test-threads=1` → FAILS with
  `lseek returned 0x1…` (a host address). **Paste the value in the report** — that is the control.

- [ ] **Step 3: The fix.** In `forward_and_diff`, before the loop:

```rust
        // M37: a register the row marks `Scalar` carries a NUMBER — a pid, an offset, a flag word —
        // and is forwarded verbatim. Probing it was M34 §4b: a scalar that happened to equal a
        // mapped IPA (the recorder's own pid, on ~82 % of the pid space once the guest's
        // os_alloc_once slab lands at 0x10000 — M36) reached the host kernel as a host pointer, and
        // every self-pid csops/proc_info answered ESRCH. Only `Scalar` skips: positions past the
        // row's arity keep the probe (M30's stale-register band measurement rests on it) and every
        // memory kind needs it. The audit that licenses this is in
        // docs/sweep-evidence/2026-09-13-m37/README.md.
        let shape = retrace_arch::forwarded_shape(num);
        for i in 0..8 {
            if shape.args.get(i) == Some(&retrace_arch::ArgKind::Scalar) {
                hargs[i] = args[i] as i64;
                continue;
            }
            match self.host_span(args[i]) { … unchanged … }
        }
```

  (`hargs` is declared before the loop already.) `ArgKind` rustdoc `:140–142`: replace "`Scalar`,
  `Path` and `Ptr` change nothing at runtime — `forward_and_diff` probes `host_span` on all eight
  registers regardless — and are documentation until a later milestone consults them" with
  "`Scalar` is load-bearing since M37: `forward_and_diff` forwards it verbatim and never probes it
  (M34 §4b). `Path` and `Ptr` still change nothing at runtime — they are probed like any register
  — and remain documentation." The `Scalar` variant's doc gains the same sentence. Step 2's test
  passes; `failwrite`, `truncguard`, `fdtable` and the whole `retrace-box` package green:
  `cargo test -p retrace-box -- --test-threads=1`.

- [ ] **Step 4: The static audit.** For every row of `arg_kinds` (all of them — `sed -n
  '/pub fn arg_kinds/,/^}/p' crates/retrace-arch/src/lib.rs`), list every `Scalar` position as
  `(num, name, position, prototype parameter, source)` — source = `sys/syscall.h` +
  `bsd/kern/syscalls.master` names (the M35 scratchpad has xnu sources; the SDK header is at
  `$(xcrun --show-sdk-path)/usr/include/sys/syscall.h`), or the Mach trap prototype for negative
  numbers. Any position whose parameter is a pointer type is a **finding**: fix the row (to
  `Ptr`/`Source`/`Dest` as the prototype says), add the `EXPECTED_DIFFS` entry if a legacy view
  changes, and ledger it. Write the table to `<sdd>/task-3-audit-static.md` (Task 4 pastes it).

- [ ] **Step 5: Clippy + commit.** `cargo clippy --workspace --all-targets -- -D warnings`.
  Commit `M37 t3: a Scalar register is forwarded verbatim, never probed — with its control`.

---

### Task 4: The dynamic audit, the acceptance sweeps, the gates moved, the comments corrected

**Files:**
- Create: `docs/sweep-evidence/2026-09-13-m37/README.md` + `<b>.{N,I,S}.{rec,rp}.err` for the
  nine non-clean rows (N = non-colliding, I = inside `[0x4000,0x10000)`, S = the slab
  `[0x10000,0x18000)`)
- Rewrite: `crates/retrace/tests/apple_walls_e2e.rs` (eight reasons)
- Modify: `crates/retrace-core/src/machmsg.rs:97–99`, `crates/retrace-box/tests/truncguard.rs:237`
- Workspace: `<sdd>/sweeps/{sweep-N,sweep-I,sweep-S}.log`, `keep-{N,I,S}/`; `<sdd>/audit/`

**Interfaces:**
- Consumes: Task 1's `<sdd>/baseline/keep-base/*.bin` and `sweep-base.log`; Task 3's static
  audit table; the M36 counting rules (`docs/sweep-evidence/2026-09-13-m36/README.md` "Counting
  rules"); M35/M36 kept traces (`.superpowers/sdd/2026-09-13-retrace-m35-errholes/ddd-keep*/`,
  `.superpowers/sdd/2026-09-13-retrace-m36-sweepmeasure/keep-{O,L,I}/`).

- [ ] **Step 1: The reader.** A scratchpad crate (as M36's `errcount`, depending on this worktree's
  `crates/retrace-trace`) with three modes, its source pasted into the evidence README:
  `scalar-writes <trace>…` — for every `Event::Syscall`, for every position `i` where
  `retrace_arch::arg_kinds(num)` marks `Scalar`, report any `writes` region containing `args[i]`
  (expected: none, over every trace named above — that is audit measurement 2);
  `errs <trace>` — `err=true` count per syscall number (M36's `errcount`);
  `selfpid <trace> <pid>` — the count of 169/170/336 landmarks with the pid register `== pid`
  and `ret == 3`. (Depend on `retrace-arch` too, for `arg_kinds`.)

- [ ] **Step 2: The post-fix sweeps, three regimes, every trace kept.** Build the branch binary
  (`cargo build -p retrace`), then, one after another, each detached and polled: **N** — wrap the
  pid counter, start `< 12000`; **I** — advance to ~17,000; **S** — advance to ~66,000
  (`0x101D0`; the band is 65536..98303, the sweep spawns ~2,200 pids). Each:
  `RETRACE_SWEEP_KEEP=<sdd>/sweeps/keep-<R> RETRACE_SWEEP_KEEP_ALL=1 tools/apple-sweep.sh >>
  <sdd>/sweeps/sweep-<R>.log 2>&1` with `pidstart=`/`SWEEP_EXIT=`; assert every `recpid` inside
  the band with `awk`. Expected in all three (spec §5): 45/9/0; `csh`/`tcsh` `record error, rc=4:
  RECORD ERROR: unsupported mach_msg2 … msgh_id 3403 …`; the six §4b rows the RCV shape; `yes`
  the watchdog. Any other label is a finding: stop and ledger before going on.

- [ ] **Step 3: Audit measurement 3 — baseline vs N.** For all 54 rows: labels identical between
  `sweep-base.log` and `sweep-N.log` except the two expected moves (`csh`/`tcsh`: panic → 3403).
  For every binary with a `.bin` in both `keep-base/` and `keep-N/`: `errs` per syscall number
  identical (`diff` of the two reader outputs); list every difference by name and adjudicate it
  (a `ret=14` on a row with a `Scalar` position = a table defect → fix the row, re-run that
  binary, ledger). `yes` is excluded (killed mid-run). Write `<sdd>/audit/baseline-vs-N.md`.

- [ ] **Step 4: Acceptance — zero self-pid ESRCH.** For every kept trace in `keep-I/` and
  `keep-S/`: `selfpid <trace> <recpid>` = 0 (recpid from the ROW line). Any non-zero is a red
  (spec §5). Table in the README: binary × regime → 0.

- [ ] **Step 5: The gates.** Rewrite each `#[ignore]` reason in `apple_walls_e2e.rs` to the M37
  measurement — `csh`/`tcsh`: "M37 wall, class C (new subsystem: process creation), parked, not
  routed. `dup2` was the M36 wall and is modelled (M37 t2); the row now stops at `record error,
  rc=4: RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 3403 dest 0x203 … send_size
  64` — `mach_ports_register` from libxpc `xpc_atfork_prepare` ← `libSystem_atfork_prepare` ←
  `fork`; behind it `fork`(2) itself, which has no row. Identical in runs N/I/S (recpids …).
  Evidence docs/sweep-evidence/2026-09-13-m37/csh.{N,I,S}.rec.err. UN-IGNORE when the box models
  process creation." — the six §4b rows: "M37 wall, class C, parked, not routed. The B half (M34
  §4b) is retired: with `Scalar` positions never probed, runs N/I/S (recpids …, non-colliding /
  `[0x4000,0x10000)` / `[0x10000,0x18000)`) all stop at `record error, rc=4: RECORD ERROR:
  unsupported mach_msg2 at pc 0x1804adc34: options 0x404000102 …` (`mach_msg2_trap+8`,
  `Route::Unsupported`, the RCV-shaped message-queue call), landmark …, 0 self-pid ESRCH in every
  kept trace. Evidence …{N,I,S}.{rec,rp}.err. UN-IGNORE when the box services the RCV-shaped
  message-queue call." Keep the file header and helper. `cargo test -p retrace --test
  apple_walls_e2e -- --test-threads=1` → 0/0/8; `--ignored` → 8 failed, each at the new line
  (paste; note the pid regime). No placeholder survives.

- [ ] **Step 6: The two comments.** `machmsg.rs:97–99` → "M36 measured the `brk` those four
  binaries reach as libdispatch's own crash on a §4b-failed `proc_info`, not a consequence of
  this refusal — with a correctly-forwarded pid (M37) they reach the RCV-shaped message-queue
  call instead (docs/sweep-evidence/2026-09-13-m36/README.md, …/2026-09-13-m37/README.md)."
  `truncguard.rs:237` → the measured window `[0x4000, 0x18000)` with the slab, "retired by M37:
  a Scalar is never probed". `cargo test -p retrace-box --test truncguard -- --test-threads=1`
  and `cargo test -p retrace-core -- --test-threads=1` green.

- [ ] **Step 7: The evidence directory.** `docs/sweep-evidence/2026-09-13-m37/README.md`: binary
  commit, script commit, the three runs' tables (pidstart, recpid range, tally), the nine rows'
  labels per regime, the counting rules (M36's, by reference + the `selfpid` rule), the reader
  source, audit measurements 1 (Task 3's table, pasted), 2 (scalar-writes: 0 hits over N traces),
  3 (baseline vs N: the diff, adjudicated), the acceptance table (self-pid ESRCH 0 × 9 × 2), the
  `dup2` measurement from spec §2a (the four calls, the `fork` backtrace); one line per committed
  `.err` file. Byte total well under 100 KB.

- [ ] **Step 8: Clippy + commit.** `cargo clippy --workspace --all-targets -- -D warnings`.
  Commit `M37 t4: the audit, the acceptance sweeps, the gates at their new walls`.

---

### Task 5: Docs, the gate, the merge

**Files:**
- Modify: `README.md` ("What works today" Apple-binaries paragraph; the gate paragraph; Known
  limits: the sweep bullet's table + the §4b entry + the parked-gates bullet + the descriptor
  entry's `dup2` sentence)
- Modify: `docs/status-log.md` (append `## Status: M37-classb — dup2 modelled, a Scalar never
  probed, and the wall behind csh is fork`; never edit an earlier line)
- Modify: `docs/superpowers/specs/2026-09-13-retrace-m37-classb-design.md` (§11 only)

**Interfaces:**
- Consumes: Task 2–4 reports, the evidence README, the controller's `task-5-numbers.md`.

- [ ] **Step 1: README.** Edit in place: the §4b Known-limits entry is retired (moved to the
  log; the README keeps one sentence: "a `Scalar` register is never probed — M37"); the sweep
  table's six rows lose their B face (class C at the RCV shape on every pid); `csh`/`tcsh` rows:
  class C at `fork`; the descriptor entry: `dup2` modelled, `F_DUPFD` still named as unmodelled;
  the parked-gates bullet: still ten, reasons moved; the gate paragraph from the numbers file.
  Greps: `dup2 is not modelled`, `[0x4000, 0x18000)` (only as history), `82 %` (only as history),
  `M37` present tense → none.

- [ ] **Step 2: Status log.** The section, in M36's shape: what it set out to do (the two B
  walls); the pre-spec measurement (§2a verbatim: the four `dup2` calls, the `fork` backtrace);
  the `dup2` model (the slot kind, the two predicates, the mirror) and its two controls
  (verbatim assertions); the §4b fix, its unit control (the pre-fix value pasted), the static
  audit table, audit 2 and 3 with their adjudications; the three acceptance sweeps (tallies, pid
  ranges, every non-PASS line verbatim, the 0-ESRCH table); the gates moved (reasons, Control
  `--ignored` output); the two comment corrections; rulings (spec §8 + every ledger `Ruling:`);
  gate + reconciliation; **M38 does not exist** (charter §3) with the sentence; what stays owed
  (`fork`/process creation; the RCV-shaped `mach_msg2`; `F_DUPFD`; the console-close deferral;
  the probe past arity; whatever the audit adjudicated as deferred).

- [ ] **Step 3: Spec §11.** Outcome vs §9's prediction; what the audit found; corrections.

- [ ] **Step 4: Commit** `M37 t5: README, status-log section, spec outcome`.

- [ ] **Step 5 (controller): the gate.** The M36 `gate.sh` with paths changed; prediction from
  source (spec §9: 582 / 0 / 10 over 128 — reconcile file by file against M36's 575 / 0 / 10 over
  126: `fdtable.rs` +4, `scalarprobe.rs` +1 new binary, `dup2_e2e.rs` +2 new binary, everything
  else unchanged); `#[test]` counts diffed per file. Then `git merge --no-ff m37-classb` into
  local `main`; never push. Remove worktree + branch; memory file.

---

## Self-Review

**Spec coverage.** §3a → Task 2 (Steps 1–7); §3b → Task 3 (fix + static audit) and Task 4
(Steps 1, 3 — dynamic audits 2 and 3); §3c → Task 1; §3d → Task 4 Step 5; §3e → Task 4 Step 6;
§4 controls 1–2 → Task 2 Step 8, control 3 → Task 3 Step 2, control 4 → Task 4 Steps 2–4; §5 →
Task 4 Steps 2, 4; §6 → Task 2 Steps 4–5 (no returning arm; `verify_thread` count recorded); §7 →
Task 5 Step 2's owed list; §8 rulings → carried into Task 5; §9 → Task 5 Step 5; §10 → Task 5 Step 2.

**Placeholder scan.** Task 4 Step 5's reason templates carry `…` where the measured pids/landmarks
go — filled from the run logs, checked by `grep -n '…'` finding nothing in the test file.

**Type consistency.** `FdTable::dup2(fd: u64, fd2: u64, host_fd2: Option<i32>) ->
Result<(u64, Option<i32>), u64>` is the one signature (Task 2 Steps 1, 2, 4, 5 all use it; the
"Interfaces" line's first draft is superseded by Step 2). `console_of(gfd: u64) -> Option<u8>`.
`Box_::is_console_write(&self, num: u64, gfd: u64) -> bool`, `is_console_close` likewise.
`retrace_arch::{is_write_syscall, is_close_syscall}(num: u64) -> bool`. `SCALARPROBE`,
`DUP2_DYN` follow their siblings' `concat!(env!("OUT_DIR"), …)` shape.

**The things the plan cannot know**: (a) whether the static audit finds a wrong `Scalar` (then
Task 3 fixes rows and Task 4's audit 3 is the check); (b) whether audit 3 shows an unexpected
`err` delta on a PASS row (a finding to adjudicate — an `EFAULT` is a red, a moved `gettimeofday`
count is noise, named as such); (c) the exact `lseek` value the pre-fix control returns.
