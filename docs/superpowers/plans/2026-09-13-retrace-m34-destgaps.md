# M34-destgaps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `proc_info` (336) and `csops`/`csops_audittoken` (169/170) `Dest` rows in
`arg_kinds` so their forwarded length is clamped to the guest backing and their diff window covers
what the kernel may write; cite the kernel bound that keeps `getattrlist`/`fgetattrlist` (220/228)
`Ptr`; pin all four decisions with tests that go red under the spec's mutations; commit the census
instrument; and discharge the README's "three still get a flat 64 KiB".

**Architecture:** Three row edits and four comment rewrites in `retrace-arch`'s `arg_kinds`,
consumed unchanged by `Box_::diff_window` and the `DestLen::Reg` clamp arm of `forward_and_diff`.
Two tests in `retrace-box/tests/truncguard.rs`: one through `diff_window_for_test` (the M29 seam —
window widening is a pure function of the table and the args), one through `forward_and_diff`'s
return value (the clamp has no seam; `proc_listpids` returns exactly the byte count it was allowed).
Three `EXPECTED_DIFFS` entries keep M33's equivalence sweep honest. No trap arm, no format change.

**Tech Stack:** Rust 1.95.0, `aarch64-apple-darwin`, Hypervisor.framework, POSIX `sh` and
Python 3 for the census tool. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-13-retrace-m34-destgaps-design.md`

## Global Constraints

- **`--test-threads=1` is mandatory** on every `cargo test` invocation (one VM per process).
- **Target is pinned** by `.cargo/config.toml`; do not override it or the codesigning `runner`.
- **The rows are spec §3a, verbatim.** 336 → `[Scalar, Scalar, Scalar, Scalar, Dest(Reg(5)), Scalar]`;
  169 → `[Scalar, Scalar, Dest(Reg(3)), Scalar]`; 170 → `[Scalar, Scalar, Dest(Reg(3)), Scalar, Ptr]`;
  220 and 228 **unchanged** (Ruling 1). `Reg`, never `DerefU64`; clamp, never refuse (spec §3a).
- **Every row comment names its bound and its citation** (the `Ptr` rule in `ArgKind`'s doc).
  The citations are in spec §3b; copy them, do not paraphrase from memory.
- **The measurement is already taken** (spec §4, 2026-09-13) and the numbers in this plan come
  from it. Do not re-derive a length from a man page; if a number here disagrees with what you
  find, stop and say so — do not pick one silently.
- **No new `#[ignore]`. No `TRACE_MAGIC` bump.** Either is a charter §5 halt: stop with the branch
  intact and a written explanation.
- **Do not push.** Commit and merge locally only.
- **No fix for spec §4b** (the pid-collision probe defect). It is recorded and routed; touching
  `forward_and_diff`'s probe loop is out of scope for every task below.
- Run targeted tests only in Tasks 1–3; the full chunked gate runs once, in Task 4, **in the
  controller, in the background**, never inside a subagent (a multi-minute blocking run is where
  subagents die).
- Write each task's report to `.superpowers/sdd/2026-09-13-retrace-m34-destgaps/task-N-report.md`
  **before** messaging the controller; the file is the deliverable of record.
- Branch: a fresh worktree on `m34-destgaps` from `main` at `dfc12ff` or later (the spec commit).

---

### Task 1: Commit the census instrument

The spec's §4 measurement was run from the scratchpad; this task makes it a repo tool so the next
milestone that needs the census re-runs it instead of rediscovering M33's lost outputs. The full
corpus run takes ~10 minutes and was already done; this task **smoke-tests** the committed copy on
three binaries and records that it matches the spec's numbers for them.

**Files:**
- Create: `tools/destgaps-census.sh` (from `/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/45f91304-acc4-4973-8088-27aa1053f270/scratchpad/destgaps-census.sh`)
- Create: `tools/destgaps-census-summary.py` (from `.../scratchpad/destgaps-analyze.py`)

**Interfaces:**
- Produces: `tools/destgaps-census.sh <retrace-binary> <guest-out-dir> <out.tsv>` and
  `tools/destgaps-census-summary.py <out.tsv>`; nothing later depends on them at build or test time.

- [ ] **Step 1: Copy both files in and fix the one scratchpad-ism**

```sh
cp /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/45f91304-acc4-4973-8088-27aa1053f270/scratchpad/destgaps-census.sh tools/destgaps-census.sh
cp /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/45f91304-acc4-4973-8088-27aa1053f270/scratchpad/destgaps-analyze.py tools/destgaps-census-summary.py
chmod +x tools/destgaps-census.sh tools/destgaps-census-summary.py
```

In `tools/destgaps-census.sh`, delete the line
`[ -d "$ROOT/tools" ] || ROOT=/Users/noahmitchem/Documents/GitHub/retrace` — inside `tools/`,
`$(dirname "$0")/..` is the repo root, exactly as `tools/apple-sweep.sh` computes it. Leave every
other line, including the header comment about the 5 GB stderr flood — that comment is the reason
the script streams through `head`, and the next person to "simplify" it needs to read why.

In `tools/destgaps-census-summary.py`, change the docstring's first line to
`"""Summarise tools/destgaps-census.sh output: per syscall, the length operand's distribution, …`
so the two files name each other.

- [ ] **Step 2: Syntax-check both**

