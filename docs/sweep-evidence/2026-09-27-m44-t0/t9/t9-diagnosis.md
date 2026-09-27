# T9 diagnosis: why lldb gets no frame #1 past `crashy`fstat` over `retrace gdbserver`

**Status: DONE.** Diagnosed to one server answer, and the diagnosis is backed by a measurement on a
patched throwaway build. No file under `crates/` or `docs/` was edited. All evidence is in
`t9diag/` (below, `$T` = `.superpowers/sdd/2026-09-27-retrace-m44-owed/t9diag`).

## The mechanism, in one paragraph

The server's `jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}` answer
(`rsp::image_json`, `crates/retrace/src/rsp.rs:242-286`) lists the exe's **`__PAGEZERO`** segment
(`"vmaddr":0,"vmsize":4294967296,…,"maxprot":0`). lldb's Darwin loader
(`DynamicLoaderDarwin::UpdateImageLoadAddress`) turns a maxprot-0 `__PAGEZERO` segment into
`Process::AddInvalidMemoryRegion([0, 0x1_0000_0000))`. lldb's memory cache then **fails every read
below 4 GiB locally, without sending a packet** (`memory read failed for 0x…`). On real macOS nothing
lives there. In a retrace guest the main thread's stack does: `DYN_STACK_TOP = 0x0280_0000`
(`crates/retrace-box/src/lib.rs:98`), so sp is `0x27ff770` and fp is `0x27ff7b0`. Frame 0 (the stub)
unwinds from live registers (pc from lr), so frame #1 (`main + 60`) is found at first. But frame
#2's pc is the saved lr at `[fp + 8] = 0x27ff7b8`, and that read fails. lldb then tries frame 0's
arch-default fallback plan, which reads the same `0x27ff7b8` and fails too. A failed
`TryFallbackUnwindPlan` leaves frame 0's register-location cache pointing at that memory slot. The
walk is re-run and frame #1's pc now comes from that dead slot: `could not get pc value`, and frame
#1 is dropped as well. That is why `bt` has only frame #0. `thread step-inst-over`, `thread step-out`
and `finish` all need frame #1 (the return address), so they degrade exactly as Task 9 measured.

## Commands (all bounded: `perl -e 'alarm 120; exec @ARGV'`, all `</dev/null`)

Setup, as the brief says:
```
cargo build -p retrace ; cargo build -p retrace-guest          # worktree, HEAD 5d1945c (no-op builds)
cp target/aarch64-apple-darwin/debug/retrace                                  $T/retrace
cp target/aarch64-apple-darwin/debug/build/retrace-guest-803c6b68f3336105/out/crashy $T/crashy
codesign -s - -f --entitlements retrace.entitlements $T/retrace               # as util::bin()
$T/retrace record-dyn $T/crashy -o $T/crashy.bin        # ABSOLUTE argv0, like util::record_dynamic; exit 139
$T/retrace record-dyn ./crashy  -o $T/crashy-rel.bin    # relative argv0, for the no-path control; exit 139
```
- **Native:** `/usr/bin/lldb -x -b -s $T/native.cmds` (`target create crashy; b main; run;
  breakpoint set -a 0x100000530; continue; log enable -v -f native-unwind.log lldb unwind;
  thread step-inst; bt; …`). Output is in `native.out`, the log in `native-unwind.log`.
- **Remote:** `$T/run-remote.sh <tag> <retrace> <trace> <body> [pre]`. It starts
  `<retrace> gdbserver <trace> --port 0`, reads the port from stderr, and writes
  `gdb-remote 127.0.0.1:<port>` + `command script import …/crates/retrace/lldb/retrace.py` + body
  + `script print("END")`. Then it runs `/usr/bin/lldb -x -b -s <tag>.cmds </dev/null`, as
  `lldb_e2e::session` does. Runs:
  - `r1`: `remote.body`. It sets the packet log, sets the breakpoint, continues, turns on
    `log enable -v … lldb unwind`, then runs `thread step-inst` and `bt`.
  - `r2`: `remote2.body`. It runs the memory-read probes, then `bt`.
  - `head-{ni,stepout,finish}` / `patched-{ni,stepout,finish}`: `run-rows.sh`. These are the
    command sequences of the two `#[ignore]`d rows.
  - `head-nopath`: `crashy-rel.bin`, `target create crashy` before the connect, and `target
    modules load --file crashy --slide 0`.
  - `head-nomodule`: `crashy-rel.bin` with no `target create`.

