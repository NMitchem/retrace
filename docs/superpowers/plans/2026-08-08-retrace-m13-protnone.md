# M13-protnone Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a `PROT_NONE` guest page actually fault, so libstd's stack-overflow guard page guards and a full-`std` Rust guest's stack overflow records and replays bit-for-bit.

**Architecture:** One new stage-1 attribute (`ATTR_NONE`, AP=`0b00`) stamped by the existing `set_region_exec_attr` machinery, tracked in a `noaccess` range map that mirrors `reservations` exactly, installed at four call sites, and followed by M9's `flush_guest_tlb` because — unlike every existing stamp site — the protected page is one the guest has already translated. The resulting stage-1 permission fault arrives as `Stop::Fault` and routes through M12's disposition check unchanged; a reserved-uncommitted page still faults at stage 2 into `Stop::Other`, so the hardware separates the two cases and no software gate is needed.

**Tech Stack:** Rust 1.95.0 (`aarch64-apple-darwin`), Hypervisor.framework, `clang` for freestanding arm64 guest fixtures, `rustc` for full-`std` guest fixtures.

## Global Constraints

- **macOS 26.x on Apple Silicon.** Every binary touching `hv_*` needs `com.apple.security.hypervisor` (ad-hoc signable).
- **`--test-threads=1` is mandatory** — one HVF VM per process. `just gate` sets it; a bare `cargo test` flakes with `HV_BUSY`.
- **`just gate` is THE exit gate:** `cargo test --workspace` + `clippy -D warnings`. Baseline entering this milestone: **296 passed / 0 failed / 0 ignored** (90 test binaries).
- **All six headline gates stay green and un-ignored:** `hello_dyn_e2e`, `hello_rust_e2e`, `jq_e2e`, `jq_file_e2e`, `panic_e2e`, `segv_rust_e2e`. A new wall gets a NEW parked gate, never a regression of these.
- **`clippy.toml` bans `Instant::now`/`SystemTime::now`/`std::thread`.** Load-bearing for determinism, not style.
- **SPTM: all guest memory is anonymous.** A file-backed `hv_vm_map` hard-panics macOS 26.
- **W^X:** executing a writable guest page hangs the vCPU. Code pages are RO+exec; data is RW+non-exec.
- **Drop order:** `Box_`'s `vcpu` field must stay declared before `vm`. Do not reorder struct fields.
- **`TRACE_MAGIC` must NOT change in this milestone.** If a task believes it needs a format break, that is a design error — stop and escalate.
- **Never reimplement Apple's PAC.** Sign/authenticate by running `pac*`/`aut*` on the guest vCPU.
- Branch: `m13-protnone`, already created, spec committed at `c8315ff`.
- Spec: `docs/superpowers/specs/2026-08-08-retrace-m13-protnone-design.md`. Read it before Task 1.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `spikes/protnone.c` | **New.** Native probe: which signal/`si_code` Darwin raises for a `PROT_NONE` access, with an unmapped control | 1 |
| `spikes/README.md` | Modify: build recipe + findings for `protnone.c` | 1 |
| `crates/retrace-core/src/lib.rs` | Modify: a `[commit]` `RETRACE_TRACE` line (record-only); `mach_vm_protect` record + replay arms routed into the box | 2, 9 |
| `crates/retrace-arch/src/lib.rs` | Modify: `signal_of_esr`'s permission-fault row, per the spike | 3 |
| `crates/retrace-box/src/lib.rs` | Modify: `subtract_range` extraction; `set_region_attr` rename; `leaf_desc` extraction; `ATTR_NONE`; `noaccess` field + accessor; `protect_none`/`unprotect`; `ipa_is_noaccess`; hooks in `map_mmap_region`, `guest_mprotect`, `guest_munmap`; `BoxState` + `checkpoint` + `from_checkpoint` + three landmark-0 constructors | 4, 5, 6, 7, 8, 9, 10 |
| `crates/retrace-box/tests/protnone.rs` | **New.** Box-level unit + guest tests for the map, the stamp, the fault, and the restore | 4, 5, 6, 7, 8, 10 |
| `crates/retrace-guest/asm/protnone.s` | **New.** touch → `mprotect` PROT_NONE → touch again → must fault | 7 |
| `crates/retrace-guest/asm/protrestore.s` | **New.** touch → PROT_NONE → back to RW → touch → exit 0 | 7 |
| `crates/retrace-guest/asm/protnone_mach.s` | **New.** the same through `mach_vm_protect` (svc −14) | 9 |
| `crates/retrace-guest/asm/protreserve.s` | **New.** fail-loud negative: `mprotect` PROT_NONE over an uncommitted reservation page | 10 |
| `crates/retrace-guest/rs/overflow.rs` | **New.** The headline guest: full-`std` Rust, recurses until the guard faults | 11 |
| `crates/retrace-guest/build.rs` | Modify: build the four asm guests and the Rust guest | 7, 9, 10, 11 |
| `crates/retrace-guest/src/lib.rs` | Modify: path constants for the five new guests | 7, 9, 10, 11 |
| `crates/retrace/tests/stackoverflow_rust_e2e.rs` | **New.** The headline gate + the reverse-debug seek gate | 11 |
| `README.md` | Modify: the M13 Status section | 12 |
| `CLAUDE.md` | Modify: gate count, milestone list, headline-gate set | 12 |

**Ordering rationale.** Measurement first (Tasks 1–2), matching M12's shape — every one of M12's five real defects came from executing the plan rather than reading it, and Tasks 1–2 exist to move two such discoveries to the front. `signal_of_esr` (Task 3) precedes every gate that asserts a signal number. The pure refactors (Task 4) land before the code that consumes them. The mechanism (Tasks 5–6) precedes its call sites (7–10). The headline (11) is last before the close (12).

---

### Task 1: Spike — what signal does Darwin raise for a `PROT_NONE` access? (R1)

**Files:**
- Create: `spikes/protnone.c`
- Modify: `spikes/README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: a measured `(signal, si_code)` pair for a `PROT_NONE` load and store, and the answer to "does an unmapped address still raise `SIGSEGV`". Task 3 consumes both.

**Why this is first.** `crates/retrace-arch/src/lib.rs:307` maps DFSC `0x0c..=0x0f` (permission fault) to `(SIGSEGV, SEGV_ACCERR)`. **No guest has ever produced that ESR** — every fault M6/M11/M12 recorded was a *translation* fault (`0x04..0x07`), where SIGSEGV is right on both platforms. Darwin's `ux_exception` maps `KERN_INVALID_ADDRESS` → SIGSEGV and everything else, including `KERN_PROTECTION_FAILURE`, → SIGBUS; libstd's comment on this exact code path says *"This ensures SIGBUS will be raised on stack overflow."* Three sources, two answers. Measure.

- [ ] **Step 1: Write the spike**

Create `spikes/protnone.c`:

```c
// M13 R1. Which signal does Darwin raise for a PROT_NONE access, and with what si_code?
//
// retrace's signal_of_esr maps an AArch64 permission fault (DFSC 0x0c..0x0f) to
// (SIGSEGV, SEGV_ACCERR) -- the Linux answer, and a row NO guest has ever exercised. Darwin's
// ux_exception translates EXC_BAD_ACCESS by code: KERN_INVALID_ADDRESS -> SIGSEGV, everything
// else (including KERN_PROTECTION_FAILURE) -> SIGBUS. libstd's install_main_guard comment says
// "This ensures SIGBUS will be raised on stack overflow." Three sources, two answers.
//
// The UNMAPPED control is not optional: if Darwin raised SIGBUS for that too, M6's crashy_e2e
// classification would be wrong as well, and this would be a much larger finding than M13.
#include <stdio.h>
#include <signal.h>
#include <string.h>
#include <setjmp.h>
#include <sys/mman.h>
#include <unistd.h>

static sigjmp_buf jb;
static volatile sig_atomic_t got_sig, got_code;
static void * volatile got_addr;

static void h(int sig, siginfo_t *si, void *uc) {
    (void)uc;
    got_sig = sig;
    got_code = si->si_code;
    got_addr = si->si_addr;
    siglongjmp(jb, 1);
}

static const char *signame(int s) {
    return s == SIGSEGV ? "SIGSEGV" : s == SIGBUS ? "SIGBUS" : "OTHER";
}

#define TRY(label, stmt) do {                                                      \
    got_sig = 0; got_code = -1; got_addr = (void *)0;                              \
    if (sigsetjmp(jb, 1) == 0) { stmt; printf("%-22s NO FAULT\n", label); }        \
    else printf("%-22s %-8s si_code=%d si_addr=%p\n",                              \
                label, signame(got_sig), (int)got_code, got_addr);                 \
} while (0)