Run: `sh -n tools/destgaps-census.sh && python3 -m py_compile tools/destgaps-census-summary.py && echo OK`
Expected: `OK`

- [ ] **Step 3: Smoke-run the committed script on three Apple binaries**

```sh
printf '/bin/echo\n/bin/ls\n/usr/bin/yes\n' > /tmp/m34-smoke.txt
G=$(ls -dt target/aarch64-apple-darwin/debug/build/retrace-guest-*/out | head -1)
cargo build -p retrace 2>&1 | tail -1
RETRACE_CENSUS_ONLY_APPLE=1 RETRACE_SWEEP_LIST=/tmp/m34-smoke.txt \
  tools/destgaps-census.sh target/aarch64-apple-darwin/debug/retrace "$G" /tmp/m34-smoke.tsv
python3 tools/destgaps-census-summary.py /tmp/m34-smoke.tsv
```

Expected progress lines: three `apple:… traps=… matched=10` lines, `/usr/bin/yes`'s ending in
`CAPPED` (it floods; the cap is the point), then `DONE matched_total=30`. Expected summary: every
`max=` line says `fits 64 KiB`; `proc_info` shows `len=0x170`, `0x40`, `0x38`; `csops` shows
`len=0x4` and (for `/bin/ls` or `/bin/echo`, whichever carries op `0x10`) `0x408`. If `yes` does
**not** say `CAPPED` or the run takes more than ~2 minutes, the streaming filter is not working —
stop and report; do not raise the timeout.

- [ ] **Step 4: Commit**

```sh
git add tools/destgaps-census.sh tools/destgaps-census-summary.py
git commit -m "M34 t1: commit the destgaps census instrument (streams RETRACE_TRACE through a line cap)"
```

Report: the three progress lines and the summary output, verbatim, in `task-1-report.md`.

---

### Task 2: The rows, the window test, and the equivalence ledger

TDD: the window test is written first and goes red on the three rows that are still `Ptr` (and
green on the two that must stay `Ptr`); the rows are then edited; the equivalence sweep goes red
until its three `EXPECTED_DIFFS` entries are added. Positive controls 1 and 2 close the task.

**Files:**
- Modify: `crates/retrace-box/tests/truncguard.rs` — add one test after
  `the_window_widens_for_each_m29_reg_addition` (line ~156–195)
- Modify: `crates/retrace-arch/src/lib.rs` — rows at lines 503 (228), 706 (220), 764–765
  (169/170), 786 (336), and the `Dest` doc paragraph at lines 234–241
- Modify: `crates/retrace-arch/tests/legacy_equivalence.rs` — append three entries to
  `EXPECTED_DIFFS` (after the `(550, …)` entry, line ~126)

**Interfaces:**
- Consumes: `Box_::diff_window_for_test(num, i, avail, &args) -> usize`
  (`crates/retrace-box/src/lib.rs:3054`), `retrace_arch::SYS_FGETATTRLIST` (= 228).
- Produces: `arg_kinds(336).dest_buffer() == Some((4, DestLen::Reg(5)))`,
  `arg_kinds(169).dest_buffer() == Some((2, DestLen::Reg(3)))`, same for 170; Task 3 relies on
  336's.

- [ ] **Step 1: Write the failing window test**

Append to `crates/retrace-box/tests/truncguard.rs`, directly after
`the_window_widens_for_each_m29_reg_addition`:

```rust
// M34: two rows join the M29 four, and one pair is pinned as NOT joining. Same seam and same
// reasoning as the test above — `diff_window` is a pure function of the table and the args, so
// this proves the widening without a guest that calls each syscall with a huge buffer.
//
// The negative half is the milestone's Ruling 1 made executable. `getattrlist` and
// `fgetattrlist` are kernel-bounded: both reach `getattrlist_internal` → `getvolattrlist` /
// `vfs_attr_pack_internal`, and each packer rejects with ENOMEM before any copyout when the
// packed result exceeds `attr_max_buffer` (ATTR_MAX_BUFFER_LONGPATHS, 15,360 bytes;
// bsd/vfs/vfs_attrlist.c). By the `ArgKind` doc's own rule that is a `Ptr` with a citation, not
// a `Dest`. A later "completion" of M34 that makes them `Dest` fails here and is sent to the
// citation. Corpus maximum for either, measured 2026-09-13: 1,052 bytes.
#[test]
fn the_window_widens_for_the_m34_rows_and_not_for_getattrlist() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let b = Box_::load(&loaded);
    const AVAIL: usize = 1 << 20;
    const FLAT: usize = 64 * 1024; // PTR_WINDOW_CAP

    let mut args = [0u64; 8];

    args[5] = 200_000; // proc_info buffersize (uint32_t, x5)
    assert_eq!(b.diff_window_for_test(336, 4, AVAIL, &args), 200_000,
        "proc_info's destination is x4 and its length x5");
    assert_eq!(b.diff_window_for_test(336, 5, AVAIL, &args), FLAT,
        "x5 is proc_info's length, not its buffer");

    args[3] = 150_000; // csops usersize (x3)
    assert_eq!(b.diff_window_for_test(169, 2, AVAIL, &args), 150_000,
        "csops's destination is x2 and its length x3");
    assert_eq!(b.diff_window_for_test(170, 2, AVAIL, &args), 150_000,
        "csops_audittoken shares csops's destination and length");
    assert_eq!(b.diff_window_for_test(170, 4, AVAIL, &args), FLAT,
        "x4 is csops_audittoken's audit token — a 32-byte copyin, not a destination");

    // Ruling 1: `args[3]` is now also a 150,000-byte `bufferSize`, and the window must NOT follow it.
    assert_eq!(b.diff_window_for_test(220, 2, AVAIL, &args), FLAT,
        "getattrlist is kernel-bounded at 15,360 bytes (ATTR_MAX_BUFFER_LONGPATHS) and stays Ptr");
    assert_eq!(b.diff_window_for_test(retrace_arch::SYS_FGETATTRLIST, 2, AVAIL, &args), FLAT,
        "fgetattrlist shares getattrlist's packers and their bound, and stays Ptr");
}
```