## Decisive unwind-log lines

**Native** (`native-unwind.log`, the walk after `thread step-inst`):
```
 68: th1/fr0 frame uses EmulateInstructionARM64 for full UnwindPlan because this is the non-call site unwind plan and this is a zeroth frame
 79: th1/fr0 supplying caller's register pc (32), saved in register lr (30)
 85:  th1/fr1 pc = 0x100000534
 95:  th1/fr1 Using full unwind plan 'compact unwind info'
110:  th1/fr1 supplying caller's register pc (32) from the stack, saved at CFA plus offset -8 [saved at 0x16fdfe3c8]
112:   th1/fr2 pc = 0x181a33e00
```
**Remote, HEAD** (`r1.unwind`). Frame 0's plan and frame 1's first pass are **identical** to native:
```
 51:  th1/fr1 pc = 0x100000534
 61:  th1/fr1 Using full unwind plan 'compact unwind info'
 84:  th1/fr1 supplying caller's register pc (32) from the stack, saved at CFA plus offset -8 [saved at 0x27ff7b8]
 85:   th1/fr2 could not get pc value
 86:   Frame 2 invalid RegisterContext for this frame, stopping stack walk
 91: th1/fr0 supplying caller's register pc (32) from the stack, saved at CFA plus offset -8 [saved at 0x27ff7b8]
 92: th1/fr0 failed to get a pc value for the caller frame with the fallback unwind plan
 99:  th1/fr1 could not get pc value
100:  Frame 1 invalid RegisterContext for this frame, stopping stack walk
```
(`th1/fr{1,2} This is an async frame`, at lines 9 and 82, appears only in HEAD runs. It is lldb's
reaction to the unreadable pc and not a cause: the patched run's log `p2.unwind` has no such line.)

So the stub is not the problem. Frame 0's plan (assembly inspection, `CFA=sp+0, lr=<same>`) is the
same on both sides, and lldb finds `main + 60` on both. What differs is the **read of the stack at
`0x27ff7b8`**.

## The packets that matter

- `r1.packets` line 124/125: `jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}` →
  `…"segments":[{"name":"__PAGEZERO","vmaddr":0,"vmsize":4294967296,"fileoff":0,"filesize":0,"maxprot":0},{"name":"__TEXT",…`.
- **No `m`/`x` packet for any address below 4 GiB is ever sent by lldb** in `r1` or `r2`. The only
  `m27ff…` in `r2.packets` (line 160) is the one sent by hand with
  `process plugin packet send m27ff7b0,10`. The server answered it correctly:
  `10fe7f0200000000003e138001000000` (saved fp `0x27ffe10`, saved lr `0x180133e00`).
- In the same session (`r2.out`), lldb's own reads fail locally:
  ```
  error: memory read failed for 0x27ff7b0                                  (memory read -c 16 0x27ff7b0)
  SB read [0x27ff7b8]: 0x0 error: memory read failed for 0x27ff7b8
  SB read [0xfffffff8]: 0x0 error: memory read failed for 0xfffffff8        (just under 4 GiB)
  SB read [0x100000000]: 0xfeedfacf error: None                              (at 4 GiB: goes out as m100000000,200)
  ```
- Not the cause, though checked. `qMemoryRegionInfo` is answered empty (sent once, for
  `0x1400049c0` at connect), and the patched run still answers it empty and works. `qShlibInfoAddr`
  is empty on both builds. target.xml's `generic` tags and the expedited 29–33 are fine, because
  frame 1's pc and fp come out right in both logs. The stub's bytes are read (`m100000400,200`,
  `m100000600,200`).
- **HEAD `ni`/`step-out`/`finish`** (`head-*.packets`): after the breakpoint there is one
  `vCont;s:1` and **no `Z0,100000534`**. `step-out` and `finish` fail with
  `error: Could not create return address breakpoint.`, which reproduces Task 9.

## Controls that isolate the answer

1. **No-path route on the unmodified HEAD server** (`head-nopath.out`). With no image JSON (relative
   argv0), `target create` + `target modules load --slide 0`, the loader never sees `__PAGEZERO`.
   `bt` shows `#0 crashy`fstat`, `#1 crashy`main + 60`, `#2 0x180133e00`, and `thread step-out`
   stops at `pc = 0x100000534`. Same server binary, same stack addresses, and the only difference is
   that no `__PAGEZERO` segment reaches lldb's loader.
2. **No module at all** (`head-nomodule.out`, which is t0 M5(iv)'s relative-argv0 case). The stack
   read succeeds (`SB read [0x27ff7b8]: 0x30 error: None`). But with no module, frame 0 at the stub
   takes the arch-default plan (`CFA=fp+16`), which is wrong at a function's first instruction, so
   `bt` = `#0 0x1000005d8, #1 0x180133e00` and `main` is skipped. That is a different mechanism, and
   it is why M5(iv)'s relative half also saw `next` land on the stub. It is not the configuration
   the tests use (they record with an absolute argv0).

## The throwaway patch (hypothesis → measurement)

`git archive HEAD` → `$T/src`, patched there, built there (`$T/src/target`). The worktree was not
touched. The diff (`$T/nopagezero.patch`):
```diff
--- a/crates/retrace/src/rsp.rs
+++ b/…/t9diag/src/crates/retrace/src/rsp.rs
@@ -272,7 +272,10 @@ pub(crate) fn image_json(hdr: &[u8], load_address: u64, path: &str) -> Result<St
     let text = segs.iter().find(|s| s.name == "__TEXT").ok_or("no __TEXT segment")?;
     let slide = load_address.wrapping_sub(text.vmaddr);
     let uuid = uuid.ok_or("no LC_UUID")?;
-    let seg_json: Vec<String> = segs.iter().map(|s| {
+    // T9DIAG: omit __PAGEZERO. …
+    let seg_json: Vec<String> = segs.iter().filter(|s| s.name != "__PAGEZERO").map(|s| {
         let vmaddr = if s.name == "__PAGEZERO" { s.vmaddr } else { s.vmaddr.wrapping_add(slide) };
```
(The real change would also delete the now-dead `if s.name == "__PAGEZERO"` branch on the next line.)

Signed copy: `$T/retrace-nopagezero`. Same recording `crashy.bin`, same scripts.

**Before (HEAD, `r2.out`)** vs **after (patched, `p2.out`)**, after `thread step-inst` into the stub:
```
HEAD:    * frame #0: 0x00000001000005d8 crashy`fstat
PATCHED: * frame #0: 0x00000001000005d8 crashy`fstat
           frame #1: 0x0000000100000534 crashy`main + 60
           frame #2: 0x0000000180133e00
```
(Frame #2 is `dyld`start` unsymbolicated, because dyld is deliberately never listed, per Ruling R5.)
In the patched session, `memory read -c 16 0x27ff7b0` prints the bytes, and lldb itself now sends
`m27ff600,200` and `m27ffe00,200`.

**The routed rows' commands** (`rows.log`):

| row | HEAD | patched |
|---|---|---|
| `thread step-inst-over` at the `bl` | `pc = 0x1000005d8` (stub); wire `vCont;s:1` only | `pc = 0x100000534`; wire `vCont;s:1`, `Z0,100000534,4`, `c`, `z0,100000534,4` (t0 L5's shape) |
| `thread step-inst; thread step-out` | `error: Could not create return address breakpoint.` exit 1 | `stop reason = step out`, `pc = 0x100000534`, exit 0, END |
| `thread step-inst; finish` | same error, exit 1 | `stop reason = step out`, `pc = 0x100000534`, exit 0, END |

**The real test target on the patched tree:**
`cargo test -p retrace --test lldb_e2e --no-fail-fast -- --include-ignored --test-threads=1` (in
`$T/src`, log `$T/patched-lldb_e2e.log`) gave **9 passed, 1 failed**:
- `lldb_steps_over_a_call_with_ni` **ok**, and `lldb_steps_out_of_a_call_with_step_out_and_finish`
  **ok**. Both routed rows pass unmodified.
- Every other pre-existing row is ok, including crashy's reverse-debug row, determinism, and CPython.
- **`lldb_steps_over_a_call_with_next` FAILED**: `left: Some(4294968628)` (`0x100000534`, bl + 4)
  vs `right: Some(4294968792)` (`0x1000005d8`, the pinned target). This is a **consequence, not a
  regression**. The native control (`native-next.out`: `next` at the `bl` → `pc = 0x100000534`
  `crashy`main + 60`, and the same for `thread step-inst-over` and for step-inst + step-out) shows
  that bl + 4 is what lldb does when it can unwind. t0 M5(iv)'s "`next` follows the call" was
  measured **under this same wall** (absolute half) and under the no-module case (relative half,
  control 2). The row's comment calls it "lldb's behaviour, not the server's". It was the server's.

**Other targets on the patched tree.** `cargo test -p retrace --test gdbserver_e2e --no-fail-fast --
--test-threads=1` gave **30 passed, 0 failed** (`$T/patched-gdbserver_e2e.log`).
`cargo test -p retrace --bins --no-fail-fast -- --test-threads=1` gave **32 passed, 0 failed**
(`$T/patched-bins.log`). No existing wire or unit assertion depends on `__PAGEZERO` being listed.
That also means none would catch the fix being reverted, so the new pin below is owed.

## Verdict: **SMALL**

**The server change** is one answer, a two-line edit in `rsp::image_json`
(`crates/retrace/src/rsp.rs:275-276`): leave `__PAGEZERO` out of the `segments` array, and delete
the dead `__PAGEZERO` vmaddr branch. A comment at the site should say why: in a retrace guest,
`[0, 4 GiB)` is not unmapped (the main stack at `DYN_STACK_TOP = 0x280_0000`, `TSD_IPA`, the
trampoline), and lldb turns that segment into an invalid region that no packet can reach. Nothing
else in the server changes, nor in `Exec`, the trace, or the box.

(The alternative, moving the guest's main stack above 4 GiB, is **not** small. That layout is
M8/M21's measured geometry around libpthread's `0x7fc000`, and it is a snapshot-meaning change.
Omitting the segment is also what the no-path route already does, and it works (control 1).)

**Pins:**
- **gdbserver_e2e (wire):** extend `the_exe_is_listed_from_argv0_when_the_recording_has_one`, or add
  a sibling row such as `the_image_list_leaves_out_pagezero_so_lldb_can_read_the_stack_below_4_gib`,
  that sends `jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}` on `crashy()` and asserts
  `!j.contains("__PAGEZERO")` and `!j.contains(r#""maxprot":0"#)`, while `__TEXT` is still at
  `"vmaddr":4294967296`. It could also send `m27ff7b0,10`-style reads, but those already work at
  HEAD. Only the JSON is the difference this fix makes. The matching unit assert goes in
  `rsp.rs::the_image_list_describes_a_real_binary_at_its_load_address` (`--bins` chunk).
- **lldb_e2e (behaviour, the real pin):** un-`#[ignore]` `lldb_steps_over_a_call_with_ni` and
  `lldb_steps_out_of_a_call_with_step_out_and_finish`, which pass unmodified on the patched tree. A
  `bt`-has-`main + 60` assert after `thread step-inst` would pin the mechanism most directly.
- **Needs a ruling:** `lldb_steps_over_a_call_with_next` (added in 5d1945c) must change its expected
  pc from `target` to `bl + 4`, matching native lldb. It is **not** one of the three rows that
  global-constraints lets change an assertion, so the controller or operator must authorize it. Its
  comment's claim about t0 M5(iv) must be corrected as well.
- **Control for the fix task:** with the filter removed on the committed tree, the wire row fails on
  `__PAGEZERO`, and the two un-ignored lldb rows fail as in Task 9.