int main(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = h;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS,  &sa, NULL);

    long ps = sysconf(_SC_PAGESIZE);
    printf("page size = %ld\n", ps);
    printf("SEGV_MAPERR=%d SEGV_ACCERR=%d BUS_ADRALN=%d BUS_ADRERR=%d BUS_OBJERR=%d\n",
           SEGV_MAPERR, SEGV_ACCERR, BUS_ADRALN, BUS_ADRERR, BUS_OBJERR);

    volatile char *p = mmap(NULL, ps, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (p == MAP_FAILED) { perror("mmap PROT_NONE"); return 1; }
    printf("PROT_NONE page at %p\n", (void *)p);
    TRY("PROT_NONE load",  (void)*p);
    TRY("PROT_NONE store", *p = 1);

    // Control: an unmapped address MUST still be SIGSEGV, or crashy_e2e's premise breaks.
    volatile char *u = (volatile char *)0x4000dead0000UL;
    TRY("unmapped load",   (void)*u);
    TRY("unmapped store",  *u = 1);

    // The other protection-failure flavour: writing a read-only page.
    volatile char *ro = mmap(NULL, ps, PROT_READ, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (ro == MAP_FAILED) { perror("mmap PROT_READ"); return 1; }
    TRY("PROT_READ store", *ro = 1);

    return 0;
}
```

- [ ] **Step 2: Build and run it**

Run:
```bash
cd spikes && clang -o protnone protnone.c && ./protnone
```
Expected: five result lines. No entitlement and no codesigning are needed — this touches no `hv_*` API.

**Do not proceed until every line prints a signal name.** A `NO FAULT` on `PROT_NONE load` or `PROT_NONE store` would mean the probe is wrong (the compiler elided the access), not that macOS does not fault — add `-O0` and re-run.

- [ ] **Step 3: Record the findings in `spikes/README.md`**

Append a section following the file's existing style (a `## <file>.c — <one-line purpose>` heading, then bullets stating what was verified in both directions). It MUST state, verbatim from the run:

- the `(signal, si_code)` pair for a `PROT_NONE` **load** and for a **store**;
- that an unmapped address still raises `SIGSEGV` (or, if it does not, stop and escalate — that is a finding about M6, not M13);
- the `PROT_READ` store result, noted as informational, since M13 honors only `prot == 0`.

Add the build line to the `## Build & run` block:
```sh
clang -o protnone protnone.c
./protnone
```

- [ ] **Step 4: Commit**

```bash
git add spikes/protnone.c spikes/README.md
git commit -m "M13 t1: measure Darwin's PROT_NONE signal before trusting the table"
```

---

### Task 2: Measure the guest-side landscape (R2, R4, and the retained deviation)

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:679` (the record-loop `commit_reserved_page` call site)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: three measured numbers used by later tasks and by Task 12's Status section — (a) where libstd's guard page lands relative to the guest stack backing, (b) whether `mach_vm_protect` fires in the dynamic gates and with what `prot`, (c) `commit_reserved_page` hit counts per dynamic gate.

**Why.** R2: if libstd's guard falls outside the stack backing, `protect_none`'s must-be-backed assert fires and Task 8 stalls. R4: if dyld ever protects something to `PROT_NONE`, Task 9 is a live behavior change rather than a dormant one. And the spec owes a **number** for the deviation it retains, not a hand-wave.

- [ ] **Step 1: Add the `[commit]` diagnostic**

`RETRACE_TRACE` is a record-only diagnostic (`ReplaySession` carries no trace instrumentation), so this goes at the record-loop call site only. In `crates/retrace-core/src/lib.rs`, change line 679 from:

```rust
                if b.commit_reserved_page(b.fault_ipa()) { continue; }
```

to:

```rust
                if b.commit_reserved_page(b.fault_ipa()) {
                    // M13 t2: the retained PROT_NONE-reservation deviation is quantified rather
                    // than hand-waved (see the M13 spec). Record-only, like every other
                    // RETRACE_TRACE line.
                    if trace_log { eprintln!("[commit] ipa={:#x}", b.fault_ipa()); }
                    continue;
                }
```

- [ ] **Step 2: Verify it builds and the gate is still green**

Run: `just gate`
Expected: 296 passed / 0 failed / 0 ignored, clippy clean. This step is a no-behavior-change checkpoint; if the count moved, stop.

- [ ] **Step 3: Measure R2 — where libstd's guard page lands**

Run:
```bash
cargo build -p retrace 2>/dev/null
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  "$(find target -name hello_rust -type f | head -1)" -o /tmp/m13-r2.bin 2>&1 \
  | grep -E '\[trap\] num=(197|74|-14) ' | head -40
```

`num=197` is `mmap`. libstd's `install_main_guard` appears as an `mmap` whose `args[2]` (prot) is `0x0` and whose `args[3]` (flags) has `MAP_FIXED` (`0x10`) set. Record `args[0]` — the guard VA.

Then compare it against the guest stack: `DYN_STACK_TOP` and `DYN_STACK_SIZE` in `crates/retrace-box/src/lib.rs`. Write down whether `[args[0], args[0]+args[1])` lies **wholly inside** `[DYN_STACK_TOP - DYN_STACK_SIZE, DYN_STACK_TOP)`.

**If it does not lie inside a backing, STOP and escalate.** The whole plan from Task 8 on assumes it does.

- [ ] **Step 4: Measure R4 — does `mach_vm_protect` fire, and with what `prot`?**

From the same command's output, `num=-14` is `mach_vm_protect` with `args[4]` = `prot`. Run it for each of the four dynamic gates:

```bash
for g in hello_dyn hello_rust; do
  echo "=== $g ==="
  RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
    "$(find target -name $g -type f | head -1)" -o /tmp/m13-$g.bin 2>&1 \
    | grep -c '\[trap\] num=-14 ' || true
done
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn /opt/homebrew/bin/jq -o /tmp/m13-jq.bin -- --version 2>&1 \
  | grep '\[trap\] num=-14 ' | head -20
```

Record: the call count per gate, and the distinct `args[4]` values. **If any is `0x0`, note it loudly** — Task 9 then changes live behavior and its gate run needs extra scrutiny.

- [ ] **Step 5: Measure the retained deviation — `commit_reserved_page` hits per gate**

```bash
for t in /tmp/m13-hello_dyn.bin /tmp/m13-hello_rust.bin; do :; done   # traces already written above
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  "$(find target -name hello_dyn -type f | head -1)" -o /tmp/m13-c1.bin 2>&1 | grep -c '\[commit\]'
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn \
  "$(find target -name hello_rust -type f | head -1)" -o /tmp/m13-c2.bin 2>&1 | grep -c '\[commit\]'
RETRACE_TRACE=1 cargo run -q -p retrace -- record-dyn /opt/homebrew/bin/jq -o /tmp/m13-c3.bin -- --version 2>&1 | grep -c '\[commit\]'
```

Note the M12 measurement that event counts vary across runs of the same guest (`segvy`: 258/262/263/268). **Run each count three times and record the range, not a single number** — a single number would imply a stability this project has already measured to be absent.

- [ ] **Step 6: Write the measurement report**

Create `.superpowers/sdd/2026-08-08-retrace-m13-protnone/task-2-measurements.md` (untracked by convention — `.superpowers/sdd/.gitignore` is `*`) containing all four measurements verbatim, including the command lines. Task 12 reads this file to write the Status section.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-core/src/lib.rs
git commit -m "M13 t2: quantify the retained reservation deviation, and measure the guard's address"
```

---

### Task 3: `signal_of_esr`'s permission row, per the measurement

**Files:**
- Modify: `crates/retrace-arch/src/lib.rs:307` and its golden tests near `:499`

**Interfaces:**
- Consumes: Task 1's measured `(signal, si_code)` for a `PROT_NONE` access.
- Produces: `signal_of_esr(esr) -> (u64, u64)` returning the Darwin-correct pair for DFSC `0x0c..=0x0f`. Tasks 7, 9, and 11 assert against it.

**Branch on Task 1's answer.** If the spike measured `SIGSEGV`/`SEGV_ACCERR`, the current row is already right: do Step 1 only (a comment recording that the row is now *measured* rather than assumed), skip Steps 2–4, and commit. If it measured `SIGBUS`, do all steps.

- [ ] **Step 1: Write the failing test**

In `crates/retrace-arch/src/lib.rs`, in the same test module as the existing `signal_of_esr` goldens, replace the permission-fault assertion at `:499` and add the load/store pair. Substitute Task 1's measured constants for `<SIG>` and `<CODE>`:

```rust
    // M13: the permission-fault row, MEASURED (spikes/protnone.c) rather than assumed. Every fault
    // M6/M11/M12 ever recorded was a TRANSLATION fault, so this row shipped unexercised for six
    // milestones. Darwin's ux_exception maps KERN_PROTECTION_FAILURE to <SIG>, not to what the
    // Linux-shaped table said.
    #[test]
    fn a_permission_fault_takes_the_darwin_signal() {
        // DFSC 0x0f = permission fault, level 3. Bit 6 of the ISS is WnR: 0 = load, 1 = store.
        assert_eq!(signal_of_esr(0x9200_000f), (<SIG>, <CODE>), "permission fault, store");
        assert_eq!(signal_of_esr(0x9200_004f), (<SIG>, <CODE>), "permission fault, load");
        // The control that must NOT move: an unmapped address is a TRANSLATION fault and stays
        // SIGSEGV/SEGV_MAPERR, which is what crashy_e2e and segv_rust_e2e rest on.
        assert_eq!(signal_of_esr(0x9200_0006), (SIGSEGV, SEGV_MAPERR), "translation fault, level 2");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-arch a_permission_fault_takes_the_darwin_signal -- --test-threads=1`
Expected: FAIL, with the left side showing `(11, 2)` — the old Linux-shaped row.

- [ ] **Step 3: Fix the row**

In `signal_of_esr`, replace line 307:

```rust
            0x0c..=0x0f => (SIGSEGV, SEGV_ACCERR), // permission fault
```

with (substituting the measured constants):

```rust
            // M13, MEASURED (spikes/protnone.c): Darwin's ux_exception translates EXC_BAD_ACCESS by
            // code — KERN_INVALID_ADDRESS to SIGSEGV, and everything else, including
            // KERN_PROTECTION_FAILURE, to SIGBUS. libstd's install_main_guard comment says the same
            // of its own guard page. The previous SIGSEGV here was the Linux answer and had never
            // been reached by a running guest: every fault M6/M11/M12 recorded was a TRANSLATION
            // fault (0x04..0x07), whose row is unchanged and still SIGSEGV.
            0x0c..=0x0f => (<SIG>, <CODE>),        // permission fault
```

- [ ] **Step 4: Run the whole arch suite**

Run: `cargo test -p retrace-arch -- --test-threads=1`
Expected: PASS. If any *other* golden moved, you edited the wrong row — the translation-fault, access-flag, alignment, and external-abort rows must all be untouched.

- [ ] **Step 5: Run the full gate**

Run: `just gate`
Expected: 296 passed / 0 failed / 0 ignored. `crashy_e2e` and `segv_rust_e2e` both rest on the *translation* row and must be unaffected. **If either moved, stop** — that means the permission row is reachable today by a path this plan has not accounted for.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-arch/src/lib.rs
git commit -m "M13 t3: the permission-fault row is Darwin's, and now it is measured"
```

---

### Task 4: Extract `subtract_range` and rename `set_region_exec_attr`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:738` (`set_region_exec_attr` → `set_region_attr`), `:1111` (`ipa_is_exec`), `:1856` (`subtract_reservations`)
- Test: `crates/retrace-box/tests/protnone.rs` (create)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `fn subtract_range(table: &mut Vec<(u64, u64)>, addr: u64, len: u64)` — free function in `retrace-box`, `pub(crate)`.
  - `Box_::set_region_attr(&mut self, ipa: u64, len: u64, attr: u64)` — renamed from `set_region_exec_attr`.
  - `Box_::leaf_desc(&self, ipa: u64) -> Option<u64>` — the live L2/L3 leaf descriptor, extracted from `ipa_is_exec`.

**Why now.** Task 5 needs all three. `set_region_exec_attr` is handed a *non-exec* attribute in Task 5, so its name becomes a lie; `subtract_reservations`' head/tail/split arithmetic is already correct and covered by `carveout.rs`, and Task 5 would otherwise grow a second, subtly-different copy; and `ipa_is_exec`'s walk is duplicated by Task 5's `ipa_is_noaccess`. These are refactors of code M13 edits, not speculative cleanup.

- [ ] **Step 1: Write the failing test**

Create `crates/retrace-box/tests/protnone.rs`:

```rust
// M13-protnone. The no-access protection mechanism: the range table's arithmetic, the stage-1
// stamp, the fault it produces, and the restore. Run under --test-threads=1 (one HVF VM per
// process).
use retrace_box::subtract_range_for_test as subtract_range;

// The four cases carveout.rs already pins for `reservations`, now exercised through the shared
// helper so `noaccess` cannot grow a second, subtly-different copy of them.
#[test]
fn subtract_range_trims_splits_and_removes() {
    // Disjoint: untouched.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x2000_0000, 0x4000);
    assert_eq!(t, vec![(0x1000_0000, 0x1_0000)], "a disjoint cut leaves the entry whole");

    // Head trim: the cut covers the low end.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_0000, 0x4000);
    assert_eq!(t, vec![(0x1000_4000, 0xc000)], "a head cut moves the start up");

    // Tail trim: the cut covers the high end.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_c000, 0x4000);
    assert_eq!(t, vec![(0x1000_0000, 0xc000)], "a tail cut shortens the entry");

    // Interior punch: SPLITS into two entries.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_4000, 0x4000);
    assert_eq!(t, vec![(0x1000_0000, 0x4000), (0x1000_8000, 0x8000)],
        "an interior cut splits the entry in two");

    // Full cover: the entry is removed.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x0fff_0000, 0x10_0000);
    assert!(t.is_empty(), "a covering cut removes the entry");

    // The kernel rounds the cut OUT to whole pages: start down, end up. A sub-page cut in the
    // middle of a page still removes that whole page.
    let mut t = vec![(0x1000_0000, 0x1_0000)];
    subtract_range(&mut t, 0x1000_4001, 1);
    assert_eq!(t, vec![(0x1000_0000, 0x4000), (0x1000_8000, 0x8000)],
        "a sub-page cut is rounded out to whole pages");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test protnone -- --test-threads=1`
Expected: FAIL to compile — `subtract_range_for_test` does not exist.

- [ ] **Step 3: Extract `subtract_range`**

In `crates/retrace-box/src/lib.rs`, add the free function near `subtract_reservations` (`:1856`):

```rust
/// Subtract `[addr, addr+len)` from every overlapping extent in a page-granular `(start, len)`
/// table, rounding the cut OUT to whole pages the way the kernel does (start down, end up). A full
/// cover removes the entry; a head/tail overlap trims it; a strictly-interior punch SPLITS it into
/// two. Pure: identical input produces identical output on record and replay.
///
/// Shared by `reservations` (M2-carveout's hole punch) and `noaccess` (M13's protection map) so the
/// second table cannot drift from the arithmetic `carveout.rs` already pins for the first.
pub(crate) fn subtract_range(table: &mut Vec<(u64, u64)>, addr: u64, len: u64) {
    let g = GRANULE as u64;
    let s = addr & !(g - 1);                                 // trunc_page(addr)
    let e = (addr.saturating_add(len) + g - 1) & !(g - 1);   // round_page(addr + len)
    if e <= s { return; }
    let mut out = Vec::with_capacity(table.len() + 1);
    for &(start, rlen) in table.iter() {
        let end = start + rlen;
        if e <= start || s >= end {
            out.push((start, rlen));                         // disjoint: keep whole
            continue;
        }
        if s > start { out.push((start, s - start)); }       // head remnant below the cut
        if e < end   { out.push((e, end - e)); }             // tail remnant above the cut
        // [s, e) fully covers [start, end): push nothing (entry removed)
    }
    *table = out;
}

/// Test-only re-export of [`subtract_range`], so the integration test can exercise the shared
/// arithmetic directly without making it part of the crate's public surface.
#[doc(hidden)]
pub fn subtract_range_for_test(table: &mut Vec<(u64, u64)>, addr: u64, len: u64) {
    subtract_range(table, addr, len)
}
```

Then replace `subtract_reservations`' body (`:1856`) with a one-line caller, keeping its doc comment:

```rust
    fn subtract_reservations(&mut self, addr: u64, len: u64) {
        subtract_range(&mut self.reservations, addr, len);
    }
```

- [ ] **Step 4: Run the test and the carveout regression**

Run: `cargo test -p retrace-box --test protnone --test carveout -- --test-threads=1`
Expected: PASS both. `carveout.rs` is the proof the extraction changed no behavior — **it must pass unmodified.**

- [ ] **Step 5: Rename `set_region_exec_attr` → `set_region_attr`**

Rename the method at `:738` and update its two call sites (`set_region_exec` at `:727`, `ensure_tlbi_stub` at `:1258`) plus the three assert messages inside it that name it. Update its doc comment's first line to drop the exec framing:

```rust
    /// The single stamp implementation shared by `set_region_exec` (`ATTR_CODE`, guest code / cache
    /// text), `ensure_tlbi_stub` (`ATTR_TRAMP`, the M9 TLBI stub — an EL1-exec page), and
    /// `protect_none`/`unprotect` (M13, `ATTR_NONE`/`ATTR_DATA` — NOT exec attributes, which is why
    /// this is no longer named for exec). Edits the LIVE page tables to install `attr` for
```

Find every remaining reference: `rg -n 'set_region_exec_attr' crates/` must return nothing.

- [ ] **Step 6: Extract `leaf_desc` from `ipa_is_exec`**

Replace `ipa_is_exec` (`:1111`) with:

```rust
    /// The live stage-1 leaf descriptor for `ipa`, walking the L2/L3 the box maintains: an L3 page
    /// descriptor where the block has been promoted, otherwise the L2 block descriptor itself.
    /// `None` when there is no live L2, the IPA is outside the 36-bit space, or the promoted L3 is
    /// not among the tracked backings. Shared by the attribute observables below.
    fn leaf_desc(&self, ipa: u64) -> Option<u64> {
        if self.l2_host.is_null() { return None; }
        let bi = (ipa / BLK) as usize;
        if bi >= 2048 { return None; }
        let l2 = unsafe { std::slice::from_raw_parts(self.l2_host as *const u64, 2048) };
        if l2[bi] & 0x3 == DESC_TABLE {
            let l3_ipa = l2[bi] & !(GRANULE as u64 - 1);
            let host = self.backings.iter().find(|b| b.ipa == l3_ipa).map(|b| b.host)?;
            let l3 = unsafe { std::slice::from_raw_parts(host as *const u64, 2048) };
            let idx = ((ipa - bi as u64 * BLK) / GRANULE as u64) as usize;
            Some(l3[idx])
        } else {
            Some(l2[bi]) // block descriptor: identity data block, never executable
        }
    }

    /// Test/diagnostic observable: is the stage-1 leaf mapping for `ipa` executable at EL0
    /// (`ATTR_CODE`: UXN clear)? A default data block (or a promoted `ATTR_DATA` page) is
    /// non-exec; only a `set_region_exec` page (guest code, the sign stub, or a paged-in cache
    /// TEXT page) is.
    pub fn ipa_is_exec(&self, ipa: u64) -> bool {
        let Some(leaf) = self.leaf_desc(ipa) else { return false };
        leaf & 0x3 != 0 && leaf & UXN == 0
    }
```

- [ ] **Step 7: Run the full gate**

Run: `just gate`
Expected: **296 passed + 1 new = 297 passed / 0 failed / 0 ignored**, clippy clean. This task is behavior-preserving; any other movement is a bug you just introduced.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/protnone.rs
git commit -m "M13 t4: share the range-subtract, and stop calling a generic stamp 'exec'"
```

---

### Task 5: `ATTR_NONE`, the `noaccess` map, and `protect_none`/`unprotect`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — attribute constants (`:326-334`), the `Box_` struct (`:354` area), `protect_none`/`unprotect`/`noaccess`/`ipa_is_noaccess` (add near `commit_reserved_page`, `:1081`), the three landmark-0 constructors (`:908`, `:1397`, `:2176`)
- Test: `crates/retrace-box/tests/protnone.rs`

**Interfaces:**
- Consumes: `subtract_range`, `set_region_attr`, `leaf_desc` (Task 4); `flush_guest_tlb` (existing, `:1234`).
- Produces:
  - `const ATTR_NONE: u64` (private to `retrace-box`).
  - `Box_::protect_none(&mut self, ipa: u64, len: u64)`
  - `Box_::unprotect(&mut self, ipa: u64, len: u64)`
  - `Box_::noaccess(&self) -> &[(u64, u64)]`
  - `Box_::ipa_is_noaccess(&self, ipa: u64) -> bool`

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/protnone.rs`:

```rust
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

// The stamp round-trips: a backed page goes no-access and comes back, and both the live page-table
// leaf and the tracked map agree at every step. This is the mechanism with no guest and no fault
// in the way.
#[test]
fn protect_none_stamps_the_leaf_and_tracks_the_range() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);

    // A page that is genuinely backed: reserve, then commit one page (the M2-mmapcommit path).
    let base = b.guest_vm_reserve(0, 0x10000, true);
    assert!(b.commit_reserved_page(base), "the page under test must be backed");

    assert!(!b.ipa_is_noaccess(base), "a freshly committed page is ordinary RW data");
    assert!(b.noaccess().is_empty(), "nothing is protected yet");

    b.protect_none(base, 0x4000);
    assert!(b.ipa_is_noaccess(base), "the leaf must deny EL0 after protect_none");
    assert_eq!(b.noaccess(), &[(base, 0x4000)], "the extent must be tracked");

    // Its neighbour inside the same reservation is untouched: the stamp is per-page.
    assert!(!b.ipa_is_noaccess(base + 0x4000), "protection must not leak to the next page");

    b.unprotect(base, 0x4000);
    assert!(!b.ipa_is_noaccess(base), "unprotect must restore EL0 access");
    assert!(b.noaccess().is_empty(), "the extent must be dropped from the map");
}

// The M13-split invariant: no-access implies backed. Protecting a page with no backing would leave
// its fault at stage 2, where commit_reserved_page would silently materialize it — the exact
// silent-wrong-answer this milestone exists to remove. It must fail loud instead.
#[test]
#[should_panic(expected = "protect_none: no backing")]
fn protect_none_refuses_an_unbacked_page() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let base = b.guest_vm_reserve(0, 0x10000, true);  // reserved, deliberately NOT committed
    b.protect_none(base, 0x4000);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test protnone -- --test-threads=1`
Expected: FAIL to compile — `protect_none`, `unprotect`, `noaccess`, `ipa_is_noaccess` do not exist.

- [ ] **Step 3: Add `ATTR_NONE`**

In `crates/retrace-box/src/lib.rs`, after `ATTR_TRAMP` (`:331`):

```rust
// M13: EL0 gets nothing (AP 0b00 leaves EL1 RW and EL0 with no access at all), and the page is
// non-executable at both ELs. `A_COMMON` sets AF, so an EL0 access takes a clean PERMISSION fault
// (DFSC 0x0f) rather than an access-flag fault — which is what routes it through the EL1 trampoline
// to `Stop::Fault` and M12's disposition check. Marking the descriptor INVALID instead would give a
// translation fault, making a protected page indistinguishable from an unmapped one; that
// distinction is what lets `commit_reserved_page` keep its stage-2 cases (see the M13 spec).
const ATTR_NONE: u64 = A_COMMON | 0x00 /*AP EL1-RW, EL0 none*/ | UXN | PXN;
```

- [ ] **Step 4: Add the `noaccess` field**

In the `Box_` struct, immediately after the `reservations` field (`:349-354` area), matching its comment style:

```rust
    // M13: page-granular extents the guest has protected PROT_NONE, recorded by `protect_none` and
    // subtracted by `unprotect`/`guest_munmap`. Same shape and same treatment as `reservations`
    // above — a pure function of the guest's own mmap/mprotect sequence, so identical on record and
    // replay. INVARIANT: every page named here is backed (see `protect_none`).
    noaccess: Vec<(u64, u64)>,
```

Add `noaccess: Vec::new(),` to each of the three landmark-0 constructors (`:908`, `:1397`, `:2176`) beside their existing `reservations: Vec::new(),`.

- [ ] **Step 5: Add the four methods**

Insert after `commit_reserved_page` (`:1100`):

```rust
    /// Make `[ipa, ipa+len)` inaccessible to EL0 — the guest asked for `PROT_NONE`, so the page
    /// tables say so and the hardware enforces it. Page-granular: the range is rounded out to whole
    /// pages, since a stage-1 leaf is the finest thing that can carry an attribute.
    ///
    /// **Requires every page in the range to be backed, and asserts otherwise.** That is M13's
    /// central invariant, and it is what makes the fault unambiguous: a protected page is backed, so
    /// an EL0 access takes a stage-1 PERMISSION fault via the EL1 trampoline and arrives as
    /// `Stop::Fault` for M12's disposition check; a reserved-but-uncommitted page is unbacked, so its
    /// access takes a stage-2 TRANSLATION fault direct to EL2 and arrives as `Stop::Other` for
    /// `commit_reserved_page`. Two exception routes, two `Stop` variants — the hardware separates
    /// "reserved and committable" from "protected, must fault", and no software gate has to.
    ///
    /// **The TLBI is a correctness requirement here, not a precaution.** Every other caller of
    /// `set_region_attr` stamps a fresh IPA the guest has never translated — the sign stub, the TLBI
    /// stub, a cache page, a fresh exec mmap — and each documents that as its soundness argument.
    /// This one stamps a page the guest is actively using (libstd's guard page lives inside the stack
    /// it is running on). A missing flush leaves a stale PERMISSIVE entry, so the guard silently
    /// fails to guard: the precise class of quiet wrong answer this milestone exists to remove.
    ///
    /// Deterministic and trace-free: a pure function of the guest's own protection calls, which are
    /// already recorded syscalls that replay re-dispatches through this same method.
    pub fn protect_none(&mut self, ipa: u64, len: u64) {
        let g = GRANULE as u64;
        let start = ipa & !(g - 1);
        let end = (ipa.saturating_add(len) + g - 1) & !(g - 1);
        if end <= start { return; }
        let mut p = start;
        while p < end {
            assert!(self.host_span(p).is_some(),
                "protect_none: no backing for {p:#x} (in [{start:#x},{end:#x})). M13 models \
                 no-access only on BACKED pages: an unbacked protected page would fault at stage 2, \
                 where commit_reserved_page would silently materialize it instead of faulting. \
                 Model the reservation-protect case deliberately before a guest needs it.");
            p += g;
        }
        self.set_region_attr(start, end - start, ATTR_NONE);
        self.noaccess.push((start, end - start));
        // The guest may already hold a translation for these pages — unlike every other stamp site.
        self.flush_guest_tlb();
    }

    /// Restore ordinary EL0 read/write access to `[ipa, ipa+len)` and drop it from the protection
    /// map. The mirror of [`protect_none`](Self::protect_none), and it needs the same flush for the
    /// same reason in the opposite direction: a stale RESTRICTIVE entry would keep faulting on a page
    /// the guest has legitimately unprotected.
    ///
    /// Restores `ATTR_DATA` unconditionally rather than whatever the page held before it was
    /// protected. Today those are the same thing — only `ATTR_DATA` pages are ever protected, since
    /// code pages are read-only and no guest protects them — and the choice is documented rather than
    /// incidental: a general protection map would have to remember the prior attribute instead.
    pub fn unprotect(&mut self, ipa: u64, len: u64) {
        let g = GRANULE as u64;
        let start = ipa & !(g - 1);
        let end = (ipa.saturating_add(len) + g - 1) & !(g - 1);
        if end <= start { return; }
        self.set_region_attr(start, end - start, ATTR_DATA);
        subtract_range(&mut self.noaccess, start, end - start);
        self.flush_guest_tlb();
    }

    /// The tracked no-access extents as `(start, len)` (test/diagnostic observability, the twin of
    /// [`reservations`](Self::reservations)).
    pub fn noaccess(&self) -> &[(u64, u64)] { &self.noaccess }

    /// Test/diagnostic observable: does the stage-1 leaf for `ipa` deny EL0 all access (`ATTR_NONE`:
    /// AP bits `0b00`)? The AP field uniquely identifies the four attributes this box installs —
    /// `ATTR_NONE` `0x00`, `ATTR_DATA` `0x40`, `ATTR_TRAMP` `0x80`, `ATTR_CODE` `0xC0` — so testing
    /// it is exact rather than heuristic.
    pub fn ipa_is_noaccess(&self, ipa: u64) -> bool {
        let Some(leaf) = self.leaf_desc(ipa) else { return false };
        leaf & 0x3 != 0 && leaf & 0xC0 == 0x00
    }
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p retrace-box --test protnone -- --test-threads=1`
Expected: PASS, both new tests.

- [ ] **Step 7: Run the full gate**

Run: `just gate`
Expected: 299 passed / 0 failed / 0 ignored, clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/protnone.rs
git commit -m "M13 t5: ATTR_NONE, and the flush that makes revocation real"
```

---

### Task 6: `BoxState` carries the protection map

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:553` (`BoxState`), `:2791` (`checkpoint`), `:2861` (`from_checkpoint`)
- Test: `crates/retrace-box/tests/protnone.rs`

**Interfaces:**
- Consumes: `protect_none`, `noaccess`, `ipa_is_noaccess` (Task 5).
- Produces: `BoxState.noaccess: Vec<(u64, u64)>`, carried through `checkpoint`/`from_checkpoint`. M4's `checkpointed_seek` and M3's `ReplaySession` consume it implicitly.

**What is already free, and what is not.** `from_checkpoint` re-maps every backing out of `state.mem`, and the L2/L3 page tables *are* backings at fixed IPAs — M9's comment at `:2857` already relies on exactly this for `ATTR_TRAMP`. So the **stamps** survive a checkpoint with no work. What does not survive is the **map**, which `unprotect` and the fail-loud asserts consult; restoring tables without it leaves the two out of sync. `from_checkpoint` also builds a fresh `Vm`/`Vcpu`, so no restored session can hold a stale TLB entry — that risk is confined to in-run changes, which Task 5's flush covers.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/protnone.rs`:

```rust
// A seeked or checkpointed session must agree with the run it came from about what is protected.
// The page-table STAMP rides along for free (the tables are backings, captured in `mem`); the MAP
// does not, and without it `unprotect` and the fail-loud asserts would disagree with the hardware.
#[test]
fn a_checkpoint_carries_both_the_stamp_and_the_map() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let base = b.guest_vm_reserve(0, 0x10000, true);
    assert!(b.commit_reserved_page(base));
    b.protect_none(base, 0x4000);

    let st = b.checkpoint();
    assert_eq!(st.noaccess, vec![(base, 0x4000)], "the map must be captured");
    drop(b); // one VM per process: the original must go before the restored one is built

    let b2 = Box_::from_checkpoint(&st);
    assert!(b2.ipa_is_noaccess(base),
        "the stage-1 stamp rides in `mem` with the page tables and must survive the restore");
    assert_eq!(b2.noaccess(), &[(base, 0x4000)],
        "the map must survive too, or unprotect and the hardware disagree");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test protnone a_checkpoint_carries -- --test-threads=1`
Expected: FAIL to compile — `BoxState` has no `noaccess` field.

- [ ] **Step 3: Add the field to `BoxState`**

After `pub reservations: Vec<(u64, u64)>,` (`:553`):

```rust
    // M13: carried for the same reason as `reservations` — a mid-run capture cannot re-derive it.
    // The page-table STAMPS ride along inside `mem` (the L2/L3 tables are backings, exactly as M9's
    // ATTR_TRAMP note at from_checkpoint explains), but the MAP that `unprotect` and the fail-loud
    // asserts consult is box state, and a restore without it would leave the two disagreeing.
    pub noaccess: Vec<(u64, u64)>,
```

- [ ] **Step 4: Carry it through `checkpoint` and `from_checkpoint`**

In `checkpoint` (`:2791`), beside `reservations: self.reservations.clone(),`:
```rust
            noaccess: self.noaccess.clone(),
```

In `from_checkpoint` (`:2861`), beside `reservations: state.reservations.clone(),`:
```rust
            noaccess: state.noaccess.clone(),
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p retrace-box --test protnone -- --test-threads=1`
Expected: PASS, all four tests.

- [ ] **Step 6: Run the full gate**

Run: `just gate`
Expected: 300 passed / 0 failed / 0 ignored, clippy clean. The M4 checkpoint suite must be unaffected.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/protnone.rs
git commit -m "M13 t6: a seeked session agrees with its run about what is protected"
```

---

### Task 7: The mechanism gates — `protnone.s` and `protrestore.s` through `mprotect`

**Files:**
- Create: `crates/retrace-guest/asm/protnone.s`, `crates/retrace-guest/asm/protrestore.s`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`, `crates/retrace-box/src/lib.rs:1894` (`guest_mprotect`)
- Test: `crates/retrace-box/tests/protnone.rs`

**Interfaces:**
- Consumes: `protect_none`, `unprotect` (Task 5); the measured signal (Task 3).
- Produces: `retrace_guest::PROTNONE`, `retrace_guest::PROTRESTORE` (`&str` paths); `Box_::guest_mprotect` honoring `prot == 0`.

**The ordering is the test, and it is not the ordering the spec sketched.** Both guests **touch the page before protecting it**, so the page is definitely in the TLB when its attribute changes. Without `protect_none`'s flush, `protnone` sees its post-protect store *succeed* through a stale permissive entry and the test fails; without `unprotect`'s flush, `protrestore` sees its post-restore store *fault* through a stale restrictive entry and the test fails. A guest that protected a never-touched page would pass either way — vacuously.

- [ ] **Step 1: Write the guests**

Create `crates/retrace-guest/asm/protnone.s`:

```asm
// M13 t7. mprotect(PROT_NONE) must actually deny access — and the TLBI must actually invalidate.
//
// The pre-protect store is load-bearing: it puts a WRITABLE translation for the page in the TLB.
// If protect_none stamps ATTR_NONE without flushing, the second store hits that stale entry and
// SUCCEEDS, and this guest exits 0 instead of faulting. A guest that protected a never-touched page
// would pass with or without the flush — vacuously. That is why the touch comes first.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // p = mmap(NULL, 0x4000, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANON, -1, 0)
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3                 // PROT_READ|PROT_WRITE
    mov  x3, #0x1002            // MAP_PRIVATE|MAP_ANON
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197              // SYS_mmap
    svc  #0x80
    mov  x19, x0                // keep the address

    // Touch it: this is what populates the TLB with a writable entry.
    mov  x9, #0xAAAA
    str  x9, [x19]

    // mprotect(p, 0x4000, PROT_NONE)
    mov  x0, x19
    mov  x1, #0x4000
    mov  x2, #0                 // PROT_NONE
    mov  x16, #74               // SYS_mprotect
    svc  #0x80

    // The store under test. It MUST fault; reaching the exit below is the failure mode.
    mov  x9, #0xBBBB
    str  x9, [x19]              // <-- must take a stage-1 permission fault

    mov  x0, #7                 // "protection was not enforced" — never reached when M13 works
    mov  x16, #1                // SYS_exit
    svc  #0x80
```

Create `crates/retrace-guest/asm/protrestore.s`:

```asm
// M13 t7. The restore direction: a page returned from PROT_NONE to RW must be usable again.
//
// The pre-protect store puts a writable entry in the TLB, and the protect replaces it. If unprotect
// stamps ATTR_DATA without flushing, the post-restore store hits the stale RESTRICTIVE entry and
// faults, and this guest dies instead of exiting 0. So both directions of the flush are covered by
// the pair, and neither can pass vacuously.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3                 // PROT_READ|PROT_WRITE
    mov  x3, #0x1002            // MAP_PRIVATE|MAP_ANON
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197              // SYS_mmap
    svc  #0x80
    mov  x19, x0

    mov  x9, #0xAAAA
    str  x9, [x19]              // touch: populate the TLB

    mov  x0, x19                // mprotect(p, 0x4000, PROT_NONE)
    mov  x1, #0x4000
    mov  x2, #0
    mov  x16, #74
    svc  #0x80

    mov  x0, x19                // mprotect(p, 0x4000, PROT_READ|PROT_WRITE)
    mov  x1, #0x4000
    mov  x2, #3
    mov  x16, #74
    svc  #0x80

    mov  x9, #0xBBBB
    str  x9, [x19]              // must SUCCEED: a stale restrictive entry would fault here
    ldr  x10, [x19]
    cmp  x9, x10
    b.ne fail

    mov  x0, #0                 // exit 0: protected, restored, and usable
    mov  x16, #1
    svc  #0x80
fail:
    mov  x0, #9                 // the value did not survive the round trip
    mov  x16, #1
    svc  #0x80
```

- [ ] **Step 2: Register them in `build.rs`**

Append to `crates/retrace-guest/build.rs`, following the `reservecommit` block's exact shape:

```rust
    // protnone: touches an mmap'd RW page (populating the TLB), mprotects it PROT_NONE, then stores
    // again — which must take a stage-1 PERMISSION fault. The M13 t7 mechanism guest; the pre-touch
    // is what makes protect_none's TLBI load-bearing rather than decorative.
    let src = format!("{}/asm/protnone.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/protnone");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang protnone");
    assert!(status.success(), "protnone guest build failed");

    // protrestore: the same page protected PROT_NONE and then returned to RW, proving unprotect's
    // flush too — a stale restrictive entry would fault the post-restore store. Exits 0 on success.
    let src = format!("{}/asm/protrestore.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/protrestore");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang protrestore");
    assert!(status.success(), "protrestore guest build failed");
```

Add to `crates/retrace-guest/src/lib.rs` beside the other guest paths:

```rust
pub const PROTNONE: &str = concat!(env!("OUT_DIR"), "/protnone");
pub const PROTRESTORE: &str = concat!(env!("OUT_DIR"), "/protrestore");
```

- [ ] **Step 3: Write the failing test**

Append to `crates/retrace-box/tests/protnone.rs` (substituting Task 1's measured `<SIG>`):

```rust
use retrace_box::Stop;
use retrace_guest::{PROTNONE, PROTRESTORE};

// A real guest, a real mprotect, a real fault. The fault must arrive as Stop::Fault — the stage-1
// route through the EL1 trampoline that M12's disposition check consults — and NOT as Stop::Other,
// which is the stage-2 route commit_reserved_page owns. That classification is the whole of M13's
// "the hardware separates them" claim, so it is asserted rather than assumed.
#[test]
fn a_protected_page_faults_on_the_stage_one_route() {
    let loaded = parse_macho(&std::fs::read(PROTNONE).unwrap());
    let mut b = Box_::load(&loaded);
    let mut protected = 0u64;
    loop {
        match b.run() {
            // The guest's mmap and mprotect are ordinary syscalls; drive them through the box the
            // way the record loop does, so this test needs no recorder.
            Stop::Syscall { num, args } if num == 197 => {
                let ipa = b.guest_mmap(args[0], args[1], args[2], args[3]).expect("anon mmap");
                b.set_x0_err_and_return(ipa, false);
            }
            Stop::Syscall { num, args } if num == 74 => {
                protected = args[0];
                b.guest_mprotect(args[0], args[1], args[2]);
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == 1 => {
                panic!("the guest exited {} — the protected store was NOT denied, which is what a \
                        missing TLBI looks like", args[0]);
            }
            Stop::Fault { esr, far, .. } => {
                assert_eq!(far & !0x3fff, protected,
                    "the fault must be at the protected page {protected:#x}, got {far:#x}");
                assert_eq!(esr & 0x3f, 0x0f,
                    "DFSC must be 0x0f (permission fault, level 3), got {:#x} — a translation \
                     fault here would mean the descriptor was invalidated rather than AP-denied",
                    esr & 0x3f);
                assert_eq!(retrace_arch::signal_of_esr(esr).0, <SIG>,
                    "the protected fault must map to the signal Darwin raises (spikes/protnone.c)");
                return;
            }
            Stop::Other { esr } => panic!(
                "a protected page must fault at STAGE 1 (Stop::Fault), not stage 2 (Stop::Other, \
                 esr={esr:#x}) — stage 2 is commit_reserved_page's route and would silently \
                 materialize the page"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}

// The restore direction, and the other half of the TLBI proof: after unprotect, the guest's store
// must succeed and the value must read back. A stale restrictive entry faults here instead.
#[test]
fn an_unprotected_page_is_usable_again() {
    let loaded = parse_macho(&std::fs::read(PROTRESTORE).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == 197 => {
                let ipa = b.guest_mmap(args[0], args[1], args[2], args[3]).expect("anon mmap");
                b.set_x0_err_and_return(ipa, false);
            }
            Stop::Syscall { num, args } if num == 74 => {
                b.guest_mprotect(args[0], args[1], args[2]);
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == 1 => {
                assert_eq!(args[0], 0,
                    "exit {} — 9 means the value did not survive the protect/unprotect round trip",
                    args[0]);
                return;
            }
            Stop::Fault { esr, far, .. } => panic!(
                "the post-restore store must NOT fault (esr={esr:#x} far={far:#x}) — a stale \
                 restrictive TLB entry is what this looks like"),
            Stop::Other { esr } => panic!("unexpected stage-2 abort esr={esr:#x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p retrace-box --test protnone a_protected_page -- --test-threads=1`
Expected: FAIL with "the guest exited 7 — the protected store was NOT denied". That message *is* today's bug: `guest_mprotect` discards `prot`.

- [ ] **Step 5: Honor `prot == 0` in `guest_mprotect`**

Replace `guest_mprotect` (`:1894`) with:

```rust
    /// Honor `mprotect`. **Only `prot == 0` is modelled**, and that is a decision rather than an
    /// omission: dyld issues `mach_vm_protect` RW→RO during fixups and then writes through the
    /// result, so honoring the read-only bit would break the loader. No-access is what a guard page
    /// needs and is the only protection change with no blast radius on the dynamic gates. Every
    /// other `prot` keeps the pre-M13 behavior — a best-effort stage-2 re-protect to `RWX`, since
    /// our security boundary is the VMM and stage-1 W^X is already correct.
    ///
    /// A non-zero `prot` over a range that is currently no-access RESTORES it (`unprotect`), which
    /// is how a guest takes its guard page back. Shared by the `mprotect`(74) and
    /// `mach_vm_protect`(−14) dispatch arms on both record and replay, so there is one
    /// implementation and no mirror to keep in step.
    pub fn guest_mprotect(&mut self, ipa: u64, len: u64, prot: u64) {
        if prot == 0 {
            self.protect_none(ipa, len);
            return;
        }
        let end = ipa.saturating_add(len);
        if self.noaccess.iter().any(|&(s, l)| ipa < s + l && s < end) {
            self.unprotect(ipa, len);
            return;
        }
        let _ = self.vm.protect(ipa, len as usize, MemFlags::RWX);
    }
```

- [ ] **Step 6: Run both tests to verify they pass**

Run: `cargo test -p retrace-box --test protnone -- --test-threads=1`
Expected: PASS, all six tests.

- [ ] **Step 7: Run the full gate**

Run: `just gate`
Expected: 302 passed / 0 failed / 0 ignored, clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/retrace-guest/asm/protnone.s crates/retrace-guest/asm/protrestore.s \
        crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs \
        crates/retrace-box/src/lib.rs crates/retrace-box/tests/protnone.rs
git commit -m "M13 t7: mprotect denies for real, and the pre-touch makes the TLBI load-bearing"
```

---

### Task 8: The headline path — `map_mmap_region` honors `prot == 0`

**Files:**
- Modify: `crates/retrace-box/src/lib.rs:1774-1803` (`map_mmap_region`), `:1879` (`guest_munmap`)
- Test: `crates/retrace-box/tests/protnone.rs`

**Interfaces:**
- Consumes: `protect_none` (Task 5), `subtract_range` (Task 4).
- Produces: `map_mmap_region` protecting on `prot == 0` at **both** exits; `guest_munmap` dropping the range from `noaccess`.

**This is the libstd path.** `install_main_guard` mmaps `PROT_NONE MAP_FIXED` at `usrstack64 - RLIMIT_STACK`, which lands **wholly inside** the dynamic-stack backing — `map_mmap_region`'s "fully contained" case, which returns early at `:1794` via `place_fixed`. A hook added only at the normal fall-through would miss it entirely and this task would appear to work while the headline gate stayed broken.

- [ ] **Step 1: Write the failing test**

Append to `crates/retrace-box/tests/protnone.rs`:

```rust
// libstd's install_main_guard in miniature: a PROT_NONE MAP_FIXED mmap landing WHOLLY INSIDE an
// existing backing. That is map_mmap_region's "fully contained" case, which returns early through
// place_fixed — so a hook placed only at the normal exit would miss the one path that matters.
#[test]
fn a_fixed_prot_none_mmap_inside_a_backing_protects_it() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);

    // A backing to sit inside: 4 pages, mapped RW at a fresh address.
    let region = b.guest_mmap(0, 0x10000, 3, 0x1002).expect("anon mmap");
    assert!(!b.ipa_is_noaccess(region + 0x4000), "the backing starts fully accessible");

    // The guard: one page, FIXED, PROT_NONE, strictly inside it.
    let guard = region + 0x4000;
    let got = b.guest_mmap(guard, 0x4000, 0, 0x1012).expect("fixed PROT_NONE mmap");
    assert_eq!(got, guard, "a FIXED mmap is honored at the requested address");
    assert!(b.ipa_is_noaccess(guard), "the guard page must deny EL0 — this is the contained path");
    assert_eq!(b.noaccess(), &[(guard, 0x4000)], "and be tracked");

    // Its neighbours inside the same backing are untouched.
    assert!(!b.ipa_is_noaccess(region), "the page below the guard stays accessible");
    assert!(!b.ipa_is_noaccess(region + 0x8000), "the page above it stays accessible");
}

// Unmapping a protected range must drop it from the map, or the next thing mapped at that address
// inherits a protection its guest never asked for.
#[test]
fn munmap_drops_the_protection_with_the_pages() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let region = b.guest_mmap(0, 0x10000, 3, 0x1002).expect("anon mmap");
    let guard = region + 0x4000;
    b.guest_mmap(guard, 0x4000, 0, 0x1012).expect("fixed PROT_NONE mmap");
    assert_eq!(b.noaccess(), &[(guard, 0x4000)]);

    b.guest_munmap(guard, 0x4000);
    assert!(b.noaccess().is_empty(),
        "an unmapped range must leave the protection map, or the next mapping there inherits it");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retrace-box --test protnone a_fixed_prot_none -- --test-threads=1`
Expected: FAIL — "the guard page must deny EL0". `map_mmap_region` consults `prot` only for `PROT_EXEC`.

- [ ] **Step 3: Hook both exits of `map_mmap_region`**

Replace the FIXED branch and the tail of `map_mmap_region` (`:1779-1802`) so both exits protect. Note `place_fixed`'s early return must protect **before** returning:

```rust
        let ipa = if flags & Self::MAP_FIXED != 0 {
            if !Self::fixed_fits(addr, rlen) {
                // SAFETY: `host` is this function's freshly-allocated `rlen`-byte mapping, never
                // published to `backings` or the guest, and unreachable after this return.
                unsafe { libc::munmap(host as *mut _, rlen); }
                return Err(retrace_arch::EINVAL);
            }
            // Classify the overlap (shared with guest_vm_map's FIXED branch). A contained request
            // reuses the existing backing and is already complete.
            //
            // M13: this is the exit libstd's install_main_guard takes — its PROT_NONE MAP_FIXED
            // guard page lands wholly inside the stack backing — so the protection hook MUST be
            // here as well as at the tail. A hook only at the tail would miss the headline path.
            if let Some(a) = self.place_fixed(host, rlen, addr, prot & Self::PROT_EXEC != 0) {
                if prot == 0 { self.protect_none(a, rlen as u64); }
                return Ok(a);
            }
            addr
        } else { self.mmap_next };
        self.vm.map(host, ipa, rlen, MemFlags::RWX).expect("hv_vm_map (mmap region)");
        self.backings.push(Backing { host, ipa, len: rlen });
        if flags & Self::MAP_FIXED == 0 { self.mmap_next += rlen as u64; }
        // M13: a PROT_NONE mmap is protected once its backing exists — which is why the invariant
        // "no-access => backed" costs nothing on this path.
        if prot == 0 { self.protect_none(ipa, rlen as u64); }
        Ok(ipa)
```

- [ ] **Step 4: Drop protections in `guest_munmap`**

In `guest_munmap` (`:1879`), after `self.subtract_reservations(ipa, len);`:

```rust
        // M13: the pages are gone, so the protection goes with them — otherwise the next mapping at
        // this address inherits a no-access extent its guest never asked for.
        subtract_range(&mut self.noaccess, ipa, len);
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p retrace-box --test protnone -- --test-threads=1`
Expected: PASS, all eight tests.

- [ ] **Step 6: Run the full gate**

Run: `just gate`
Expected: 304 passed / 0 failed / 0 ignored, clippy clean.

**This is the first task that can move a dynamic gate**, because `hello_rust`, `jq`, and `jq_file` all run libstd's `install_main_guard`. If one breaks here, the likely cause is that libstd's guard is *not* wholly contained (Task 2 Step 3 should have caught this) or that some other `prot == 0` mmap exists that Task 2 did not surface. Diagnose with `RETRACE_TRACE=1` before changing the mechanism.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/protnone.rs
git commit -m "M13 t8: the guard page libstd asked for is the guard page it gets"
```

---

### Task 9: Route `mach_vm_protect` into the box

**Files:**
- Modify: `crates/retrace-core/src/lib.rs:348` (record arm), `:1123` (replay arm)
- Create: `crates/retrace-guest/asm/protnone_mach.s`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Test: `crates/retrace-box/tests/protnone.rs`

**Interfaces:**
- Consumes: `guest_mprotect` (Task 7).
- Produces: `retrace_guest::PROTNONE_MACH`; both `mach_vm_protect` arms calling `guest_mprotect(args[1], args[2], args[4])`.

**Arg layout**, from the constant's own comment at `retrace-core/src/lib.rs:26`:
`_kernelrpc_mach_vm_protect_trap(target, addr, size, setmax, prot)` — so `addr = args[1]`, `size = args[2]`, `prot = args[4]`. `args[3]` (`set_maximum`) is ignored: M13 models current protection only, and a maximum-protection change alters nothing the box enforces.

**Check Task 2 Step 4 before starting.** If any dynamic gate issues `mach_vm_protect` with `prot == 0` today, this task changes live behavior and its gate run needs the same scrutiny Task 8 got.

- [ ] **Step 1: Write the guest**

Create `crates/retrace-guest/asm/protnone_mach.s`:

```asm
// M13 t9. The same protection, through mach_vm_protect (svc -14) instead of mprotect (74). A
// separate dispatch arm that would otherwise be covered by nothing: before M13 it returned
// KERN_SUCCESS without calling into the box at all.
//
// _kernelrpc_mach_vm_protect_trap(target, addr, size, set_maximum, new_protection).
// As with protnone.s, the pre-protect store is what makes the TLBI load-bearing.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    mov  x0, #0
    mov  x1, #0x4000
    mov  x2, #3                 // PROT_READ|PROT_WRITE
    mov  x3, #0x1002            // MAP_PRIVATE|MAP_ANON
    mov  x4, #-1
    mov  x5, #0
    mov  x16, #197              // SYS_mmap
    svc  #0x80
    mov  x19, x0

    mov  x9, #0xAAAA
    str  x9, [x19]              // touch: populate the TLB

    // mach_vm_protect(task, addr=x19, size=0x4000, set_maximum=0, new_protection=0)
    mov  x0, #0                 // target task (ignored by retrace's arm)
    mov  x1, x19                // addr
    mov  x2, #0x4000            // size
    mov  x3, #0                 // set_maximum = FALSE
    mov  x4, #0                 // new_protection = PROT_NONE
    mov  x16, #-14              // _kernelrpc_mach_vm_protect_trap
    svc  #0x80

    mov  x9, #0xBBBB
    str  x9, [x19]              // <-- must take a stage-1 permission fault

    mov  x0, #7                 // "protection was not enforced" — never reached when M13 works
    mov  x16, #1
    svc  #0x80
```

- [ ] **Step 2: Register it in `build.rs` and `src/lib.rs`**

```rust
    // protnone_mach: the M13 t9 twin of protnone.s, protecting through mach_vm_protect (svc -14)
    // rather than mprotect (74). Before M13 that arm returned KERN_SUCCESS without calling the box.
    let src = format!("{}/asm/protnone_mach.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/protnone_mach");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang protnone_mach");
    assert!(status.success(), "protnone_mach guest build failed");
```

```rust
pub const PROTNONE_MACH: &str = concat!(env!("OUT_DIR"), "/protnone_mach");
```

- [ ] **Step 3: Write the failing test**

Append to `crates/retrace-box/tests/protnone.rs` (substituting `<SIG>`):

```rust
use retrace_guest::PROTNONE_MACH;

// mach_vm_protect is a SEPARATE dispatch arm from mprotect and had no box call at all before M13.
// One implementation now serves both, so this proves the arm is wired rather than that the
// mechanism works twice.
#[test]
fn mach_vm_protect_denies_access_too() {
    let loaded = parse_macho(&std::fs::read(PROTNONE_MACH).unwrap());
    let mut b = Box_::load(&loaded);
    let mut protected = 0u64;
    loop {
        match b.run() {
            Stop::Syscall { num, args } if num == 197 => {
                let ipa = b.guest_mmap(args[0], args[1], args[2], args[3]).expect("anon mmap");
                b.set_x0_err_and_return(ipa, false);
            }
            // _kernelrpc_mach_vm_protect_trap(target, addr, size, set_maximum, new_protection):
            // addr=args[1], size=args[2], prot=args[4]. set_maximum is ignored — M13 models current
            // protection only.
            Stop::Syscall { num, args } if num == (-14i64) as u64 => {
                protected = args[1];
                b.guest_mprotect(args[1], args[2], args[4]);
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == 1 =>
                panic!("the guest exited {} — mach_vm_protect did not deny access", args[0]),
            Stop::Fault { esr, far, .. } => {
                assert_eq!(far & !0x3fff, protected,
                    "the fault must be at the protected page {protected:#x}, got {far:#x}");
                assert_eq!(esr & 0x3f, 0x0f, "DFSC must be 0x0f (permission fault, level 3)");
                assert_eq!(retrace_arch::signal_of_esr(esr).0, <SIG>);
                return;
            }
            Stop::Other { esr } => panic!("expected a stage-1 fault, got stage-2 esr={esr:#x}"),
            Stop::Step => unreachable!("run() does not single-step"),
        }
    }
}
```

- [ ] **Step 4: Run it to verify it fails**

Run: `cargo test -p retrace-box --test protnone mach_vm_protect_denies -- --test-threads=1`
Expected: FAIL — "the guest exited 7". The box test drives the arm itself, so this failure is about `guest_mprotect` reaching the page, not about the dispatch. It should in fact PASS already if Task 7 is correct — **if it does, that is expected**: this test pins the arg layout, and Step 5 is what wires the real dispatch.

- [ ] **Step 5: Wire both dispatch arms**

In `crates/retrace-core/src/lib.rs`, replace the record arm at `:348`:

```rust
            // mach_vm_protect: M13 routes it into the box like mprotect(74), through the SAME
            // `guest_mprotect` so record and replay cannot drift. Only `prot == 0` changes anything
            // (see guest_mprotect); every other value keeps the pre-M13 no-op-success behavior, so
            // dyld's RW→RO fixup protects are unaffected. `set_maximum` (args[3]) is ignored: M13
            // models current protection only. Writes nothing itself, so it records like mprotect.
            Stop::Syscall { num, args } if num == MACH_VM_PROTECT => {
                b.guest_mprotect(args[1], args[2], args[4]);
                w.append(&Event::Syscall { num, args, ret: 0, err: false, writes: vec![] }).map_err(|e| format!("append mach_vm_protect: {e}"))?; count += 1;
                b.set_x0_err_and_return(0, false);
            }
```

At the replay arm (`:1123`), add the same call so the mirror recomputes identically. Follow the shape of the adjacent `SYS_MPROTECT` mirror at `:1150`:

```rust
                            if num == MACH_VM_PROTECT {
                                self.b.guest_mprotect(args[1], args[2], args[4]);
                            }
```

- [ ] **Step 6: Run the gate**

Run: `just gate`
Expected: 305 passed / 0 failed / 0 ignored, clippy clean. **Watch the four dynamic gates specifically** — this arm now calls into the box for every `mach_vm_protect` dyld issues.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-core/src/lib.rs crates/retrace-guest/asm/protnone_mach.s \
        crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs \
        crates/retrace-box/tests/protnone.rs
git commit -m "M13 t9: mach_vm_protect stops being a polite lie"
```

---

### Task 10: The fail-loud negative — protecting an uncommitted reservation

**Files:**
- Create: `crates/retrace-guest/asm/protreserve.s`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`
- Test: `crates/retrace-box/tests/protnone.rs`

**Interfaces:**
- Consumes: `protect_none`'s assert (Task 5).
- Produces: `retrace_guest::PROTRESERVE`; a guest-level proof that the M13-split invariant is enforced against a real guest and not only against a hand-built box.

**Why a guest, when Task 5 already unit-tests the assert.** Task 5 calls `protect_none` directly. This proves the invariant survives the *dispatch path* — that a guest can reach it through `mprotect` and still get the loud failure rather than silently having its page committed by `commit_reserved_page` at the next touch. That is the exact silent-wrong-answer M13 exists to remove, so it gets a guest.

- [ ] **Step 1: Write the guest**

Create `crates/retrace-guest/asm/protreserve.s`:

```asm
// M13 t10 fail-loud negative. mprotect(PROT_NONE) over a page inside a PROT_NONE RESERVATION that
// was never committed. M13 models no-access only on BACKED pages: an unbacked protected page would
// fault at stage 2, where commit_reserved_page would silently materialize it rather than fault. So
// this must ASSERT, not quietly succeed.
//
// Reserves via _kernelrpc_mach_vm_map_trap (svc -15) with cur_protection = 0, exactly as
// reservecommit.s does, then mprotects a page inside it WITHOUT ever touching it.
.section __TEXT,__text
.global _start
.p2align 2
_start:
    // mach_vm_map(target, &addr, size, mask, flags=ANYWHERE, ..., cur_prot=0, ...)
    adrp x1, addrout@PAGE
    add  x1, x1, addrout@PAGEOFF
    mov  x0, #0                 // target task
    mov  x2, #0x100000          // size = 1 MiB
    mov  x3, #0                 // mask
    mov  x4, #1                 // VM_FLAGS_ANYWHERE
    mov  x5, #0
    mov  x6, #0
    mov  x7, #0                 // cur_protection = 0 => a RESERVATION, never backed
    mov  x16, #-15              // _kernelrpc_mach_vm_map_trap
    svc  #0x80

    adrp x9, addrout@PAGE
    add  x9, x9, addrout@PAGEOFF
    ldr  x19, [x9]              // the reserved base

    // mprotect(base, 0x4000, PROT_NONE) — on a page with NO backing. Must fail loud.
    mov  x0, x19
    mov  x1, #0x4000
    mov  x2, #0
    mov  x16, #74
    svc  #0x80

    mov  x0, #7                 // never reached: the box must have asserted
    mov  x16, #1
    svc  #0x80

.section __DATA,__data
.p2align 3
addrout: .space 8
```

- [ ] **Step 2: Register it in `build.rs` and `src/lib.rs`**

```rust
    // protreserve: the M13 t10 fail-loud negative — mprotect(PROT_NONE) over a page inside an
    // UNCOMMITTED reservation. protect_none must assert rather than let commit_reserved_page
    // silently materialize the page at the next touch.
    let src = format!("{}/asm/protreserve.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/protreserve");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang protreserve");
    assert!(status.success(), "protreserve guest build failed");
```

```rust
pub const PROTRESERVE: &str = concat!(env!("OUT_DIR"), "/protreserve");
```

- [ ] **Step 3: Write the failing test**

Append to `crates/retrace-box/tests/protnone.rs`:

```rust
use retrace_guest::PROTRESERVE;

// The invariant survives the dispatch path, not just a direct call. A guest that protects a page it
// never committed must hit the loud assert — the alternative is commit_reserved_page materializing
// that page at the next touch, which is precisely the silent wrong answer M13 removes.
#[test]
#[should_panic(expected = "protect_none: no backing")]
fn a_guest_cannot_protect_a_page_it_never_committed() {
    let loaded = parse_macho(&std::fs::read(PROTRESERVE).unwrap());
    let mut b = Box_::load(&loaded);
    loop {
        match b.run() {
            // _kernelrpc_mach_vm_map_trap with cur_protection (args[7]) == 0 is a reservation.
            Stop::Syscall { num, args } if num == (-15i64) as u64 => {
                assert_eq!(args[7], 0, "this guest reserves; cur_protection must be 0");
                let base = b.guest_vm_reserve(0, args[2], true);
                let writes = vec![retrace_trace::Region { ipa: args[1], bytes: base.to_le_bytes().to_vec() }];
                b.apply_and_return(0, false, &writes);
            }
            Stop::Syscall { num, args } if num == 74 => {
                b.guest_mprotect(args[0], args[1], args[2]); // <-- must panic
                b.set_x0_err_and_return(0, false);
            }
            Stop::Syscall { num, args } if num == 1 =>
                panic!("the guest exited {} — protecting an uncommitted page was allowed", args[0]),
            other => panic!("unexpected stop: {other:?}"),
        }
    }
}
```

- [ ] **Step 4: Run it to verify it fails, then passes**

Run: `cargo test -p retrace-box --test protnone a_guest_cannot_protect -- --test-threads=1`
Expected: PASS — the assert added in Task 5 fires through the dispatch path. If it instead reports "the guest exited 7", the invariant is not being enforced and Task 5's assert needs revisiting.

- [ ] **Step 5: Run the full gate**

Run: `just gate`
Expected: 306 passed / 0 failed / 0 ignored, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/retrace-guest/asm/protreserve.s crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace-box/tests/protnone.rs
git commit -m "M13 t10: protecting a page you never committed fails loud, through the real path"
```

---

### Task 11: The headline — `overflow.rs` and `stackoverflow_rust_e2e`

**Files:**
- Create: `crates/retrace-guest/rs/overflow.rs`, `crates/retrace/tests/stackoverflow_rust_e2e.rs`
- Modify: `crates/retrace-guest/build.rs`, `crates/retrace-guest/src/lib.rs`

**Interfaces:**
- Consumes: everything above; the measured signal (Task 1/3).
- Produces: `retrace_guest::OVERFLOW`; the M13 headline gate.

- [ ] **Step 1: Write the guest**

Create `crates/retrace-guest/rs/overflow.rs`:

```rust
// M13 headline guest. A stock full-std Rust binary that overflows its own stack, so libstd's OWN
// guard-page handler runs — the handler M12 could not reach, because retrace did not enforce
// PROT_NONE and the guard page was ordinary writable stack memory.
//
// libstd installs its SIGSEGV/SIGBUS handler at startup (M11 measured flags 0x41 =
// SA_SIGINFO|SA_ONSTACK) and its install_main_guard mmaps a PROT_NONE MAP_FIXED page at
// usrstack64 - RLIMIT_STACK. On a fault the handler compares si_addr against that range; a hit
// prints "thread 'main' has overflowed its stack" and aborts.
//
// black_box on the recursive call is load-bearing: without it the optimizer turns this into a loop
// (or elides it entirely) and the guest never overflows. The array makes each frame big enough that
// the guard is reached in few enough frames to keep the recording small.
use std::hint::black_box;

fn recurse(depth: u64) -> u64 {
    let pad = [depth; 64];
    black_box(&pad);
    black_box(recurse(black_box(depth) + 1)) + pad[0]
}

fn main() {
    println!("about to overflow");
    let d = recurse(0);
    println!("survived at depth {d}");
}
```

- [ ] **Step 2: Register it in `build.rs` and `src/lib.rs`**

Follow `segvy`'s recipe exactly — no `-C panic=abort`, since a guard-page hit is a fault rather than a panic:

```rust
    // overflow: M13's headline — a stock full-std Rust binary that overflows its stack into
    // libstd's PROT_NONE guard page, so libstd's OWN handler recognizes the overflow and aborts.
    // Same recipe as segvy: no -C panic=abort, because a guard hit is a hardware fault, not a panic.
    let src = format!("{}/rs/overflow.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/overflow");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-o", &bin, &src])
        .status().expect("rustc overflow");
    assert!(status.success(), "overflow guest build failed");
```

```rust
pub const OVERFLOW: &str = concat!(env!("OUT_DIR"), "/overflow");
```

- [ ] **Step 3: Sanity-check the guest natively before recording it**

Run:
```bash
"$(find target -name overflow -type f | head -1)" ; echo "native exit=$?"
```
Expected: `about to overflow`, then `thread 'main' has overflowed its stack`, then exit 134 (SIGABRT). **If it prints "survived", the optimizer defeated the recursion** — fix the guest before touching retrace. This step separates "the guest is wrong" from "retrace is wrong", which M12 learned the hard way.

- [ ] **Step 4: Write the failing gate**

Create `crates/retrace/tests/stackoverflow_rust_e2e.rs` (substituting Task 1's measured `<SIG>`):

```rust
// THE M13 HEADLINE GATE. A stock full-std Rust binary overflows its own stack into libstd's
// PROT_NONE guard page. libstd's own handler recognizes the overflow, prints its message, and
// aborts — recorded and replayed bit-for-bit.
//
// The printed message is necessary and nowhere near sufficient. libstd installs the SAME handler
// for SIGSEGV and SIGBUS and prints "has overflowed its stack" whenever si_addr lands in its guard
// range, regardless of which signal arrived — so a gate resting on the string would pass with the
// WRONG signal in the trace. That is M12's exit-139 lesson one milestone later. The trace
// assertions are the gate.
mod util;
use retrace_trace::Event;

#[test]
fn a_rust_guest_overflows_into_its_own_guard_page() {
    let (rec, trace) = util::record_dynamic(retrace_guest::OVERFLOW);
    let out = String::from_utf8_lossy(&rec.stdout);
    let err = String::from_utf8_lossy(&rec.stderr);
    assert!(out.starts_with("about to overflow\n"),
        "the guest must reach its OWN code, not die inside dyld; stdout:\n{out}");
    assert!(!out.contains("survived"),
        "the recursion must hit the guard, not return — check black_box in rs/overflow.rs; \
         stdout:\n{out}");
    assert!(err.contains("has overflowed its stack"),
        "libstd's handler must RECOGNIZE the fault as a stack overflow, which it does by comparing \
         si_addr against its own guard range — this failing means si_addr is wrong or the guard is \
         not where libstd put it; stderr:\n{err}");
    assert_eq!(rec.code, 134, "134 == 128 + SIGABRT: libstd aborts after printing; stderr:\n{err}");

    let (events, torn) = retrace_trace::Reader::open_checked(&trace).unwrap();
    assert!(!torn, "a recorder killed mid-run leaves a torn trace — this must be complete");

    // (1) exactly one delivery, carrying the signal Darwin actually raises for a protection
    // failure (spikes/protnone.c). THIS is the assertion the printed message cannot make: libstd
    // handles both signals identically, so the string proves nothing about which one arrived.
    let deliveries: Vec<_> = events.iter().enumerate()
        .filter(|(_, e)| matches!(e, Event::SignalDelivery { .. })).collect();
    assert_eq!(deliveries.len(), 1, "exactly one handler entry");
    let (di, Event::SignalDelivery { sig, si_addr, handler, .. }) = deliveries[0] else {
        unreachable!()
    };
    assert_eq!(*sig, <SIG>,
        "a PROT_NONE access is a protection failure, and Darwin's signal for that was MEASURED");

    // (2) si_addr is the guard page — the fault is the GUARD, not some unrelated wild access that
    // happens to occur during deep recursion.
    let guard = guard_page_of(&events)
        .expect("libstd's install_main_guard mmaps PROT_NONE MAP_FIXED at startup");
    assert_eq!(si_addr & !0x3fff, guard,
        "si_addr {si_addr:#x} must lie in the guard page at {guard:#x}");

    // (3) the delivery targets the handler libstd installed, learned from the trace rather than
    // hardcoded (it moves with every build). Same technique as segv_rust_e2e.
    let installed = installed_handler(&trace, &events, di, *sig)
        .expect("libstd installs a fault handler at startup — M11 measured flags 0x41");
    assert_eq!(*handler, installed,
        "the delivery must target the handler the guest installed ({installed:#x})");

    // (4) replay is byte-identical, twice.
    for i in 0..2 {
        let rep = util::replay(&trace);
        assert_eq!(rep.code, 134, "replay {i}; stderr:\n{}", rep.stderr);
        assert_eq!(rep.stdout, rec.stdout, "replay {i} stdout diverged");
    }
}

/// The guard page's base, learned from libstd's own `install_main_guard` call: the PROT_NONE
/// (`args[2] == 0`) MAP_FIXED (`args[3] & 0x10`) anonymous mmap it issues at startup. Learned rather
/// than computed, so the gate does not silently agree with a retrace bug in `usrstack64`/`RLIMIT_STACK`.
fn guard_page_of(events: &[Event]) -> Option<u64> {
    events.iter().find_map(|e| match e {
        Event::Syscall { num, args, ret, err, .. }
            if *num == retrace_arch::SYS_MMAP && args[2] == 0 && args[3] & 0x10 != 0 && !*err => {
            Some(*ret & !0x3fff)
        }
        _ => None,
    })
}

/// The handler VA libstd installed for `sig`, read out of reconstructed guest memory at the
/// `sigaction` landmark. The trace carries only a POINTER to the guest's `struct __sigaction`, so
/// this seeks a `ReplaySession` to that landmark and reads `sa_handler` — which also means the
/// assertion checks retrace's own memory reconstruction.
///
/// Seeks to `li + 1`, not `li`: a coordinate `(N, 0)` is the state after `N` events have been
/// CONSUMED, so at `(li, 0)` the guest sits at the start of the window leading to the sigaction and
/// has not yet run the stores that fill the struct. See segv_rust_e2e for the measured evidence.
fn installed_handler(trace: &std::path::Path, events: &[Event], before: usize, sig: u64)
    -> Option<u64> {
    let (li, act_ptr) = events.iter().enumerate().take(before).rev().find_map(|(i, e)| match e {
        Event::Syscall { num, args, .. }
            if *num == retrace_arch::SYS_SIGACTION && args[0] == sig && args[1] != 0 => {
            Some((i, args[1]))
        }
        _ => None,
    })?;
    let s = retrace_core::seek(trace, li + 1, 0).expect("seek past the sigaction landmark");
    let bytes = s.read_mem(act_ptr, 8)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

#[test]
fn the_guard_fault_is_a_seekable_landmark() {
    // "Rewind to just before the stack overflow" is the reverse-debugging payoff of enforcing
    // PROT_NONE at all. M12 made delivery a landmark; this proves M13's fault reaches it.
    let (_, trace) = util::record_dynamic(retrace_guest::OVERFLOW);
    let (events, _) = retrace_trace::Reader::open_checked(&trace).unwrap();
    let di = events.iter().position(|e| matches!(e, Event::SignalDelivery { .. })).unwrap();
    let s = retrace_core::seek(&trace, di, 0).expect("seek to the guard-fault landmark");
    assert_eq!(s.landmark(), di);
}
```

- [ ] **Step 5: Run the gate**

Run: `cargo test -p retrace --test stackoverflow_rust_e2e -- --test-threads=1 --nocapture`
Expected: PASS.

**If it fails, diagnose in this order** — the failure message tells you which:
1. `about to overflow` missing → the guest died in dyld; unrelated to M13, use `RETRACE_TRACE=1`.
2. `survived` present → the optimizer beat the recursion; fix `overflow.rs`, not retrace.
3. `has overflowed its stack` missing but exit is 139 → the guard faulted but `si_addr` is wrong, so libstd read it as an ordinary segfault. Check what `protect_none` actually stamped versus what libstd mmap'd.
4. Neither message and no fault → the guard is not protected; check Task 8's contained-path hook.
5. Signal assertion fails → Task 3's row disagrees with Task 1's measurement.

- [ ] **Step 6: Run the full gate**

Run: `just gate`
Expected: 308 passed / 0 failed / 0 ignored, clippy clean, all six prior headline gates green.

- [ ] **Step 7: Commit**

```bash
git add crates/retrace-guest/rs/overflow.rs crates/retrace-guest/build.rs \
        crates/retrace-guest/src/lib.rs crates/retrace/tests/stackoverflow_rust_e2e.rs
git commit -m "M13 t11: a Rust stack overflow hits the guard, and retrace records its death"
```

---

### Task 12: The honest close

**Files:**
- Modify: `README.md` (new `## Status: M13-protnone` section at the end), `CLAUDE.md`

**Interfaces:**
- Consumes: Task 2's measurement report; the final `just gate` numbers.
- Produces: documentation that matches what the code actually does.

- [ ] **Step 1: Get the real numbers**

Run: `just gate 2>&1 | tail -20`
Record the exact passed/failed/ignored counts and test-binary count. **Use these, not the plan's projections.** Note the gate-log grep gotcha from M6: the output carries ANSI escapes, so `grep` for plain substrings or strip them first.

- [ ] **Step 2: Write the README Status section**

Append `## Status: M13-protnone — 🎉 the guard page actually guards` to `README.md`, following the M12 section's structure. It MUST cover:

- **The headline**: a stock full-`std` Rust binary overflows into libstd's own guard page, libstd recognizes it, prints, aborts, recorded and replayed bit-for-bit.
- **Why the message is not the gate** — libstd handles SIGSEGV and SIGBUS identically, so the gate asserts the signal number and `si_addr`.
- **The measured signal** (Task 1), stated as measured, with the spike named, *including* whether it contradicted the shipped table and that the row had never been exercised by a running guest in six milestones.
- **That the hardware, not software, separates committable from must-fault** — two exception routes, two `Stop` variants — and the invariant it rests on.
- **That the TLBI finally has a caller that needs it**, and that `protnone.s`'s pre-touch ordering is what makes it non-vacuous.
- **The retained deviation, with Task 2's numbers**: `commit_reserved_page` hits per dynamic gate, as a range across three runs, not a single figure.
- **What is still unmodelled and fail-loud**: every protection bit other than no-access; protecting an uncommitted reservation; and everything M12 carries forward (pending signals, nested delivery, `dup2`, guest stdin, threads, arm64e).
- **Any defect the plan's execution surfaced.** M12 found five that would have shipped broken behavior, and *none* was found by reading the plan. If this milestone found some, they are the most valuable thing in the section; if it genuinely found none, say that plainly rather than inventing some.

- [ ] **Step 3: Update `CLAUDE.md`**

Three edits:
1. The milestone list: add **M13-protnone** (`PROT_NONE` enforcement: the guest's guard pages actually guard) after M12.
2. The gate count: replace "296 passed / 0 failed / 0 ignored (90 test binaries)" with Step 1's real numbers.
3. The headline-gate paragraph: "all six headline gates" → seven, adding `stackoverflow_rust_e2e` with a one-line description, and note that it does not rest on its exit code either — 134 is also what `panic_e2e` exits — so it asserts the delivery's signal and `si_addr`.

- [ ] **Step 4: Verify the docs match reality**

Run: `just gate 2>&1 | tail -5`
Cross-check every number you wrote. A Status section that overstates the gate is the one thing this project's discipline exists to prevent.

- [ ] **Step 5: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "M13 t12: the honest close — what guards, what is measured, what is still deferred"
```

- [ ] **Step 6: Merge**

Use `superpowers:finishing-a-development-branch` to decide the integration. The M12 precedent is a `--no-ff` merge to `main` with a summarizing message, then push:

```bash
git checkout main
git merge --no-ff m13-protnone
git push origin main
```

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: M13-attr → 5; M13-split → 5 (the assert) + 7 (the `Stop::Other` assertion) + 10 (the guest); M13-map → 5, 6; M13-sites → 7 (`guest_mprotect`), 8 (`map_mmap_region`, `guest_munmap`), 9 (`mach_vm_protect`); M13-tidy → 4; determinism posture → structural, verified by Task 9's shared implementation and the absence of any `TRACE_MAGIC` change; fail-loud boundaries → 5, 10; the retained deviation → 2, 12; exit criterion → 11; testing → 4, 5, 6, 7, 8, 9, 10, 11; R1 → 1, 3; R2 → 2; R3 → 7; R4 → 2, 9; R5 → 11; R6 → 11 Step 3; R7 → 12 Step 1 note. Open questions 1 and 2 are answered inline in Tasks 3 and 5's doc comments respectively; questions 3–5 are diagnostic and belong to execution, not to a task.

**Two deliberate departures from the spec**, both refinements found while writing the steps:

1. The spec described the mechanism guests as reporting through their own handlers and exit codes. The plan asserts on `Stop::Fault`'s ESR and FAR at box level instead — stronger (it pins the DFSC and the stage-1 route, which an exit code cannot) and simpler (no trampoline, no handler, no re-test of M12).
2. The spec put the TLBI proof on `protrestore`. That is backwards: `protrestore` wants its store to succeed, so a missing flush would make it pass. The plan moves the primary proof to `protnone`'s **pre-protect touch**, which populates the TLB with a writable entry before the attribute changes, and keeps `protrestore` as the proof of `unprotect`'s flush in the opposite direction. Neither can pass vacuously.

**Type consistency.** `protect_none(u64, u64)`, `unprotect(u64, u64)`, `noaccess() -> &[(u64,u64)]`, `ipa_is_noaccess(u64) -> bool`, `set_region_attr(u64, u64, u64)`, `leaf_desc(u64) -> Option<u64>`, `subtract_range(&mut Vec<(u64,u64)>, u64, u64)`, `guest_mprotect(u64, u64, u64)` are used identically everywhere they appear. `BoxState.noaccess` matches `Box_.noaccess`. Guest path constants (`PROTNONE`, `PROTRESTORE`, `PROTNONE_MACH`, `PROTRESERVE`, `OVERFLOW`) match their `build.rs` output names.

**`<SIG>`/`<CODE>` are not placeholders** — they are Task 1's measured output, and Task 3 is explicitly gated on substituting them. Every task that uses them (3, 7, 9, 11) names Task 1 as the source. No task may guess them.

**Projected test counts** (297, 299, 300, 302, 304, 305, 306, 308) assume the current 296 baseline and that each task adds exactly the tests it lists. Treat them as a checksum, not a contract: if a count is off by one, find out why before proceeding — a silently-absent test is how a gate stops meaning anything.