- [ ] **Step 2: Run it and confirm it fails on exactly the three new rows**

Run: `cargo test -p retrace-box --test truncguard the_window_widens_for_the_m34_rows -- --test-threads=1 2>&1 | tail -15`
Expected: FAIL at the **first** assertion, `proc_info's destination is x4 and its length x5`,
with `left: 65536, right: 200000`. (A failure anywhere else means the seam or the constants are
not what this plan says — stop and report.)

- [ ] **Step 3: Edit the five rows' comments and the three rows' shapes**

In `crates/retrace-arch/src/lib.rs`:

**(a)** Replace the `fgetattrlist` comment + row (lines 501–503, the row is
`SYS_FGETATTRLIST => row!(P, [Fd, Ptr, Ptr, Scalar, Scalar]),`) with:

```rust
        // fgetattrlist(int fd, struct attrlist *alist, void *attrbuf, size_t bufsize, u_long opts):
        // alist is a fixed 24-byte struct (sys/attr.h, sizeof). attrbuf is a destination the KERNEL
        // bounds, not the caller: it reaches `getattrlist_internal` → `getvolattrlist` /
        // `vfs_attr_pack_internal`, and each packer rejects with ENOMEM BEFORE any copyout when
        // the packed result exceeds `attr_max_buffer` — ATTR_MAX_BUFFER_LONGPATHS = 8192 − 1024 +
        // 8192 = 15,360 (sys/attr.h; bsd/vfs/vfs_attrlist.c, the `ab.allocated > attr_max_buffer`
        // gates and the copy `lmin(buf_size, ab.allocated)`). The cited bound, four times inside
        // the window, so Ptr by the rule above — M34 Ruling 1, pinned by
        // `truncguard::the_window_widens_for_the_m34_rows_and_not_for_getattrlist`. Corpus
        // maximum measured 2026-09-13: 40 bytes.
        SYS_FGETATTRLIST => row!(P, [Fd, Ptr, Ptr, Scalar, Scalar]),
```

**(b)** Replace the `getattrlist` comment + row (lines 702–706, ending
`220 => row!(P, [Path, Ptr, Ptr, Scalar, Scalar]),`) with:

```rust
        // getattrlist(const char *path, struct attrlist *alist, void *attributeBuffer, size_t
        //             bufferSize, u_long options): alist is a fixed 24-byte struct (sys/attr.h,
        // sizeof). attributeBuffer is kernel-bounded at ATTR_MAX_BUFFER_LONGPATHS (15,360) exactly
        // as its `fgetattrlist` sibling — same `getattrlist_internal`, same two packers, same
        // ENOMEM-before-copyout gate (bsd/vfs/vfs_attrlist.c) — so Ptr with the bound cited, not
        // Dest: M34 Ruling 1. Corpus maximum measured 2026-09-13: 1,052 bytes (CPython).
        220 => row!(P, [Path, Ptr, Ptr, Scalar, Scalar]),
```

**(c)** Replace the `csops` comment + two rows (lines 759–765, from
`// csops(pid_t pid, …` through `170 => row!(P, [Scalar, Scalar, Ptr, Scalar, Ptr]),`) with:

```rust
        // csops(pid_t pid, uint32_t ops, void *useraddr, size_t usersize) / csops_audittoken(…,
        // audit_token_t *uaudittoken): `csops_internal` (bsd/kern/kern_proc.c) dispatches on ops
        // (sys/codesign.h). The blob ops — CS_OPS_ENTITLEMENTS_BLOB (7), CS_OPS_BLOB (10),
        // CS_OPS_DER_ENTITLEMENTS_BLOB (16), and IDENTITY/TEAMID (11/14) — copy out up to usersize
        // via `csops_copy_token` (an 8-byte header and ERANGE if usersize is short), and
        // CS_OPS_BLOB is the whole code-signing SuperBlob: one CodeDirectory hash per page of the
        // binary, so hundreds of KiB for a large one. No citable bound below the window → Dest,
        // length x3 (M34). The fixed-size ops write 4 bytes (CS_OPS_STATUS 0, with NO usersize
        // check; VALIDATION_CATEGORY 17), 8 (PIDOFFSET 6) or a struct whose size usersize must
        // equal (CDHASH 5, CDHASH_WITH_INFO 18) — all inside the 64 KiB floor `diff_window` keeps
        // under every Dest (`base.max(…)`), which is what makes a STATUS with usersize 0 still
        // fully captured; do not "fix" that floor away for Dest rows. Corpus, measured 2026-09-13:
        // op 0 with usersize 4 from every dynamic guest, op 16 with 1032 (169: two guests;
        // 170: every dynamic guest) — both rows inert for the window, live for the clamp. The
        // audit token (170, x4) is a fixed 32-byte copyin.
        169 => row!(P, [Scalar, Scalar, Dest(Reg(3)), Scalar]),
        170 => row!(P, [Scalar, Scalar, Dest(Reg(3)), Scalar, Ptr]),
```

