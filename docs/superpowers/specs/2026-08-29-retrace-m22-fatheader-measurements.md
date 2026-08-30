# M22-fatheader measurements

Measured 2026-08-29 on macOS 26.x / Apple Silicon, against `main` at `ccfc8f9` (post-M20).

## Why measure before designing

The milestone began as an incidental observation while assessing the project's overall usefulness,
not as a planned capability: *every* Apple system binary appeared to fail under `record-dyn`, all of
them identically. The question the measurements had to settle was whether that was one small defect
or a wall — because "retrace cannot run Apple's binaries" and "retrace cannot *open* Apple's
binaries" call for completely different milestones, and the failure text alone does not distinguish
them.

## S1. The failure is at byte 0, not anywhere in the box

```
$ cargo run -q -p retrace -- record-dyn /bin/echo -o t.bin hi
thread 'main' panicked at crates/retrace-guest/src/lib.rs:10:5:
assertion `left == right` failed: not a 64-bit Mach-O (MH_MAGIC_64)
  left: 3199925962      # 0xBEBAFECA — FAT_MAGIC (0xcafebabe) read little-endian
 right: 4277009103      # 0xFEEDFACF — MH_MAGIC_64
```

`parse_macho` asserts `MH_MAGIC_64` against byte 0. Every macOS system binary is a **universal
(fat)** file, whose byte 0 is `0xcafebabe`:

```
$ lipo -archs /bin/echo /bin/ls /usr/bin/grep /usr/lib/dyld
x86_64 arm64e      # all four, identically
$ lipo -archs /opt/homebrew/bin/jq
arm64              # thin — which is why the ladder up to M21 could use it
```

So the guest ladder's reliance on self-built binaries plus Homebrew's thin `jq` was never a
statement about retrace's runtime capability. It was a statement about its *loader*.

## S2. Slicing by hand clears it — including arm64e guests with PAC

This is the measurement that decided the milestone's size. Extract the arm64e slice and re-run:

```
$ lipo -thin arm64e /bin/echo -output echo_thin
$ cargo run -q -p retrace -- record-dyn echo_thin -o t.bin -- hi
hi
```

It records. It also **replays**. Nothing below the loader needed to change, and the reason is
structural rather than lucky:

- An arm64e main turns PAC on through the existing `pac_posture(cpusubtype)` path
  (`retrace-box/src/lib.rs:164`), which M7 already built.
- Replay does not read the file at all — `restore()` re-derives the posture from the snapshot's own
  mach header via `pac_posture_from_memory` (`retrace-box/src/lib.rs:181`). So an arm64e guest
  replays correctly **by construction**, with no trace-format change and no new recorded field.

`TRACE_MAGIC` therefore does not move, and no recording made before M22 becomes unreadable.

## S3. The breadth this unlocks — 34 of 54, and only four causes

Sampled all of `/bin` plus every 8th binary of `/usr/bin` (148 candidates), pointing retrace
straight at each file. A run counts as **PASS** only if it records, replays, stdout is
byte-identical between the two, *and* the exit codes match. An interactive/destructive denylist
(`su`, `login`, `dd`, `shutdown`, …) and non-arm64 files were skipped; stdin was `/dev/null`.

| Outcome | Count |
|---|---|
| **PASS** (record + replay, stdout identical, exit codes equal) | **34** |
| `non-syscall exit … (EC=0x00 ISS=0x0 FSC=0x0) far/ipa=0x0 (UNMAPPED) pc=0x4204 elr=0x4404` | 13 |
| `unsupported mach_msg2 … msgh_id 412` | 4 |
| `dup2 is not modelled by the M10 fd table` (the existing fail-loud assert) | 2 |
| genuine replay divergence | 1 |
| attempted (non-skipped) | 54 |

**63% pass, from a prior baseline of exactly zero.** Passing binaries include `echo`, `cat`, `ls`,
`cp`, `mv`, `rm`, `chmod`, `mkdir`, `rmdir`, `ln`, `df`, `pwd`, `hostname`, `expr`, `test`, `sh`,
`dash`, `ksh`, `ed`, `stty`, `realpath`, `bzip2`, `pax`, `sync`, `wc`, `grep`, `uname`, `basename`.

The distribution is the load-bearing part. Twenty failures resolve to **four** causes, not a long
tail — so the wall behind this milestone is narrow and nameable rather than diffuse:

- **13 × `pc=0x4204`** — all of them modern ObjC/Swift-heavy `/usr/bin` tools (`aa`, `afktool`,
  `AssetCacheManagerUtil`, `automationmodetool`, `avmediainfo`, `bioutil`, `chfn`, …). `EC=0x00` is
  the exception class the box cannot categorise at all, and the pc is a low address one granule in,
  so control has left the loaded images rather than faulting inside them. **Not diagnosed.** This is
  where M22's honest gate is parked.
- **4 × `mach_msg2` `msgh_id` 412** (`bash`, `zsh`, `date`, `cal`) — one unrouted MIG message, the
  same shape as the 3409/3410/3405 routes M2 added one at a time.
- **2 × unmodelled `dup2`** (`csh`, `tcsh`) — the M10 fd table's own `assert!`, firing exactly as
  designed. Not a new defect; a documented gap being reached by a new guest.
- **1 × divergence** (`ps`) — the replay oracle catching real nondeterminism rather than replaying
  something wrong in silence. Also not diagnosed.

## S4. `slice_arm64e` already existed, and is nearly the whole implementation

`retrace-guest` has shipped a fat-slice picker since M2, used for dyld:

```rust
parse_macho(slice_arm64e(&std::fs::read(DYLD_PATH).unwrap()))
```

It already handles big-endian fat headers, `FAT_MAGIC` and `FAT_MAGIC_64`, both `fat_arch` strides,
and thin passthrough. Two things kept it from covering the guest:

1. It was never called on the **main executable** — only on dyld (`crates/retrace/src/main.rs:50`).
2. It **panics** when no arm64e slice exists, so it cannot serve the `x86_64 + arm64` universal
   shape (Homebrew's, when it builds universal).

## S5. Preference order is load-bearing, and no Apple binary can test it

Every fat Apple binary on this machine is `x86_64 + arm64e` — none carries both arm64 *and* arm64e.
So the preference rule cannot be exercised by any real file here, and only a synthetic fixture can
pin it. It still matters: `cpusubtype` is what `pac_posture` reads, so taking the plain-arm64 slice
of a file carrying both would run an arm64e guest **with PAC off** — M7's wall reached by a new
road, and a silent wrong-answer rather than a loud one.

## What this measures *against* — the counts M22 must not disturb

`main` at `ccfc8f9`: 476 passed / 0 failed / 2 ignored across 106 test binaries.

## Open questions the measurements did *not* settle

- Why `pc=0x4204`. Thirteen binaries reach it; none was traced with `RETRACE_TRACE=1` to the last
  good pc. That is the next milestone's measurement, not this one's.
- What `msgh_id` 412 is. Four binaries need it; the MIG subsystem was not decoded.
- Whether `ps`'s divergence is one cause or several.
- Whether the 63% figure holds over the full 921 executables in `/bin` + `/usr/bin`; the sweep was
  stopped at 67 rows to stop competing for VMs with a concurrent M21 session on the same machine.
