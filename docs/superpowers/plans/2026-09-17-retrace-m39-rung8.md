# M39-rung8 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rung 8 — the real CPython interpreter runs a repo-owned script that does modest stdlib
work on a data file and ends in a `ctypes` bad-pointer deref; the run records, replays
bit-for-bit, and a scripted `debug` session `reverse-continue`s from the `Event::Crash` to the
store that wrote the bad pointer — the 2026-07-05 vision spec's headline.

**Architecture:** The one measured wall (`mach_vm_remap`, msgh_id 4813, from libffi's Apple
trampoline table on every `import ctypes`) is serviced on the `mach_vm_map` (4811) precedent: a
router route, a pure decoder and reply encoder in `machmsg.rs`, one `Box_` method that writes the
first non-identity stage-1 entries the box has ever written (an alias: the target page's L3
descriptor becomes a copy of the source page's), followed by `flush_guest_tlb`; record appends
the synthesised reply, replay recomputes it through the same method and byte-compares. Beyond
that wall the milestone is a bounded walk (ceiling six, wall 1 included): each further wall is
named from a trace, classified, guarded by a repo-owned red-then-green fixture, fixed under the
symmetry rules, and re-recorded. `arg_kinds` rows are added only with the landmark that reached
them.

**Tech Stack:** Rust 1.95 (pinned), Hypervisor.framework, C guest fixtures built by
`crates/retrace-guest/build.rs` (clang, arm64), Python fixtures as repo files, Homebrew
`python@3.14` at its framework path (tests skip loudly without it), POSIX sh
(`tools/apple-sweep.sh`).

**Spec:** `docs/superpowers/specs/2026-09-17-retrace-m39-rung8-design.md` (and its companion
`2026-09-17-retrace-m39-rung8-measurements.md`, the t0 probe every "measured" claim below cites).

## Global Constraints

- **`--test-threads=1`** on every `cargo test` (one VM per process). The cargo runner ad-hoc-signs
  binaries it runs; a test that spawns the CLI itself uses `util::bin()` (it signs a copy). A
  hand-run of the CLI outside cargo needs `codesign -s - -f --entitlements retrace.entitlements
  <bin>` first — or use `cargo run -q -p retrace -- …`, which goes through the runner.
- **`TRACE_MAGIC` does not move in Tasks 1–5.** It is `RT\x00\x0a`. A wall in the walk (Task 6)
  that needs a trace-shape change may move it to `RT\x00\x0b` (spec §3d, conditionally
  pre-authorised) — first thing in that task, with CLAUDE.md's two mentions and README updated in
  the same commit. No task touches `crates/retrace-trace/src/lib.rs` otherwise.
- **No new returning arm; seven `verify_thread` sites.** The new route lives inside the existing
  `mach_msg2` match under the generic `Syscall` arm, *after* that arm's `verify_thread` (spec R8).
  `grep -c 'self.verify_thread(' crates/retrace-core/src/lib.rs` prints `7` before and after
  every task.
- **Symmetry rule 1 by construction:** `Box_::guest_vm_remap` is called with the same decoded
  arguments on both sides; replay byte-compares the recomputed reply against the recorded
  `writes`. Any further wall's fix follows the same shape or goes below the trace (rule 2).
- **The remap reply's protections are measured, not chosen** (spec R7): the constants
  `VM_REMAP_CUR_PROT` / `VM_REMAP_MAX_PROT` are set from Task 2's native run and its output is
  in Task 2's report. If the native run disagrees with the prediction (5/5), the constants follow
  the run.
- **Every e2e gate asserts on the trace or on bytes** (CLAUDE.md honest-gate rule 1); the rung
  helper's exit-0 demand is the fixture-ran check, never the property. Exit 139 is never asserted
  alone.
- **Row rule** (spec §3b.2): an `arg_kinds` row is added only in the walk task whose trace
  reached it, with that landmark quoted in the row's comment. None is added speculatively.
- **Halt conditions** (spec §5): a red gate surviving one fix round; any `#[ignore]` on a test not
  ignored at `832a1cb`; a class-E sweep row; scope the spec lacks; **the seventh wall**. Stop
  with the branch intact and a written explanation.
- **Commit messages** end with the attribution line the session mandates. **Push only in Task 7
  Step 9**, after the merge to local `main`.
- **Bash tool ceiling is 10 minutes:** the sweep and the gate run `nohup … &` and are polled;
  the gate is chunked (`.superpowers/sdd/2026-09-16-retrace-m38-owed/gate-merge-832a1cb/gate.sh`
  copied with paths changed). macOS has no `timeout(1)`: bound a hand-run with
  `perl -e 'alarm shift; exec @ARGV' 240 <cmd…>`.
- **Worktree** `.claude/worktrees/m39-rung8`, branch `m39-rung8` from `main` at `0016d52`
  (the spec commit; the M38 merge is `832a1cb`); task reports under
  `.superpowers/sdd/2026-09-17-retrace-m39-rung8/`; evidence under
  `docs/sweep-evidence/2026-09-17-m39/`.
- **Homebrew Python** is `/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python`
  (`REAL` in `cpython_e2e.rs`). Present on this machine; both CPython gates skip with a loud
  `eprintln!` elsewhere.
- **Line numbers** in this plan are as of `0016d52`. Re-locate with the quoted text if a prior
  task shifted them.

---

### Task 1: The fixture and the gate, red

**Files:**
- Create: `crates/retrace-guest/py/crash.py`
- Create: `crates/retrace-guest/py/crash.json`
- Modify: `crates/retrace-guest/src/lib.rs:207` (after `EXEC_DYN`) — two constants; a wiring
  test in the existing `mod tests` after `dupfd_guest_parses` (`:329`)
- Create: `crates/retrace/tests/cpython_crash_e2e.rs`

**Interfaces:**
- Produces: `retrace_guest::CRASH_PY: &str`, `retrace_guest::CRASH_JSON: &str` (absolute repo
  paths, no `build.rs` step — spec R1). Task 5 and Task 6 re-run `cpython_crash_e2e`.
- Consumes: `util::record_dynamic_args`, `util::replay`, `util::bin`, `util::strip_annot`
  (`crates/retrace/tests/util/mod.rs`), `retrace_trace::{Reader, Event}`.

- [ ] **Step 1: Write the script.** `crates/retrace-guest/py/crash.py`:

```python
# M39 rung-8 fixture: a real script with modest stdlib use, ending in a ctypes bad-pointer deref.
#
# The bad address is COMPUTED from the data file (base + offset, parsed from hex strings), never a
# literal in this file, so the reverse-debug question "who stored this pointer?" has a real answer:
# the store inside _ctypes's cast() that writes the computed value into the Pointer object's
# buffer. The script reveals that buffer's address (ctypes.addressof(p)) on stdout before the
# deref, the M6 marker convention: tests DISCOVER the cell from the recording, never hardcode it.
#
# 0x4000_DEAD_0000 has bit 46 set (L1 index 0x400, never mapped, < 2^47) — the same FAR crashy.c
# uses — so the deref is a stage-1 EL0 data abort with FAR == the computed target.
#
# Import order is ctypes FIRST on purpose: the t0 measurements (spec companion) showed the one
# wall before the marker is `import ctypes` itself (libffi's mach_vm_remap), and everything the
# other imports need already works; keeping ctypes first keeps the walk's first stop where the
# probe measured it.
import ctypes
import json
import os
import sys

here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, "crash.json")) as f:
    cfg = json.load(f)

table = {}
for row in cfg["rows"]:
    table[row["name"]] = int(row["base"], 16) + int(row["offset"], 16)

target = table[cfg["target"]]
p = ctypes.cast(target, ctypes.POINTER(ctypes.c_long))

sys.stdout.write(f"CRASHPY cell={ctypes.addressof(p):#x} target={target:#x} rows={len(table)}\n")
sys.stdout.flush()

print(p[0])  # stage-1 fault at `target`; never returns
sys.stdout.write("UNREACHED\n")
```

- [ ] **Step 2: Write the data file.** `crates/retrace-guest/py/crash.json`:

```json
{
  "target": "scratch",
  "rows": [
    {"name": "heap",    "base": "0x100000000",   "offset": "0x1000"},
    {"name": "stack",   "base": "0x16f000000",   "offset": "0x8000"},
    {"name": "scratch", "base": "0x400000000000", "offset": "0xdead0000"}
  ]
}
```

- [ ] **Step 3: Run it natively** to confirm the fixture's own contract before any retrace work:

```sh
cd crates/retrace-guest/py && /opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python crash.py; echo "exit=$?"
```
Expected: one line `CRASHPY cell=0x… target=0x4000dead0000 rows=3`, then `exit=139`. (Measured
2026-09-17 in the probe; if this differs, stop — the fixture, not retrace, is wrong.)

- [ ] **Step 4: Constants and the wiring test.** In `crates/retrace-guest/src/lib.rs`, after
  `pub const EXEC_DYN` (`:207`):

```rust
/// M39: the rung-8 script and its data file. Python needs no compile step, so these are repo
/// paths, not `OUT_DIR` products (spec R1); the script finds `crash.json` beside itself.
pub const CRASH_PY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/py/crash.py");
pub const CRASH_JSON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/py/crash.json");
```

and in `mod tests`, after `dupfd_guest_parses` (`:329`):

```rust
    #[test]
    fn crash_py_fixture_is_wired() {
        // M39: proves the two path constants point at the repo files and that the data file
        // carries the target the script computes (the tests assert 0x4000dead0000 by name).
        let py = std::fs::read_to_string(CRASH_PY).unwrap();
        assert!(py.contains("ctypes.cast(target"), "crash.py must cast the computed target");
        assert!(py.contains("CRASHPY cell="), "crash.py must print the M6-style marker");
        let json = std::fs::read_to_string(CRASH_JSON).unwrap();
        assert!(json.contains("\"scratch\"") && json.contains("\"0x400000000000\"")
                && json.contains("\"0xdead0000\""),
                "crash.json must carry base 0x400000000000 + offset 0xdead0000 under `scratch`");
    }
```

- [ ] **Step 5: Run the wiring test.**
  `cargo test -p retrace-guest crash_py_fixture_is_wired -- --test-threads=1`. Expected: PASS.

- [ ] **Step 6: Write the gate.** `crates/retrace/tests/cpython_crash_e2e.rs`:

```rust
// M39 headline gate — rung 8: the real CPython interpreter runs `crash.py` (a script file with
// modest stdlib use on a data file) and dies on a ctypes bad-pointer deref; the run records,
// replays bit-for-bit, and a scripted debug session reverse-continues from the crash to the
// store that wrote the bad pointer.
//
// Four assertions, each on the difference rung 8 makes (CLAUDE.md honest-gate rule 1) — never
// on exit 139 alone, which a guest that died inside dyld produces identically:
//   1. the script RAN: its marker line is in the recorded stdout, `UNREACHED` is not;
//   2. the crash IS the deref: the terminal Event::Crash has far == the computed target, a
//      level-1 translation DFSC, and the thread that wrote the marker;
//   3. replay agrees, twice (byte-identical stdout, no divergence, the M6 crash convention);
//   4. `watch <cell>; reverse-continue` from the crash lands on the store of the pointer,
//      proved by its effect (crashy_cli's proof): before the store the cell is not `target`,
//      one stepi later it is.
// `cell` and every coordinate are DISCOVERED from the recording (the M6 marker convention).
//
// Neither the interpreter nor its stdlib is a repo artifact, so the test skips with a loud
// eprintln! naming the missing path rather than passing quietly — a silent skip reads as a green
// it did not earn. That is also why every mechanism the walk fixes gets its own repo-owned guard
// (vmremap_e2e is the first): this gate guards nothing on a machine without Homebrew Python.
mod util;
use std::path::Path;
use retrace_trace::{Event, Reader};

const REAL: &str =
    "/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python";
const TARGET: u64 = 0x4000_DEAD_0000; // crash.json: 0x400000000000 + 0xdead0000

fn debug_run(trace: &str, script: &str) -> (i32, String, String) {
    let out = std::process::Command::new(util::bin())
        .args(["debug", trace, "--script", script])
        .output().expect("spawn debug");
    (out.status.code().unwrap_or(-1),
     String::from_utf8(out.stdout).unwrap(),
     String::from_utf8(out.stderr).unwrap())
}

/// `cell=0x…` out of the guest's own marker line.
fn parse_cell(stdout: &str) -> u64 {
    let start = stdout.find("CRASHPY cell=0x")
        .unwrap_or_else(|| panic!("missing `CRASHPY cell=0x` in stdout:\n{stdout}")) + "CRASHPY cell=0x".len();
    let hex: String = stdout[start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    u64::from_str_radix(&hex, 16).expect("cell hex")
}

/// The terminal crash event, and the thread tag of the LAST write to fd 1 before it (the marker:
/// nothing else reaches stdout after it — `print(p[0])` never gets to write).
fn crash_and_marker_thread(trace: &Path) -> ((u64, u64, u64, u32), u32) {
    let mut last_write_thread = None;
    let mut crash = None;
    for e in Reader::open(trace).unwrap().iter() {
        match e {
            Event::Syscall { num, args, thread, .. } if (*num == 4 || *num == 397) && args[0] == 1 => {
                last_write_thread = Some(*thread);
            }
            Event::Crash { pc, esr, far, thread } => { crash = Some((*pc, *esr, *far, *thread)); }
            _ => {}
        }
    }
    (crash.expect("trace has a Crash event"), last_write_thread.expect("a write(1) before the crash"))
}

#[test]
fn cpython_runs_a_real_script_crashes_on_the_computed_pointer_and_reverse_debugs_to_its_store() {
    if !Path::new(REAL).exists() {
        eprintln!(
            "SKIPPED cpython_runs_a_real_script…: {REAL} not found (expected a Homebrew \
             `python@3.14` install). This gate did NOT run — it is not evidence of anything."
        );
        return;
    }
    let (rec, trace) = util::record_dynamic_args(REAL, &[retrace_guest::CRASH_PY]);
    let stdout = String::from_utf8_lossy(&rec.stdout).into_owned();

    // 1. The script ran: the marker is there, the line after the deref is not.
    assert!(stdout.contains(&format!("target={TARGET:#x} rows=3")),
        "marker line missing (record exit {}). stdout:\n{stdout}\nstderr:\n{}", rec.code, rec.stderr);
    assert!(!stdout.contains("UNREACHED"), "the deref must not return. stdout:\n{stdout}");
    let cell = parse_cell(&stdout);

    // 2. The crash is the deref: FAR is the computed target; DFSC 0x05 (level-1 translation —
    //    bit 46 of the VA selects an L1 slot that has never been mapped, exactly crashy's
    //    0x92000005 at the same VA); and it happened on the thread that wrote the marker.
    let ((_pc, esr, far, cthread), mthread) = crash_and_marker_thread(&trace);
    assert_eq!(far, TARGET, "crash FAR must be the computed target (esr={esr:#x})");
    assert_eq!(esr & 0x3f, 0x05, "DFSC must be a level-1 translation fault (esr={esr:#x})");
    assert_eq!(cthread, mthread, "the deref must run on the marker's thread");
    assert_eq!(rec.code, 139, "a recorded crash exits 139 (M6). stderr:\n{}", rec.stderr);

    // 3. Replay agrees, twice.
    for i in 1..=2 {
        let rp = util::replay(&trace);
        assert_eq!(rp.code, 139, "replay {i} must reproduce the crash. stderr:\n{}", rp.stderr);
        assert_eq!(rp.stdout, rec.stdout, "replay {i} stdout must be byte-identical");
        assert!(!rp.stderr.contains("DIVERGENCE"), "replay {i} diverged:\n{}", rp.stderr);
    }

    // 4. THE demo: run to the crash, watch the cell the faulting load read the pointer from, run
    //    BACKWARD to its last writer, and prove it by effect: pre-retire the cell does not yet
    //    hold `target`; one stepi later it does. Symbol-free (spec R2): cast() is static in
    //    _ctypes.so.
    let ts = trace.to_str().unwrap();
    let script = format!("continue; watch 0x{cell:x}; reverse-continue; x 0x{cell:x} 8; stepi; x 0x{cell:x} 8");
    let (code, out, err) = debug_run(ts, &script);
    assert_eq!(code, 0, "debug session failed. stderr:\n{err}\nstdout:\n{out}");
    assert!(out.contains(&format!("guest crashed: pc=")), "continue must park at the crash:\n{out}");
    assert!(out.contains(&format!("hit watch 0x{cell:x} (write at ")), "reverse-continue must find a writer:\n{out}");
    let target_hex = TARGET.to_le_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    let xs: Vec<&str> = out.lines().filter(|l| l.starts_with(&format!("0x{cell:x}:"))).collect();
    assert_eq!(xs.len(), 2, "two x dumps expected:\n{out}");
    assert!(!xs[0].contains(&target_hex), "before the store the cell is NOT yet the target:\n{out}");
    assert!(xs[1].contains(&target_hex), "after one stepi the store of the target retired:\n{out}");
}
```

- [ ] **Step 7: Run the gate — expect RED at assertion 1.**
  `cargo test -p retrace --test cpython_crash_e2e -- --test-threads=1`
  Expected: FAIL at "marker line missing (record exit 4)" with stderr ending
  `RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 4813 …` (the measured wall;
  the record aborts before `main.rs` emits the accumulated stdout — measurements Finding 4).
  Save the failing output to
  `.superpowers/sdd/2026-09-17-retrace-m39-rung8/task-1-red.log`. If it fails anywhere else,
  or passes, that is a finding for the report — the probe's tree and this one should agree.

- [ ] **Step 8: Commit.**

```bash
git add crates/retrace-guest/py/crash.py crates/retrace-guest/py/crash.json crates/retrace-guest/src/lib.rs crates/retrace/tests/cpython_crash_e2e.rs
git commit -m "M39 t1: crash.py fixture and the rung-8 gate, red at the measured 4813 wall"
```

---

### Task 2: The guard fixture, and the native measurement it doubles as

**Files:**
- Create: `crates/retrace-guest/c/vmremap_dyn.c`
- Modify: `crates/retrace-guest/build.rs` (after the `dupfd_dyn` block, `:359`)
- Modify: `crates/retrace-guest/src/lib.rs` — `VMREMAP_DYN` constant after `CRASH_JSON`; a
  parse test after `crash_py_fixture_is_wired`
- Create: `crates/retrace/tests/vmremap_e2e.rs`

**Interfaces:**
- Produces: `retrace_guest::VMREMAP_DYN`; the measured `cur`/`max` protections (in the task
  report, and as the two numbers in `vmremap_e2e`'s `EXPECT`) that Task 4 pins into
  `machmsg::VM_REMAP_CUR_PROT` / `VM_REMAP_MAX_PROT`.
- Consumes: `util::assert_rung_records_and_replays`, `retrace_trace::{Reader, Event}`.

- [ ] **Step 1: Write the fixture.** `crates/retrace-guest/c/vmremap_dyn.c`:

```c
// M39 wall-1 guard AND the native protections probe (spec §3f, R7). Two remaps, both shared
// (copy=FALSE), FIXED|OVERWRITE into a vm_allocate'd 3-page region — libffi's exact shape:
//  SELF: alias this program's own text page and CALL through the alias (the executable proof:
//        an alias that is not executable, or not the same bytes, cannot return 42).
//  FFI:  dlopen /usr/lib/libffi-trampolines.dylib (fat x86_64+arm64e — its load is part of the
//        measured wall), remap its whole 2-page __TEXT from the base (the export
//        ffi_closure_trampoline_table_page sits at __TEXT+0x4000), compare bytes through the alias.
// Printed cur/max are the KERNEL's answer natively; under retrace they must be the box's, and
// vmremap_e2e asserts the two agree — so the model's constants are measured, never chosen.
#include <dlfcn.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define PAGE 0x4000UL

// Position-independent leaf: no data references, so it runs from any page it is aliased to.
__attribute__((noinline)) static int forty_two(void) { return 42; }

static kern_return_t remap_shared(mach_vm_address_t target, mach_vm_size_t size,
                                  mach_vm_address_t src, vm_prot_t *cur, vm_prot_t *max) {
    return mach_vm_remap(mach_task_self(), &target, size, 0, VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
                         mach_task_self(), src, FALSE, cur, max, VM_INHERIT_SHARE);
}

int main(void) {
    mach_vm_address_t region = 0;
    kern_return_t kr = mach_vm_allocate(mach_task_self(), &region, 3 * PAGE, VM_FLAGS_ANYWHERE);
    if (kr != KERN_SUCCESS) { printf("SELF allocate kr=%d\n", kr); return 2; }
    uintptr_t fn = (uintptr_t)&forty_two;
    mach_vm_address_t page = fn & ~(PAGE - 1);
    mach_vm_address_t target = region + PAGE;
    vm_prot_t cur = 0, max = 0;
    kr = remap_shared(target, PAGE, page, &cur, &max);
    int called = -1;
    if (kr == KERN_SUCCESS) {
        int (*alias)(void) = (int (*)(void))(target + (fn - page));
        called = alias();
    }
    printf("SELF kr=%d cur=%d max=%d call=%d\n", kr, cur, max, called);

    void *h = dlopen("/usr/lib/libffi-trampolines.dylib", RTLD_NOW);
    void *sym = h ? dlsym(h, "ffi_closure_trampoline_table_page") : NULL;
    if (!sym) { printf("FFI dlopen/dlsym failed: %s\n", dlerror()); return 3; }
    mach_vm_address_t text = ((mach_vm_address_t)sym) - PAGE;
    mach_vm_address_t region2 = 0;
    kr = mach_vm_allocate(mach_task_self(), &region2, 3 * PAGE, VM_FLAGS_ANYWHERE);
    if (kr != KERN_SUCCESS) { printf("FFI allocate kr=%d\n", kr); return 4; }
    mach_vm_address_t target2 = region2 + PAGE;
    vm_prot_t cur2 = 0, max2 = 0;
    kr = remap_shared(target2, 2 * PAGE, text, &cur2, &max2);
    int same = -1;
    if (kr == KERN_SUCCESS) same = memcmp((void *)target2, (void *)text, 2 * PAGE) == 0;
    printf("FFI kr=%d cur=%d max=%d same=%d\n", kr, cur2, max2, same);
    return 0;
}
```

- [ ] **Step 2: Wire `build.rs`.** After the `dupfd_dyn` block (`:359`), same recipe:

```rust
    // vmremap_dyn: the M39 wall-1 guard — two shared mach_vm_remaps (own text page, called
    // through the alias; libffi-trampolines.dylib's __TEXT, compared through it). Also the native
    // protections probe. Same recipe as hello_dyn.
    let src = format!("{}/c/vmremap_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/vmremap_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang vmremap_dyn");
    assert!(status.success(), "vmremap_dyn guest build failed");
```

- [ ] **Step 3: Constant and parse test.** In `crates/retrace-guest/src/lib.rs` after
  `CRASH_JSON`:

```rust
pub const VMREMAP_DYN: &str = concat!(env!("OUT_DIR"), "/vmremap_dyn");
```

and in `mod tests` after `crash_py_fixture_is_wired`:

```rust
    #[test]
    fn vmremap_guest_parses() {
        // M39: proves the build.rs wiring and the path constant; behaviour is vmremap_e2e's.
        let l = parse_macho(&std::fs::read(VMREMAP_DYN).unwrap());
        assert!(l.segments.iter().any(|s| l.entry >= s.vaddr && l.entry < s.vaddr + s.memsz as u64));
    }
```

- [ ] **Step 4: Build and run the parse test.**
  `cargo test -p retrace-guest vmremap_guest_parses -- --test-threads=1`. Expected: PASS
  (this also compiles the fixture).

- [ ] **Step 5: THE MEASUREMENT — run the fixture natively** (no retrace; `.cargo/config.toml`
  sets `[build] target = "aarch64-apple-darwin"`, so the binary is under
  `target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/vmremap_dyn`):

```sh
B=$(ls -t target/aarch64-apple-darwin/debug/build/retrace-guest-*/out/vmremap_dyn | head -1); "$B"; echo "exit=$?"
```
Expected shape: `SELF kr=0 cur=5 max=5 call=42`, `FFI kr=0 cur=5 max=5 same=1`, `exit=0`.
**Record the two lines verbatim in the task report** — they are the measurement spec R7 owes.
The prediction is `cur=5 max=5` for both (the segments' `r-x/r-x`); if the kernel says
otherwise, the kernel is right: carry its numbers forward. If `kr != 0` or `call != 42`
natively, stop — the fixture is wrong, and nothing downstream can be built on it.

- [ ] **Step 6: Write the guard.** `crates/retrace/tests/vmremap_e2e.rs` — put the measured
  numbers into `EXPECT`:

```rust
// M39 wall-1 guard: a shared, FIXED|OVERWRITE mach_vm_remap (4813) of an RX page into a
// vm_allocate'd region is serviced as a stage-1 alias — executable through the alias (SELF's
// call returns 42), byte-identical through it (FFI's memcmp), with the protections the KERNEL
// returns natively (measured 2026-09-17, Task 2 Step 5 — the two numbers in EXPECT), and it
// replays bit-for-bit. Repo-owned so the mechanism is guarded on a machine without Homebrew
// Python (cpython_crash_e2e skips there and guards nothing).
//
// The second assertion is on the TRACE: the 4813 landmark carries a synthesised 60-byte reply as
// its recorded write — the route serviced it, nothing forwarded it (a forward would have handed
// the host kernel retrace's own address space, the M2-mach wall).
mod util;
use retrace_trace::{Event, Reader};

// From the native run (Task 2 Step 5). If a future OS returns different protections this line
// is what changes, together with machmsg::VM_REMAP_{CUR,MAX}_PROT — by measurement, both.
const EXPECT: &[u8] = b"SELF kr=0 cur=5 max=5 call=42\nFFI kr=0 cur=5 max=5 same=1\n";

#[test]
fn a_shared_fixed_remap_is_an_executable_alias_and_replays() {
    let r = util::assert_rung_records_and_replays(retrace_guest::VMREMAP_DYN, &[], EXPECT);
    let remaps: Vec<usize> = Reader::open(&r.trace).unwrap().iter().filter_map(|e| match e {
        Event::Syscall { num, args, writes, .. }
            if *num == (-47i64) as u64 && (args[4] >> 32) == 4813 => Some(writes.iter().map(|w| w.bytes.len()).sum()),
        _ => None,
    }).collect();
    assert_eq!(remaps, vec![60, 60], "two 4813 landmarks, each with one 60-byte synthesised reply: {remaps:?}");
}
```

- [ ] **Step 7: Run the guard — expect RED.**
  `cargo test -p retrace --test vmremap_e2e -- --test-threads=1`
  Expected: FAIL inside `assert_rung_records_and_replays` (record exit 4, stderr
  `RECORD ERROR: unsupported mach_msg2 … msgh_id 4813 …`). Save to
  `.superpowers/sdd/2026-09-17-retrace-m39-rung8/task-2-red.log`.

- [ ] **Step 8: Commit.**

```bash
git add crates/retrace-guest/c/vmremap_dyn.c crates/retrace-guest/build.rs crates/retrace-guest/src/lib.rs crates/retrace/tests/vmremap_e2e.rs
git commit -m "M39 t2: vmremap_dyn guard fixture (red at 4813) and the native protections measurement"
```

---

### Task 3: `Box_::guest_vm_remap` — the stage-1 alias

**Files:**
- Modify: `crates/retrace-box/src/lib.rs` — lift `PT_ADDR` out of `va_to_ipa` (`:4304`) to a
  module const; add two private descriptor helpers and the public method after
  `guest_vm_reserve`'s block (the `impl Box_` that holds `guest_vm_map`, `:2008`)
- Create: `crates/retrace-box/tests/vmremap.rs`

**Interfaces:**
- Produces: `pub fn guest_vm_remap(&mut self, target: u64, size: u64, src: u64) -> u64`
  (returns `target`; panics on the unmodelled shapes named in spec §3f). Task 4 calls it from
  both arms.
- Consumes: `set_region_attr`, `flush_guest_tlb`, `l2_host`, `backings`, `GRANULE`, `BLK`,
  `DESC_TABLE`, `DESC_PAGE`, `ATTR_DATA` (all existing).

- [ ] **Step 1: Write the failing unit test.** `crates/retrace-box/tests/vmremap.rs`:

```rust
// M39 t3: the stage-1 alias behind mach_vm_remap. A static guest is enough: its code page is a
// page-granular ATTR_CODE entry, and `guest_vm_map(anywhere)` is what a vm_allocate becomes.
// The observable is the stage-1 walk itself (`va_to_ipa`): after the alias the TARGET VA
// resolves to the SOURCE's IPA while its neighbours stay identity. Executing through the alias
// is vmremap_e2e's job (it needs a running guest).
use retrace_box::Box_;
use retrace_guest::{parse_macho, HELLO};

#[test]
fn a_remapped_page_resolves_to_the_source_ipa_and_neighbours_stay_identity() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let text = loaded.entry & !0x3fff;                       // the guest's code page
    let region = b.guest_vm_map(0, 0xc000, true, false);     // 3 anon RW pages, ANYWHERE
    let target = region + 0x4000;
    assert_eq!(b.va_to_ipa(target), Some(target), "identity before the alias");
    assert_eq!(b.va_to_ipa(text), Some(text), "the source is identity-mapped");

    assert_eq!(b.guest_vm_remap(target, 0x4000, text), target);

    assert_eq!(b.va_to_ipa(target), Some(text), "the target now walks to the source's IPA");
    assert_eq!(b.va_to_ipa(region), Some(region), "the page below is untouched");
    assert_eq!(b.va_to_ipa(region + 0x8000), Some(region + 0x8000), "the page above is untouched");
    assert_eq!(b.va_to_ipa(text), Some(text), "the source is untouched");
}

#[test]
#[should_panic(expected = "not a page multiple")]
fn a_non_page_multiple_size_is_refused() {
    let loaded = parse_macho(&std::fs::read(HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    let text = loaded.entry & !0x3fff;
    let region = b.guest_vm_map(0, 0xc000, true, false);
    b.guest_vm_remap(region + 0x4000, 0x100, text);
}
```

- [ ] **Step 2: Run it — expect compile failure** (`guest_vm_remap` not found):
  `cargo test -p retrace-box --test vmremap -- --test-threads=1`.

- [ ] **Step 3: Implement.** In `crates/retrace-box/src/lib.rs`:

  a. Lift the constant. Replace, inside `va_to_ipa` (`:4304`),
  `const PT_ADDR: u64 = 0x0000_FFFF_FFFF_C000; // descriptor output-address bits 47:14` with
  nothing, and add next to `DESC_PAGE` (`:436`):
  ```rust
  const PT_ADDR: u64 = 0x0000_FFFF_FFFF_C000; // descriptor output-address bits 47:14
  ```
  (`va_to_ipa`'s three uses now read the module const; no other change there.)

  b. After `guest_vm_reserve`'s closing brace (still inside the `impl Box_` that holds
  `guest_vm_map`), add:
  ```rust
    /// The live L3 descriptor for `va`'s page, or None if its 32 MiB block is still an
    /// unpromoted data BLOCK (no page-granular entry exists). Walks the live L2 like
    /// `promote_and_set` does, by host pointer, not through guest memory.
    fn l3_desc(&self, va: u64) -> Option<u64> {
        let l2 = unsafe { std::slice::from_raw_parts(self.l2_host as *const u64, 2048) };
        let l2e = l2[(va / BLK) as usize];
        if l2e & 0x3 != DESC_TABLE { return None; }
        let l3_ipa = l2e & !(GRANULE as u64 - 1);
        let host = self.backings.iter().find(|b| b.ipa == l3_ipa).map(|b| b.host)?;
        let l3 = unsafe { std::slice::from_raw_parts(host as *const u64, 2048) };
        Some(l3[((va % BLK) / GRANULE as u64) as usize])
    }

    /// Overwrite the live L3 descriptor for `va`'s page. The block must already be promoted
    /// (callers `set_region_attr` first).
    fn write_l3_desc(&mut self, va: u64, desc: u64) {
        let l2 = unsafe { std::slice::from_raw_parts(self.l2_host as *const u64, 2048) };
        let l2e = l2[(va / BLK) as usize];
        assert!(l2e & 0x3 == DESC_TABLE, "write_l3_desc: {va:#x} is in an unpromoted block");
        let l3_ipa = l2e & !(GRANULE as u64 - 1);
        let host = self.backings.iter().find(|b| b.ipa == l3_ipa).map(|b| b.host)
            .expect("write_l3_desc: promoted L3 table backing not found");
        let l3 = unsafe { std::slice::from_raw_parts_mut(host as *mut u64, 2048) };
        l3[((va % BLK) / GRANULE as u64) as usize] = desc;
    }

    /// mach_vm_remap (4813), shared (`copy == FALSE`), at a FIXED target: alias `size` bytes at
    /// `target` to the memory `src` maps, page by page — the first NON-identity stage-1 entries
    /// the box writes. Each target page's L3 descriptor becomes a verbatim copy of the source
    /// page's (its output address, the source's IPA, and its attribute bits — ATTR_CODE for
    /// libffi's RX trampoline text), so the guest reads, and executes, the source through the
    /// target VA. No new backing: an alias adds no memory, the IPA-indexed snapshot/diff never
    /// counts it twice, and M4 checkpoints restore it with the page tables they already carry.
    /// Ends with `flush_guest_tlb`: the target sits in a `vm_allocate`d region the guest may
    /// already have translated as RW/UXN, and M9's rule is that a stale RW entry under a
    /// now-executable page is invalidated by the guest's own `tlbi` before it is executed.
    /// Deterministic — same call, same tables, on both sides. Returns `target`.
    ///
    /// Known consequence, documented not fixed: `read_guest(target)` and the debugger's `x` on
    /// the alias range read the old identity backing, not the source — callers that read guest
    /// memory by VA assume identity. Nothing on rung 8's path reads a trampoline page by VA.
    ///
    /// Asserts (spec §3f — scope the spec lacks): page-multiple size and alignment; every source
    /// page mapped at page granularity (a source inside an unpromoted data block is not the kind
    /// of memory anything remaps as code, and copying a BLOCK descriptor into an L3 slot would
    /// be silent garbage). `copy`, `ANYWHERE` and a foreign `src_task` are dispatch's asserts.
    pub fn guest_vm_remap(&mut self, target: u64, size: u64, src: u64) -> u64 {
        let g = GRANULE as u64;
        assert!(size > 0 && size % g == 0, "vm_remap: size {size:#x} is not a page multiple");
        assert!(target % g == 0 && src % g == 0, "vm_remap: unaligned target {target:#x} / src {src:#x}");
        // Ensure every target page has a page-granular entry (promotes an unpromoted block,
        // identity-filled with ATTR_DATA — what the block already meant), then alias.
        self.set_region_attr(target, size, ATTR_DATA);
        let mut off = 0;
        while off < size {
            let sdesc = self.l3_desc(src + off).unwrap_or_else(|| panic!(
                "vm_remap: source page {:#x} has no page-granular stage-1 entry (unpromoted block)", src + off));
            assert!(sdesc & 0x3 == DESC_PAGE, "vm_remap: source page {:#x} descriptor {sdesc:#x} is not a page", src + off);
            self.write_l3_desc(target + off, sdesc);
            off += g;
        }
        self.flush_guest_tlb();
        target
    }
  ```

- [ ] **Step 4: Run the unit test.**
  `cargo test -p retrace-box --test vmremap -- --test-threads=1`. Expected: both PASS.
  Then `cargo test -p retrace-box -- --test-threads=1` in full (the box's other tests must be
  untouched; `flush_guest_tlb` has its own test that must still pass) and
  `cargo clippy -p retrace-box --all-targets -- -D warnings`.

- [ ] **Step 5: Commit.**

```bash
git add crates/retrace-box/src/lib.rs crates/retrace-box/tests/vmremap.rs
git commit -m "M39 t3: Box_::guest_vm_remap — the stage-1 alias, flushed; PT_ADDR lifted"
```

---

### Task 4: The 4813 codec, route, and both dispatch arms

**Files:**
- Modify: `crates/retrace-core/src/machmsg.rs` — `Route` enum (`:70`), `route()` (`:135`, the
  `4811 =>` arm), a request struct + decoder after `decode_vm_map` (`:236`), a reply encoder
  after `encode_vm_map_reply` (`:347`), tests in the existing `mod tests`.
- Modify: `crates/retrace-core/src/lib.rs` — record arm after `Route::ServiceVmMap`'s block
  (`:451`–`:470`); replay arm after its `Route::ServiceVmMap` block (`:1878`–`:1898`). **Both
  `match`es on `Route` are exhaustive (no wildcard), so the variant and its two arms must land in
  the same task or the crate does not build.**

**Interfaces:**
- Produces: `Route::ServiceVmRemap`; `pub struct VmRemapReq { target: u64, size: u64, mask: u64,
  flags: u32, src_task: u32, src: u64, copy: u32, inheritance: u32 }`;
  `pub fn decode_vm_remap(buf: &[u8]) -> Result<VmRemapReq, String>`;
  `pub fn encode_vm_remap_reply(reply_port: u32, target: u64, cur: u32, max: u32) -> Vec<u8>`
  (60 bytes with trailer); `VM_REMAP_CUR_PROT` / `VM_REMAP_MAX_PROT: u32`.
- Consumes: `Box_::guest_vm_remap` (Task 3), `VM_FLAGS_ANYWHERE` (`lib.rs:36`), `Region`,
  `Event::Syscall`, `guest_task_port: Option<u64>` (record's local at `:139`, replay's field).

- [ ] **Step 1: Write the failing tests.** In `mod tests` of `machmsg.rs`, after
  `decode_rejects_malformed`:

```rust
    /// The 4813 request the t0 probe captured (measurements Finding 2): libffi remapping
    /// libffi-trampolines.dylib's 2-page __TEXT (src 0xa0183c000) shared, FIXED|OVERWRITE, into
    /// the region it vm_allocate'd (target 0xa017fc000).
    const FIXTURE_VM_REMAP_REQ: [u8; 92] = [
        0x13,0x15,0x00,0x80, 0x5c,0x00,0x00,0x00, 0x03,0x02,0x00,0x00, 0x03,0x14,0x00,0x00,
        0x00,0x00,0x00,0x00, 0xcd,0x12,0x00,0x00, 0x01,0x00,0x00,0x00, 0x03,0x02,0x00,0x00,
        0x00,0x00,0x00,0x00, 0x00,0x00,0x13,0x00, 0x00,0x00,0x00,0x00, 0x01,0x00,0x00,0x00,
        0x00,0xc0,0x7f,0x01, 0x0a,0x00,0x00,0x00, 0x00,0x80,0x00,0x00, 0x00,0x00,0x00,0x00,
        0x00,0x00,0x00,0x00, 0x00,0x00,0x00,0x00, 0x00,0x40,0x00,0x00, 0x00,0xc0,0x83,0x01,
        0x0a,0x00,0x00,0x00, 0x00,0x00,0x00,0x00, 0x00,0x00,0x00,0x00,
    ];
    #[test]
    fn decodes_the_captured_vm_remap_request() {
        let r = decode_vm_remap(&FIXTURE_VM_REMAP_REQ).unwrap();
        assert_eq!(r.target, 0xa_017f_c000);
        assert_eq!(r.size, 0x8000);
        assert_eq!(r.mask, 0);
        assert_eq!(r.flags, 0x4000);               // VM_FLAGS_OVERWRITE; FIXED (ANYWHERE clear)
        assert_eq!(r.src_task, 0x203);             // the guest's own task port
        assert_eq!(r.src, 0xa_0183_c000);
        assert_eq!(r.copy, 0);                     // shared
        assert_eq!(r.inheritance, 0);              // VM_INHERIT_SHARE
    }
    #[test]
    fn vm_remap_decode_rejects_malformed() {
        assert!(decode_vm_remap(&FIXTURE_VM_REMAP_REQ[..88]).is_err());         // short
        let mut bad = FIXTURE_VM_REMAP_REQ; bad[20] = 0xcc;                     // msgh_id byte
        assert!(decode_vm_remap(&bad).is_err());
        let mut bad = FIXTURE_VM_REMAP_REQ; bad[24] = 2;                        // desc_count
        assert!(decode_vm_remap(&bad).is_err());
        let mut bad = FIXTURE_VM_REMAP_REQ; bad[0] = 0x13;                      // complex bit clear
        assert!(decode_vm_remap(&bad).is_err());
    }
    #[test]
    fn vm_remap_reply_has_the_documented_shape() {
        // header(24) + NDR(8) + RetCode(4) + target(8) + cur(4) + max(4) = 52 = msgh_size;
        // + trailer(8) = 60 = the rcv_size the probe saw. Reply id = 4813 + 100.
        let e = encode_vm_remap_reply(0x1403, 0xa_017f_c000, 5, 5);
        assert_eq!(e.len(), 60);
        assert_eq!(u32::from_le_bytes(e[4..8].try_into().unwrap()), 52);          // msgh_size
        assert_eq!(u32::from_le_bytes(e[12..16].try_into().unwrap()), 0x1403);    // reply-local port
        assert_eq!(i32::from_le_bytes(e[20..24].try_into().unwrap()), 4913);      // reply id
        assert_eq!(i32::from_le_bytes(e[32..36].try_into().unwrap()), 0);         // KERN_SUCCESS
        assert_eq!(u64::from_le_bytes(e[36..44].try_into().unwrap()), 0xa_017f_c000);
        assert_eq!(u32::from_le_bytes(e[44..48].try_into().unwrap()), 5);         // cur_protection
        assert_eq!(u32::from_le_bytes(e[48..52].try_into().unwrap()), 5);         // max_protection
        assert_eq!(&e[52..60], &TRAILER);
    }
    #[test]
    fn routes_vm_remap_to_service_only_on_the_guest_task_port() {
        assert!(matches!(route(&msg(4813, 0x203, KOBJ), Some(0x203)), Route::ServiceVmRemap));
        // Not the guest's task port: stays unsupported (fail loud), never serviced.
        assert!(matches!(route(&msg(4813, 0x207, KOBJ), Some(0x203)), Route::Unsupported(_)));
    }
```

- [ ] **Step 2: Run them — expect compile failure** (`decode_vm_remap`, `Route::ServiceVmRemap`
  not defined): `cargo test -p retrace-core --lib machmsg -- --test-threads=1`.
  (After Step 3 the build fails differently — the two exhaustive `match`es in `lib.rs` miss the
  new variant — until Steps 5–6 add the arms. That is expected; do not add a wildcard.)

- [ ] **Step 3: Implement.** In `machmsg.rs`:

  a. `Route` (`:70`): add `ServiceVmRemap` after `ServiceVmMap`:
  ```rust
  pub enum Route { ServiceVmMap, ServiceVmRemap, ServiceGetSpecialPort, ServiceSetSpecialPort, StubMigReply(i32),
  ```
  b. `route()` (`:135`), after `4811 => return Route::ServiceVmMap,`:
  ```rust
            // mach_vm_remap (4813): libffi's Apple trampoline table aliases the freshly-loaded
            // libffi-trampolines.dylib __TEXT into a vm_allocate'd region, shared, on every
            // `import ctypes` (M39 t0). Serviced as a stage-1 alias (Box_::guest_vm_remap) —
            // never forwarded (that would remap retrace's own address space). Dispatch decodes
            // the body and asserts the shapes the spec models (copy=FALSE, FIXED, own task).
            4813 => return Route::ServiceVmRemap,
  ```
  c. After `decode_vm_map` (`:236`):
  ```rust
  /// _kernelrpc_mach_vm_remap (4813) request body: header(24) + desc_count(4) + port
  /// descriptor(12: src_task name @28, pad, disposition @38) + NDR(8) + target(8) @48 + size(8)
  /// @56 + mask(8) @64 + flags(4) @72 + src(8) @76 + copy(4) @84 + inheritance(4) @88 = 92.
  /// Captured byte-for-byte in the M39 t0 probe (measurements Finding 2).
  pub struct VmRemapReq {
      pub target: u64, pub size: u64, pub mask: u64, pub flags: u32,
      pub src_task: u32, pub src: u64, pub copy: u32, pub inheritance: u32,
  }

  pub fn decode_vm_remap(buf: &[u8]) -> Result<VmRemapReq, String> {
      if buf.len() < 92 { return Err(format!("vm_remap request short: {} < 92", buf.len())); }
      let (bits, id, descs) = (u32_at(buf, 0), u32_at(buf, 20), u32_at(buf, 24));
      if id != 4813 { return Err(format!("msgh_id {id} != 4813")); }
      if bits & MACH_MSGH_BITS_COMPLEX == 0 { return Err("complex bit clear".into()); }
      if descs != 1 { return Err(format!("descriptor count {descs} != 1")); }
      Ok(VmRemapReq {
          src_task: u32_at(buf, 28),
          target: u64_at(buf, 48), size: u64_at(buf, 56), mask: u64_at(buf, 64),
          flags: u32_at(buf, 72), src: u64_at(buf, 76), copy: u32_at(buf, 84),
          inheritance: u32_at(buf, 88),
      })
  }
  ```
  d. After `encode_vm_map_reply` (`:347`):
  ```rust
  /// KERN_SUCCESS reply for 4813: header(24) + NDR(8) + RetCode(4) + target(8) + cur_protection(4)
  /// + max_protection(4) = 52, + trailer(8) = 60 (the rcv_size the probe saw). The protections are
  /// the kernel's measured answer (VM_REMAP_CUR_PROT / VM_REMAP_MAX_PROT), passed in by dispatch.
  pub fn encode_vm_remap_reply(reply_port: u32, target: u64, cur: u32, max: u32) -> Vec<u8> {
      let mut out = Vec::with_capacity(60);
      reply_header(&mut out, 52, reply_port, 4913);
      out.extend_from_slice(&NDR);
      out.extend_from_slice(&0i32.to_le_bytes());            // KERN_SUCCESS
      out.extend_from_slice(&target.to_le_bytes());
      out.extend_from_slice(&cur.to_le_bytes());
      out.extend_from_slice(&max.to_le_bytes());
      out.extend_from_slice(&TRAILER);
      out
  }
  ```
  e. Next to `MACH_MSG_SUCCESS` (`:163`), the two constants Task 5's arms pass, pinned to
  Task 2 Step 5's native output (edit the numbers if the measurement differed from 5/5):
  ```rust
  /// The protections the kernel returns for a shared remap of an r-x/r-x text segment —
  /// MEASURED natively by vmremap_dyn (M39 Task 2 Step 5: `SELF … cur=5 max=5`, `FFI … cur=5
  /// max=5`), not chosen (spec R7). vmremap_e2e asserts the box's answer matches the kernel's.
  pub const VM_REMAP_CUR_PROT: u32 = 5;
  pub const VM_REMAP_MAX_PROT: u32 = 5;
  ```

- [ ] **Step 4: Confirm the build now fails only on the two exhaustive matches.**
  `cargo build -p retrace-core 2>&1 | grep -E 'non-exhaustive|ServiceVmRemap'` — two
  `non-exhaustive patterns` errors naming `ServiceVmRemap`, one per dispatch loop, and nothing
  else. Then add the arms.

- [ ] **Step 5: The record arm.** In `record_box`, directly after the
  `machmsg::Route::ServiceVmMap => { … }` block (ends `b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes); }` at `:470`):

```rust
                    machmsg::Route::ServiceVmRemap => {
                        // M39 wall 1: mach_vm_remap (4813). libffi's Apple trampoline table
                        // aliases libffi-trampolines.dylib's freshly-loaded __TEXT into the region
                        // it vm_allocate'd, shared, FIXED|OVERWRITE (measured on every `import
                        // ctypes`). Serviced as a stage-1 alias — the vm_map posture: reply
                        // synthesised here, recomputed and byte-compared on replay. The
                        // protections are the kernel's measured answer, never chosen (spec R7).
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let req = machmsg::decode_vm_remap(&buf)
                            .unwrap_or_else(|e| panic!("mach_vm_remap (4813) decode: {e}"));
                        assert_eq!(req.copy, 0, "mach_vm_remap: copy=TRUE is unmodelled (spec §3f)");
                        assert_eq!(req.flags as u64 & VM_FLAGS_ANYWHERE, 0,
                            "mach_vm_remap: VM_FLAGS_ANYWHERE is unmodelled (spec §3f)");
                        assert_eq!(Some(req.src_task as u64), guest_task_port,
                            "mach_vm_remap: src_task {:#x} is not the guest's own task port", req.src_task);
                        let target = b.guest_vm_remap(req.target, req.size, req.src);
                        let writes = vec![Region { ipa: m.data, bytes: machmsg::encode_vm_remap_reply(
                            m.reply_port, target, machmsg::VM_REMAP_CUR_PROT, machmsg::VM_REMAP_MAX_PROT) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS, ret1: 0,
                            err: false, writes: writes.clone(), thread })
                            .map_err(|e| format!("append mach_msg2 vm_remap: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
```

- [ ] **Step 6: The replay arm.** In `ReplaySession::advance`, directly after the
  `machmsg::Route::ServiceVmMap => { … }` block (ends `self.b.apply_and_return(*ret, *err, writes); }` at `:1898`):

```rust
                                    machmsg::Route::ServiceVmRemap => {
                                        // Mirror of record's arm: same decode, same asserts, same
                                        // Box_ call, then the byte-equality that IS the oracle.
                                        let buf = self.b.read_guest(m.data, m.send_size as usize);
                                        let req = machmsg::decode_vm_remap(&buf).map_err(|e| Divergence {
                                            landmark: self.idx, pc, detail: format!("replay vm_remap decode: {e}") })?;
                                        assert_eq!(req.copy, 0, "mach_vm_remap: copy=TRUE is unmodelled (spec §3f)");
                                        assert_eq!(req.flags as u64 & VM_FLAGS_ANYWHERE, 0,
                                            "mach_vm_remap: VM_FLAGS_ANYWHERE is unmodelled (spec §3f)");
                                        assert_eq!(Some(req.src_task as u64), self.guest_task_port,
                                            "mach_vm_remap: src_task {:#x} is not the guest's own task port", req.src_task);
                                        let target = self.b.guest_vm_remap(req.target, req.size, req.src);
                                        let reply = machmsg::encode_vm_remap_reply(
                                            m.reply_port, target, machmsg::VM_REMAP_CUR_PROT, machmsg::VM_REMAP_MAX_PROT);
                                        if writes.len() != 1 || writes[0].bytes != reply {
                                            return Err(Divergence { landmark: self.idx, pc,
                                                detail: format!("mach_vm_remap reply mismatch: replay target {target:#x}") });
                                        }
                                        self.b.apply_and_return(*ret, *err, writes);
                                    }
```

- [ ] **Step 7: Build, run the codec tests, count the oracle sites.**
  `cargo build --workspace` clean;
  `cargo test -p retrace-core --lib machmsg -- --test-threads=1` — the four new tests PASS and
  every existing `machmsg` test still passes;
  `grep -c 'self.verify_thread(' crates/retrace-core/src/lib.rs` prints `7` (spec R8);
  `cargo clippy -p retrace-core --all-targets -- -D warnings` clean.

- [ ] **Step 8: Commit.**

```bash
git add crates/retrace-core/src/machmsg.rs crates/retrace-core/src/lib.rs
git commit -m "M39 t4: mach_vm_remap (4813) — route, codec, measured protections, both dispatch arms"
```

---

### Task 5: The guard green, rung 7 intact, and the walk's next stop

**Files:**
- No source change expected. Report: `.superpowers/sdd/2026-09-17-retrace-m39-rung8/task-5-report.md`
  plus the two logs named below. If a step here needs a code change, that change is a finding
  for the report and a Task 6 instantiation — not a silent fix.

**Interfaces:**
- Consumes: Tasks 2–4 complete on the branch.
- Produces: the walk's next stopping point (which `cpython_crash_e2e` assertion fails, at which
  landmark, on what trap), written in the report — Task 6's input.

- [ ] **Step 1: The guard goes green.**
  `cargo test -p retrace --test vmremap_e2e -- --test-threads=1`. Expected: PASS — stdout equals
  `EXPECT` (the box's `cur`/`max` are the kernel's), two 60-byte replies in the trace, replayed
  twice. If `SELF … call=` is not 42 under retrace while it was natively, the alias is not
  executable: check `flush_guest_tlb` ran and the copied descriptor is `ATTR_CODE` — do not
  loosen `EXPECT`.

- [ ] **Step 2: The rung-7 gate still passes.**
  `cargo test -p retrace --test cpython_e2e -- --test-threads=1`. Expected: PASS (both tests
  ran — no `SKIPPED` line).

- [ ] **Step 3: The walk's next stop.**
  `cargo test -p retrace --test cpython_crash_e2e -- --test-threads=1 2>&1 | tee .superpowers/sdd/2026-09-17-retrace-m39-rung8/task-5-crash-run.log`
  — capture cargo's exit code **before** the pipe (`set -o pipefail` or run twice). Then record
  the same guest by hand with the trace flag for the landmark:
  `RETRACE_TRACE=1 perl -e 'alarm shift; exec @ARGV' 240 cargo run -q -p retrace -- record-dyn "$REAL" -o /tmp/m39-t5.bin -- crates/retrace-guest/py/crash.py 2> .superpowers/sdd/2026-09-17-retrace-m39-rung8/task-5-trace.err > .superpowers/sdd/2026-09-17-retrace-m39-rung8/task-5-trace.out; echo "exit=$?"`
  In the report, state exactly one of:
  - **GREEN** — all four assertions pass: rung 8 is reached with one wall; Task 6 is empty and
    Task 7 begins.
  - **Assertion N fails at landmark L on trap T** (from the last `[trap]`/`[fault]`/`RECORD
    ERROR`/`DIVERGENCE` line): wall 2, named. Quote the line. Do not fix it in this task.

- [ ] **Step 4: Commit the report and logs** (they are exclude-listed under `.superpowers/`, so
  this is a report-only task with no repo commit unless a step above produced one; say which in
  the report).

---

### Task 6: The walk — one instantiation per further wall (up to five)

This task is a **procedure**, not a fixed change: nothing past wall 1 has been measured (spec
§2, measurements "What was not measured"), so no code can be written for it in advance without
inventing a mechanism — the failure the probe exists to prevent. The controller instantiates it
as `Task 6.k` (k = 2…6) by writing a brief at
`.superpowers/sdd/2026-09-17-retrace-m39-rung8/task-6.<k>-brief.md` **from the previous task's
measured stopping point**, with the steps below filled in with that wall's names. Every
instantiation has the same steps, and each is its own reviewer's gate. The seventh wall halts
(spec §5).

**Files (per instantiation):** the crate the wall lives in; a fixture under
`crates/retrace-guest/{c,asm}/`; a `build.rs` block, a constant and a parse test (Task 2 Steps
2–3's shape); a new `crates/retrace/tests/<mechanism>_e2e.rs`; the report.

- [ ] **Step 1: Name it.** From the trace-flag run of the previous task: the landmark index, the
  trap (`num`, decoded name — `crates/retrace-arch/src/lib.rs`'s syscall table, or the
  `mach_msg2` msgh_id and its subsystem), the thread, and the `pc`. If it is a `RECORD ERROR`,
  quote it; if a replay `DIVERGENCE`, quote both sides' `(num, args)`; if a `[fault]`, the ESR
  and FAR. One paragraph, verbatim lines.

- [ ] **Step 2: Classify it**, one of:
  - **A missing `arg_kinds` row** (the M33 fail-loud names the number): add the row in
    `crates/retrace-arch/src/lib.rs`'s table with the landmark quoted in its comment, mirror any
    `fd_operands`/`Ret` view it needs, run `cargo test -p retrace-arch -- --test-threads=1` (the
    census tests count rows — update the count they assert with the row named). No fixture is
    owed beyond the census (spec §3b.3); the sweep at the close is its regression check.
  - **A mechanism gap** (a `Box_` path, a new trap shape, an unmodelled Mach RPC, a replay
    asymmetry): write the design in the brief — which `Box_` method, which route, which of
    symmetry rules 1/2 it falls under, what it asserts as unmodelled — *before* any code. If the
    mechanism needs a trace-shape change, this is where `TRACE_MAGIC` moves to `RT\x00\x0b`
    (Global Constraints), first commit of the task.
  - **A CPython/libffi behaviour that is not a retrace defect** (e.g. the crash arrives but on a
    path the gate did not predict): the gate's assertion is re-measured and, if the fixture's
    contract holds natively, the assertion is corrected with the measurement quoted — never
    loosened past what rung 8 makes different.

- [ ] **Step 3: Guard it, red.** For a mechanism gap: a freestanding C (or asm) fixture that
  exercises exactly that mechanism without CPython, a `<mechanism>_e2e.rs` asserting on the
  difference the fix makes (bytes or trace, never an exit code alone), run → RED, log kept as
  `task-6.<k>-red.log`.

- [ ] **Step 4: Fix it.** The mechanism, in both dispatch arms (or below the trace); a returning
  arm is a new `verify_thread` site and CLAUDE.md's count moves with it in the same commit.

- [ ] **Step 5: Guard green; neighbours untouched.**
  `cargo test -p retrace --test <mechanism>_e2e -- --test-threads=1` PASS;
  `cargo test -p retrace --test vmremap_e2e --test cpython_e2e -- --test-threads=1` PASS;
  `cargo test -p retrace-core -p retrace-box -p retrace-arch -- --test-threads=1` PASS;
  `cargo clippy --workspace --all-targets -- -D warnings` clean.

- [ ] **Step 6: Re-record and name the next stop** — Task 5 Step 3 verbatim (the gate run, the
  trace-flag run, the report's one-of statement). `k == 6` and not GREEN → **halt** with the
  walk written up (spec §5).

- [ ] **Step 7: Commit** `M39 t6.<k>: <mechanism> — <one line>; crash.py's next stop named`.

---

### Task 7: Close — README, CLAUDE.md, status log, spec §10, sweep, gate, merge, push

**Files:**
- Modify: `README.md` ("What works today" rung table — the `| 7 |` row is at `:111`; a new
  demo subsection after the table; "Known limits" at `:448` for anything re-parked; the gate
  paragraph's figures)
- Modify: `CLAUDE.md` (the e2e list in "Commands": add `cpython_crash_e2e` and `vmremap_e2e`
  after `exec_e2e`; the `verify_thread` count only if Task 6 moved it; `TRACE_MAGIC` only if
  Task 6 moved it)
- Modify: `docs/status-log.md` (append one section; never edit an old one)
- Modify: `docs/superpowers/specs/2026-09-17-retrace-m39-rung8-design.md` (§10 Outcome)
- Create: `docs/sweep-evidence/2026-09-17-m39/README.md` + the sweep's kept files

- [ ] **Step 1: The demo transcript.** From the worktree, on the final tree:
  ```sh
  REAL=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
  cargo run -q -p retrace -- record-dyn "$REAL" -o /tmp/m39-demo.bin -- crates/retrace-guest/py/crash.py; echo "record exit=$?"
  cargo run -q -p retrace -- replay /tmp/m39-demo.bin; echo "replay exit=$?"
  CELL=$(cargo run -q -p retrace -- replay /tmp/m39-demo.bin 2>/dev/null | sed -n 's/.*cell=\(0x[0-9a-f]*\).*/\1/p')
  cargo run -q -p retrace -- debug /tmp/m39-demo.bin --script "continue; watch $CELL 8; reverse-continue; where; x $CELL 8; stepi; x $CELL 8"
  ```
  Keep the exact output for the README (trim nothing but the `[retrace] fall-throughs` line).

- [ ] **Step 2: README.** Edit in place. The rung table gains
  `| 8 | real CPython on a real script that crashes | `crash.py`: json/os/sys work on a data file, then a ctypes deref; recorded, replayed, and reverse-continued from the crash to the store of the pointer |`.
  Below the table, a subsection **"Reverse-debugging CPython"** with the Step 1 transcript and
  three sentences: what the script does, what the crash is, what `reverse-continue` found. "What
  works today" also states that `import ctypes` works (libffi's trampoline `mach_vm_remap`
  serviced as a stage-1 alias — the first non-identity stage-1 entry the box writes) and lists
  every further mechanism Task 6 cleared, one line each with its guard's name. "Known limits":
  the alias consequence (`x`/`read_guest` on an alias range read the old backing); anything Task 6
  re-parked; the gate figures replaced by Step 7's.

- [ ] **Step 3: CLAUDE.md.** The e2e list: after `exec_e2e (…)`, add `cpython_crash_e2e`
  (rung 8 — the real interpreter on `crash.py`, asserting the marker, the `Event::Crash` at the
  computed target, two replays, and the reverse-continue to the store by its effect; skips loud
  without Homebrew Python) and `vmremap_e2e` (the repo-owned guard for the `mach_vm_remap`
  alias, so the mechanism is guarded on a machine without Python). Counts/magic only if moved.

- [ ] **Step 4: Spec §10.** Outcome vs §9: the measured figures, the wall count (1 + Task 6's),
  each wall named with its guard, the remap protections as measured, and every prediction in the
  spec marked confirmed or corrected (the DFSC, the thread tag, the second wall).

- [ ] **Step 5: Status log.** Append `## Status: M39-rung8 — 🎉 rung 8: a real script, a real
  crash, and reverse-continue to the store` (or, if halted, `## Status: M39-rung8 — parked at
  wall <k>: <name>`), in the M38 section's shape: the numbers paragraph, "What it set out to
  do", one subsection per wall with the measurement that named it and the red-then-green guard,
  "The gate" (Step 7's table, reconciled file-by-file against 618/0/9 over 135 with the
  per-file `#[test]` deltas), "Rulings", "What stays owed" (M38's list carried forward, minus
  anything discharged here, plus: the alias read-by-VA consequence; `copy=TRUE`/`ANYWHERE`/
  foreign-task remaps; symbols for runtime-loaded dylibs; the lldb seam and async signals as the
  remaining v1 gaps).

- [ ] **Step 6: The sweep.** From the worktree, `cargo build -p retrace`, then
  `RETRACE_SWEEP_KEEP=docs/sweep-evidence/2026-09-17-m39/sweep nohup tools/apple-sweep.sh > docs/sweep-evidence/2026-09-17-m39/sweep.log 2>&1 &`
  and poll (`until grep -q '^TALLY' docs/sweep-evidence/2026-09-17-m39/sweep.log; do sleep 30; done`,
  bounded). Diff the `ROW` lines against `docs/sweep-evidence/2026-09-16-m38/README.md`'s table
  by binary name; every row whose label or `rec_reason` changed is listed in the evidence README
  with the task that moved it. PASS ≥ 44. **A row that moved for a reason this plan does not
  name is a finding** (spec §5). Commit the evidence with the docs:
  `M39 t7: README rung 8 + transcript, status-log section, spec outcome, sweep`.

- [ ] **Step 7 (controller): the gate.** Copy
  `.superpowers/sdd/2026-09-16-retrace-m38-owed/gate-merge-832a1cb/gate.sh` to
  `.superpowers/sdd/2026-09-17-retrace-m39-rung8/gate.sh`, set `W=` to the worktree and `L=` to
  `…/2026-09-17-retrace-m39-rung8/gate`, run `nohup bash gate.sh > gate-run.log 2>&1 &`, poll
  for `GATE DONE`. Every chunk `exit=0`; `binaries=` / `passed=` / `failed=0` / `ignored=9`
  (or `10`, named, if halted); zero `SKIPPED` lines. Reconcile file-by-file against 618/0/9 over
  135 (`git diff 832a1cb HEAD -- crates | grep -c '^+.*#\[test\]'`, per file) and put the table
  in the status-log section; re-run the gate if any commit after it touches `crates/`.

- [ ] **Step 8 (controller): merge.** From the repo root:
  `git merge --no-ff m39-rung8 -F <message file>` — the message in the M37/M38 merge shape (what
  landed, the numbers, the walls, the gate figure and the commit it was measured on).

- [ ] **Step 9 (controller): push.** `git push origin main` — the one push this milestone makes.
  Then `git worktree remove .claude/worktrees/m39-rung8 && git branch -d m39-rung8`, after
  copying `.superpowers/sdd/2026-09-17-retrace-m39-rung8/` to the root (the exclude-listed SDD
  directory does not travel with the merge).

---

## Self-Review

**Spec coverage.** §3a fixture → Task 1 Steps 1–5. §3b walk rules → Task 6 (procedure), row rule
in Global Constraints and Task 6 Step 2, ceiling in Task 6 Step 6. §3c gate, four assertions →
Task 1 Step 6 (1: marker; 2: FAR/DFSC/thread; 3: two replays; 4: the effect proof). §3c.5 guards
→ Task 2 (`vmremap_e2e`) and Task 6 Step 3. §3d symmetry → Task 4's two arms, Global Constraints
(`verify_thread` 7, `TRACE_MAGIC` conditional). §3e demo → Task 7 Steps 1–2. §3f wall 1 → Tasks
3 (alias + flush, unit test), 4 (codec/route, asserts named, both arms), 2 (guard + measured
protections; R7), 5 (green, and the next stop). §4 order → Tasks 1–7 in that order. §5 envelope → Global Constraints, Task 6
Step 6, Task 7 Step 9. §6 acceptance → Task 7 Steps 6–7. §7 out-of-scope → nothing here touches
exec-in-place, threads, the lldb seam, async signals, runtime-dylib symbols, sweep-only rows or
`faulthandler`. §8 R1 (repo paths) → Task 1 Step 4; R2 (effect not symbol) → Task 1 Step 6
assertion 4; R3 (the FAR) → `crash.json`; R4/R5 → Task 6; R6 (alias + flush) → Task 3 Step 3;
R7 → Task 2 Step 5 + Task 4 Step 3e; R8 → Task 4 Step 7.

**Placeholders.** Task 6 is the one section without concrete code, by design and stated as such
(the spec forbids inventing an unmeasured mechanism); every other step carries its content.
`EXPECT`'s two numbers and the two constants are set from a measurement the plan itself makes
(Task 2 Step 5) and says how to change if it disagrees with the prediction.

**Type consistency.** `decode_vm_remap` → `VmRemapReq { target, size, mask, flags, src_task,
src, copy, inheritance }` (Task 4) is what its arms read (`req.copy`, `req.flags`, `req.src_task`,
`req.target`, `req.size`, `req.src`). `encode_vm_remap_reply(reply_port: u32, target: u64, cur:
u32, max: u32)` (Task 4) matches both arms and the shape test. `guest_vm_remap(&mut self, target:
u64, size: u64, src: u64) -> u64` (Task 3) matches the unit test and both arms. Constants
`VM_REMAP_CUR_PROT`/`VM_REMAP_MAX_PROT: u32` (Task 4) are passed where `u32` is expected.
`CRASH_PY`/`CRASH_JSON`/`VMREMAP_DYN: &str` match their uses. `Event::Syscall`'s field set
`{ num, args, ret, ret1, err, writes, thread }` is M38's wire order.