**(d)** Replace the `proc_info` comment + row (lines 784–786, ending
`336 => row!(P, [Scalar, Scalar, Scalar, Scalar, Ptr, Scalar]),`) with:

```rust
        // proc_info(int32_t callnum, int32_t pid, uint32_t flavor, uint64_t arg, void *buffer,
        //           int32_t buffersize): `proc_info_internal` (bsd/kern/proc_info.c) dispatches on
        // callnum (sys/proc_info_private.h). LISTPIDS (1) copies out min(nprocs+20, buffersize/4)
        // pids; KERNMSGBUF (4) the message buffer up to buffersize; LISTCOALITIONS (11),
        // PIDDYNKQUEUEINFO (13), UDATA_INFO (14) lists bounded by buffersize and a count the
        // kernel owns — none with a citable constant below the window → Dest, length x5 (M34).
        // PIDINFO (2) / PIDFDINFO (3) / PIDFILEPORTINFO (6) / PIDORIGINATORINFO (10) copy out a
        // fixed struct after `if (buffersize < size) return ENOMEM`, so ≤ buffersize always. Two
        // callnums ride under Dest as an over-approximation (M34 Ruling 2): SETCONTROL (5) with
        // PROC_SELFSET_THREADNAME COPIES IN ≤ 63 bytes (MAXTHREADNAMESIZE − 1) — a Source shape
        // bounded 1000× inside the window, so no canary coverage is lost; SET_DYLD_IMAGES (15)
        // transfers nothing ("don't need to copyin the buffer. just setting the buffer range in
        // the task struct" — `proc_set_dyld_images`). For both, the clamp min(avail, buffersize)
        // fires only if the guest's buffer already overruns its own backing. Corpus, measured
        // 2026-09-13 (318 dispatches, every dynamic guest): callnum 2 flavors 13/17 with 64/56,
        // callnum 5 with 4 (the Rust guests naming `main`), callnum 15 with 368 from dyld at
        // pc 0x14000600c — all inside the window; inert for the window, live for the clamp.
        336 => row!(P, [Scalar, Scalar, Scalar, Scalar, Dest(Reg(5)), Scalar]),
```

**(e)** In the `ArgKind::Dest` doc comment, replace the paragraph that begins
`/// **Seeded only with what is measured or SDK-verified.**` (lines 234–241, ending
`/// measured", the guard band is what makes that safe, and M34 is to measure each (spec §7).`)
with:

```rust
    /// **Seeded only with what is measured or SDK-verified.** M29 added `getdirentries64`,
    /// `recvfrom` (both spellings) and `getfsstat64`/`sysctlbyname` (sysctl's own shape); each of
    /// those rows names its own second destination where it has one, so a later reader can see it
    /// was considered and dismissed on a number rather than overlooked. M34 measured the three
    /// M29 left as "structurally capable of overrunning": `proc_info` and `csops` joined (their
    /// blob/list callnums are bounded only by the caller's length), and `getattrlist`/
    /// `fgetattrlist` did NOT — the kernel caps them at 15,360 bytes before writing, which is the
    /// `Ptr` rule's case, and their rows cite it. After M34 no `Ptr` in this table means "not
    /// measured"; every one names its bound. The corpus maximum across all five, measured
    /// 2026-09-13, is 1,052 bytes: both new rows are inert for the window today and live for the
    /// clamp, which is the half that protects retrace's own process.
```

- [ ] **Step 4: Run the window test — green — and the arch tests — red on the sweep**

Run: `cargo test -p retrace-box --test truncguard the_window_widens_for_the_m34_rows -- --test-threads=1 2>&1 | tail -5`
Expected: `test result: ok. 1 passed`

Run: `cargo test -p retrace-arch -- --test-threads=1 2>&1 | grep -a -B2 -A12 'FAILED\|panicked' | head -40`
Expected: exactly one failure, `legacy_equivalence::every_view_reproduces_its_legacy_table`,
whose message lists `(336, DestBuffer)`, `(169, DestBuffer)`, `(170, DestBuffer)` as **unlisted**
differences and nothing else. (If `no_row_has_more_than_one_dest_argument_or_more_than_eight_arguments`
also fails, a row has two `Dest`s — re-read Step 3.)

- [ ] **Step 5: Add the three `EXPECTED_DIFFS` entries**

In `crates/retrace-arch/tests/legacy_equivalence.rs`, after the `(550, View::ReadsGuestBuffer, …)`
entry and before the closing `];`, append:

```rust
    // M34: the two `Dest` rows the charter's destgaps entry owed (its third, getattrlist, stayed
    // Ptr on a cited kernel bound — Ruling 1 — and so does not differ). Both exercised by every
    // dynamic guest in the census; both measured inert for the window on landing (corpus maximum
    // 368 and 1032 bytes against a 64 KiB flat window) and live for the forwarded-count clamp.
    (336, View::DestBuffer, "proc_info(callnum, pid, flavor, arg, buffer, buffersize): buffer is a Dest of x5 bytes — exercised (every dynamic guest; corpus max 368)"),
    (169, View::DestBuffer, "csops(pid, ops, useraddr, usersize): useraddr is a Dest of x3 bytes (CS_OPS_BLOB is unbounded below the window) — exercised (every dynamic guest; corpus max 1032)"),
    (170, View::DestBuffer, "csops_audittoken(…): csops's shape plus a 32-byte token copyin — exercised (every dynamic guest; corpus max 1032)"),
```

- [ ] **Step 6: Run the arch tests and clippy — green**

Run: `cargo test -p retrace-arch -- --test-threads=1 2>&1 | grep -a '^test result'`
Expected: every line `ok`, zero `failed`. (`census`'s two tests run twice — in their own binary and
`#[path]`-included by `legacy_equivalence` — so five results from three attributes there is
correct; see that file's header.)

Run: `cargo clippy -p retrace-arch -p retrace-box --all-targets -- -D warnings 2>&1 | tail -3`
Expected: `Finished`, no warnings.

- [ ] **Step 7: Positive control 1 — revert 336 to `Ptr`, watch two detectors go red, restore**

Change 336's row to `[Scalar, Scalar, Scalar, Scalar, Ptr, Scalar]` (row only; leave the comment).

Run: `cargo test -p retrace-box --test truncguard the_window_widens_for_the_m34_rows -- --test-threads=1 2>&1 | grep -a 'proc_info\|test result'`
Expected: FAIL, `proc_info's destination is x4 and its length x5`, `left: 65536, right: 200000`.

Run: `cargo test -p retrace-arch --test legacy_equivalence every_view -- --test-threads=1 2>&1 | grep -a 'stale\|test result'`
Expected: FAIL, `EXPECTED_DIFFS entries that no longer differ (stale — delete or explain): [(336, DestBuffer)]`.

Restore the row to `[Scalar, Scalar, Scalar, Scalar, Dest(Reg(5)), Scalar]`. Re-run both; both green.
Paste both red outputs into the report.

- [ ] **Step 8: Positive control 2 — make 220 a `Dest`, watch two detectors go red, restore**

Change 220's row to `[Path, Ptr, Dest(Reg(3)), Scalar, Scalar]` (row only).

Run: `cargo test -p retrace-box --test truncguard the_window_widens_for_the_m34_rows -- --test-threads=1 2>&1 | grep -a 'getattrlist is\|test result'`
Expected: FAIL, `getattrlist is kernel-bounded at 15,360 bytes … and stays Ptr`, `left: 150000, right: 65536`.

Run: `cargo test -p retrace-arch --test legacy_equivalence every_view -- --test-threads=1 2>&1 | grep -a 'unlisted\|no EXPECTED_DIFFS\|test result'`
Expected: FAIL, `views disagree with the legacy tables and no EXPECTED_DIFFS entry says why: [(220, DestBuffer)]`.

Restore the row to `[Path, Ptr, Ptr, Scalar, Scalar]`. Re-run both; both green. Paste both red
outputs into the report.

- [ ] **Step 9: Commit**

```sh
git add crates/retrace-arch/src/lib.rs crates/retrace-arch/tests/legacy_equivalence.rs crates/retrace-box/tests/truncguard.rs
git commit -m "M34 t2: Dest rows for proc_info and csops; getattrlist stays Ptr on a cited bound (Ruling 1)"
```

---

### Task 3: Positive control 3 — the clamp is seen to fire

**Files:**
- Modify: `crates/retrace-box/tests/truncguard.rs` — add one test after Task 2's

**Interfaces:**
- Consumes: `Box_::forward_and_diff(&mut self, num: u64, args: [u64; 8]) -> (u64, bool, Vec<Region>)`,
  `Box_::host_span_for_test(&self, ipa: u64) -> Option<(*mut u8, usize)>`,
  `retrace_box::STACK_TOP_IPA` (= `0x2_0000`), `Stop` (derives `Debug`).
- Produces: nothing downstream.

- [ ] **Step 1: Write the test**

Append to `crates/retrace-box/tests/truncguard.rs`, after
`the_window_widens_for_the_m34_rows_and_not_for_getattrlist`:

```rust
// M34 control 3: the forwarded-count clamp REACHES the new rows. `tests/clamp.rs` proves
// `clamp_count` as a pure function; nothing before this proved the `DestLen::Reg` arm is taken
// for a given row, and that arm is the half of `Dest` that is live on this corpus (every measured
// destination already fits the flat window — M34 spec §4). `hargs` is local to `forward_and_diff`,
// so the clamp is observed through the kernel's own return value, the way `memdiff.rs`'s
// `forward_and_diff_captures_a_read_larger_than_the_window` observes the window through `ret`.
//
// The call is `proc_info(PROC_INFO_CALL_LISTPIDS, PROC_ALL_PIDS, …)` and not a PIDINFO flavor,
// because LISTPIDS takes no pid: `forward_and_diff` rewrites ANY register whose value lands in a
// backing to a host pointer (spec §4b — a pid in 16384..=65535 hits the trampoline/page-table
// backings), and a control that could be failed by the recorder's pid would measure that defect
// instead of this arm. `proc_listpids` copies out `min(nprocs + 20, buffersize / 4)` pids and
// returns the byte count (bsd/kern/proc_info.c); every Mac runs far more than 16 processes, so
//   clamp taken   -> the kernel is handed buffersize = 64 and returns exactly 64, no error;
//   clamp skipped -> it is handed 4160 and either faults on the copyout (err, EFAULT) or writes
//                    past the 64-byte backing and returns more than 64.
// `(64, false)` is produced by the clamp and by nothing else.
#[test]
fn the_clamp_reaches_proc_info() {
    let loaded = retrace_guest::parse_macho(&std::fs::read(retrace_guest::HELLO).unwrap());
    let mut b = Box_::load(&loaded);
    // Run to the guest's first syscall stop. That call is not forwarded and the guest is never
    // resumed; the box only needs to be in a state where `forward_and_diff` may be called.
    match b.run() {
        Stop::Syscall { .. } => {}
        other => panic!("expected the guest's first syscall stop, got {other:?}"),
    }

    const AVAIL: u64 = 64;
    // The static stack backing is [STACK_TOP_IPA - GRANULE, STACK_TOP_IPA), so this destination
    // has exactly AVAIL bytes of backing behind it.
    let dest = retrace_box::STACK_TOP_IPA - AVAIL;
    let (_, avail) = b.host_span_for_test(dest).expect("the static stack backing ends at STACK_TOP_IPA");
    assert_eq!(avail as u64, AVAIL, "dest must sit exactly {AVAIL} bytes before the end of its backing");

    let args: [u64; 8] = [
        1,            // PROC_INFO_CALL_LISTPIDS
        1,            // PROC_ALL_PIDS
        0, 0,
        dest,         // buffer
        AVAIL + 4096, // buffersize: past the backing, so only the clamp can bring it to AVAIL
        0, 0,
    ];
    // The scalar arguments must not themselves land in a backing, or this test would be
    // measuring spec §4b's probe defect rather than the clamp.
    for &a in &[args[0], args[1], args[2], args[3], args[5]] {
        assert!(b.host_span_for_test(a).is_none(), "scalar {a:#x} collides with a guest backing");
    }

    let (ret, err, _writes) = b.forward_and_diff(336, args);
    assert_eq!((ret, err), (AVAIL, false),
        "proc_info(LISTPIDS) with buffersize {} into a {AVAIL}-byte backing: the clamp must hand \
         the kernel {AVAIL} and get {AVAIL} back; got ret={ret} err={err}", AVAIL + 4096);
}
```

- [ ] **Step 2: Run it — green (the rows landed in Task 2)**

Run: `cargo test -p retrace-box --test truncguard the_clamp_reaches_proc_info -- --test-threads=1 2>&1 | tail -5`
Expected: `test result: ok. 1 passed`. If it fails with `ret=3` (`ESRCH`), a scalar collided with a
backing despite the precondition — report the values; do not change the arguments to make it pass.

- [ ] **Step 3: Positive control 3 — revert 336 to `Ptr`, watch it go red, restore**

In `crates/retrace-arch/src/lib.rs`, change 336's row to
`[Scalar, Scalar, Scalar, Scalar, Ptr, Scalar]` (row only).

Run: `cargo test -p retrace-box --test truncguard the_clamp_reaches_proc_info -- --test-threads=1 2>&1 | grep -a 'got ret\|test result'`
Expected: FAIL, the message ending `got ret=<n> err=<b>` where `(n, b)` is **not** `(64, false)`
— record which of the two unclamped outcomes it was (`err=true` with `ret=14`, or `err=false`
with `ret > 64`). Either is the kernel being handed 4160.

Restore the row to `[Scalar, Scalar, Scalar, Scalar, Dest(Reg(5)), Scalar]`. Re-run; green. Paste
the red output into the report.

- [ ] **Step 4: Run the whole truncguard binary and clippy**

Run: `cargo test -p retrace-box --test truncguard -- --test-threads=1 2>&1 | grep -a '^test result'`
Expected: `ok`, `0 failed` (the count is the previous close's truncguard count + 2).

Run: `cargo clippy -p retrace-box --all-targets -- -D warnings 2>&1 | tail -2`
Expected: no warnings.

- [ ] **Step 5: Commit**

```sh
git add crates/retrace-box/tests/truncguard.rs
git commit -m "M34 t3: control 3 — the forwarded-count clamp is seen to fire on proc_info"
```

---

### Task 4: Docs, the sweep, the gate, the merge

The **edits** in this task are a subagent's; the **sweep, the gate and the merge are the
controller's**, run in the background (a full gate is ~50 minutes and a subagent blocking on it is
where subagents die). Steps 1–3 are the subagent's; 4–7 the controller's.

**Files:**
- Modify: `README.md:452–453` (the "Three still get a flat 64 KiB" sentence) and `README.md:602–604`
  (the owed-list entry)
- Modify: `docs/status-log.md` — append a `## Status: M34-destgaps` section at the end
- Modify: `docs/superpowers/specs/2026-09-13-retrace-m34-destgaps-design.md` §11

- [ ] **Step 1: README**

Replace, at `README.md:452–453`, the sentence
`**Three still get a flat 64 KiB**: `proc_info` (336); `getattrlist`/`fgetattrlist` (220/228); `csops` (169/170).`
with:

```markdown
**M34 closes that list**, by two mechanisms: `proc_info` (336) and `csops`/`csops_audittoken`
  (169/170) gained `Dest` rows — their blob and list callnums are bounded only by the caller's
  length, so the window now follows it and the forwarded count is clamped to the backing; and
  `getattrlist`/`fgetattrlist` (220/228) turned out not to belong on the list at all, because the
  kernel rejects with `ENOMEM` before writing anything when the packed result exceeds
  `ATTR_MAX_BUFFER_LONGPATHS` (15,360 bytes) — a cited bound four times inside the window, so they
  stay `Ptr` and a test pins them there. The corpus maximum across all five, measured 2026-09-13
  over 851 dispatches from 76 guests, is 1,052 bytes: both new rows are inert for the window today
  and live for the clamp. Measuring that also found a defect outside M34's scope, recorded in
  "Known limits" below (the pid-collision probe).
```

At `README.md:602–604`, delete the owed-list clause
`**M34's three `Dest` rows** (`proc_info`, `getattrlist`/`fgetattrlist`, `csops` — deliberately left `Ptr`, because each is a length measurement the charter assigns to M34);`
(keep the surrounding list intact — the next clause begins `**nested-pointer translation**`).

Then, in the same owed list, before the `**nested-pointer translation**` clause, insert:

```markdown
**the pid-collision probe** (M34 §4b: `forward_and_diff` rewrites *any* register whose value
  lands in a guest backing to a host pointer, and the trampoline/page-table backings occupy
  `[0x4000, 0x10000)` on the dynamic path, so whenever the recorder's pid is in 16384..=65535 —
  roughly half of them — every `csops` returns `ESRCH`, every `proc_info(PIDINFO)` returns `ESRCH`,
  and dyld's `SET_DYLD_IMAGES` returns `EINVAL`; measured on `hello_dyn` at pid `0x6a30`. Record
  and replay agree, so the oracle cannot see it; its fix is for `forward_and_diff` to skip
  `Scalar` positions, whose precondition is a `Scalar` audit of the whole table — its own
  milestone, and a hypothesis for the sweep's intermittent row that M36 is to test by recording
  the recorder's pid);
```

- [ ] **Step 2: Status log**

Append to `docs/status-log.md` (it is append-only; never edit an earlier section) a section in the
shape of the M33 one immediately above it — read that section first for the house style — with
these subsections and content:

```markdown
## Status: M34-destgaps — two `Dest` rows, one cited bound, and a corpus that reaches neither

**Merged to local `main` at `<merge sha>`** (filled in by the controller at merge). Spec:
`docs/superpowers/specs/2026-09-13-retrace-m34-destgaps-design.md`; plan:
`docs/superpowers/plans/2026-09-13-retrace-m34-destgaps.md`. Third milestone of the M32–M38
charter, run unattended under its §5.

### What it set out to do

[The charter's entry, quoted: the three uncovered `dest_buffer` syscalls, "each needs its
reply-length operand located and added".]

### Ruling 1 — re-scoped on xnu source

[Spec's Ruling 1 verbatim, then one paragraph: both entry points, both packers, the ENOMEM gate,
the 15,360 figure and its arithmetic, the copy bound; why the table's own `Ptr` rule makes this
the right row; what a `Dest` here would have falsely told a reader.]

### The measurement (spec §4)

[The corpus described; the 851/76/42 coverage figures; the §4 table reproduced; the raw-vs-libc
check; the plain statement that the corpus maximum is 1,052 bytes, so the window half is inert
and the clamp half is the value.]

### What changed

[Rows 336/169/170 with their new shapes; 220/228 unchanged with the bound cited; the `Dest` doc
paragraph; three `EXPECTED_DIFFS` entries; two truncguard tests; `tools/destgaps-census.sh` and
its summary, with the 5 GB stderr lesson; the README sentence and owed-list entries.]

### Positive controls, run and recorded

[Controls 1–3 from Task 2 Steps 7–8 and Task 3 Step 3: the mutation, the red output's key line,
which test(s) caught it. Control 3's outcome under the mutation — EFAULT or an over-long return —
stated as measured.]

### A finding outside scope: the pid-collision probe (spec §4b, Ruling 3)

[The `hello_dyn` table of returns; the located cause with file:line and symbol; the backing
ranges and the pid range they cover; why record and replay agree; why it is not fixed here (M33
§7's unverified `Scalar` classification); the fix shape and its positive control; the M36 routing
with the concrete instruction to record the recorder's pid per sweep row.]

### The sweep

[The tally from Step 4 below, verbatim, beside M33's `pass=46 fail=8 skip=0`, and whether it
moved — spec §10 predicted unmoved.]

### Gate

[From Step 5 below: the chunked invocations, cargo exit codes captured before any pipe, the
per-chunk `test result` lines, the total against M33's 570/0/2 over 124, and the file-by-file
`#[test]` reconciliation table showing `truncguard.rs` +2 and every other file unchanged.]

### What stays owed

- The §4b pid-collision probe — a fix milestone with a `Scalar` audit as its task 1; M36 to
  record the recorder's pid beside each sweep row.
- A `bigcsops`-shaped guest, only if a later milestone finds a real guest whose `CS_OPS_BLOB`
  exceeds 64 KiB; none in the corpus does.
- Everything M33 left owed and M34 did not touch: the per-argument canary fill, nested-pointer
  translation, `pipe`'s return, the `execve`/`posix_spawn` assert (Ruling 7, operator's), console
  `writev`, `__disable_threadsignal`, `AT_FDCWD` (M33 Ruling 10), M35's two holes.
```

Fill every bracketed item from the spec, the task reports and the controller's Step 4–5 numbers;
leave no brackets in the committed text. The section may cite the spec but must be readable
without it (the log is the history; the spec is the design).

- [ ] **Step 3: Spec §11 and commit**

Replace `*(Written at close.)*` in the spec's §11 with three short paragraphs: what landed as
designed; what the controls showed (control 3's measured unclamped outcome in particular); the
gate and sweep figures. Then:

```sh
git add README.md docs/status-log.md docs/superpowers/specs/2026-09-13-retrace-m34-destgaps-design.md
git commit -m "M34 t4: close the milestone — README, status-log section, spec outcome"
```

(The merge sha in the status-log header is filled by the controller in Step 7 with a one-line
follow-up commit, as M33 did.)

- [ ] **Step 4 (controller): The Apple sweep, once, in the background**

```sh
cargo build -p retrace 2>&1 | tail -1
tools/apple-sweep.sh > .superpowers/sdd/2026-09-13-retrace-m34-destgaps/sweep.log 2>&1
```

Expected: the closing `TALLY pass=46 fail=8 skip=0` (spec §10 predicts unmoved). A different
tally is not a halt by itself — record it and the per-binary lines that changed; a *regression*
(a previously passing binary now failing) after one diagnose-edit-rerun cycle is the charter's
red-gate halt.

- [ ] **Step 5 (controller): Predict, then run, the chunked gate**

Predict first, from source:
```sh
for f in $(git diff --name-only main...HEAD -- 'crates/*'); do
  printf '%s main=%s branch=%s\n' "$f" \
    "$(git show main:$f 2>/dev/null | grep -c '^\s*#\[test\]')" \
    "$(grep -c '^\s*#\[test\]' $f)"
done
```
Expected: only `crates/retrace-box/tests/truncguard.rs` changes, `+2`; prediction **572 / 0 / 2
over 124**.

Then run, each chunk in the background with cargo's exit code captured **before any pipe**, logs
sanitised before parsing (`LC_ALL=C tr -cd '\11\12\15\40-\176' | sed 's/\x1b\[[0-9;]*m//g'`):

```sh
cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1
cargo test -p retrace-box --no-fail-fast -- --test-threads=1
cargo test -p retrace-box --doc
cargo test -p retrace --test <name> --no-fail-fast -- --test-threads=1   # per e2e target, chunked index-free
cargo test -p retrace --bins --no-fail-fast -- --test-threads=1          # never omit
```

Reconcile the `^test result:` lines against the prediction and against M33's per-file counts.
`cargo clippy --workspace --all-targets -- -D warnings` clean.

- [ ] **Step 6 (controller): Fill the status-log's gate and sweep subsections, commit on the branch**

- [ ] **Step 7 (controller): Merge to local `main`, no push**

```sh
git checkout main && git merge --no-ff m34-destgaps -m "Merge M34-destgaps: two Dest rows, one cited bound, and a corpus that reaches neither"
```
Then the one-line follow-up that writes the merge sha into the status-log header, and prove tree
identity rather than re-running the gate in the main checkout (`git rev-parse main^{tree}` equals
the merge's tree by construction; run one headline test — `cargo test -p retrace-box --test
truncguard -- --test-threads=1` — in `main` for direct evidence).

---

## Self-Review

**Spec coverage.** §3a rows → Task 2 Step 3 (c)(d); 220/228 unchanged → Step 3 (a)(b). §3b
citations → the row comments carry them verbatim. §4 measurement → already taken; Task 1 commits
the instrument and smoke-tests it. §4b → not fixed (constraint), recorded in README/status-log
(Task 4 Steps 1–2) and routed. §5a–e → Tasks 1, 2, 4. §6 controls 1–3 → Task 2 Steps 7–8, Task 3
Step 3. §7 → the Global Constraints forbid every listed non-goal. §8 → no arm touched; nothing to
do. §9 rulings → status-log (Task 4 Step 2). §10 → Task 4 Steps 4–5, prediction 572/0/2/124. §11
→ Task 4 Step 3.

**Placeholders.** The status-log skeleton in Task 4 Step 2 uses bracketed prompts by design — they
name exactly which numbers to transcribe and from where, and the step says "leave no brackets in
the committed text". No "TBD"/"similar to".

**Type consistency.** `diff_window_for_test(num: u64, i: usize, avail: usize, args: &[u64; 8]) ->
usize`; `forward_and_diff(num: u64, args: [u64; 8]) -> (u64, bool, Vec<Region>)` — Task 3
destructures three; `host_span_for_test(ipa: u64) -> Option<(*mut u8, usize)>` — Task 3 casts
`avail as u64`. `STACK_TOP_IPA: u64`. `DestLen::Reg(usize)`, `row!` takes the index as a literal —
matches the M29 rows (`Dest(Reg(2))`). `View::DestBuffer` exists. `SYS_FGETATTRLIST` exists; 336,
169, 170, 220 have no `SYS_` constants and are written as literals, as they are in the table.
