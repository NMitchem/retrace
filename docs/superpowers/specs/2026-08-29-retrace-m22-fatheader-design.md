# M22-fatheader design

Companion to [`2026-08-29-retrace-m22-fatheader-measurements.md`](2026-08-29-retrace-m22-fatheader-measurements.md).
Every claim below traces to a measurement there (S1–S5).

## The problem, precisely

`parse_macho` asserts `MH_MAGIC_64` against byte 0 of the bytes it is handed. Every executable Apple
ships is a **universal (fat)** file whose byte 0 is `0xcafebabe`, so every Apple binary failed at the
header having executed nothing (S1).

The consequence was mis-read for twenty milestones as a capability boundary. It is not one. Slice the
file by hand and the same binaries record *and* replay, arm64e and PAC included, with nothing below
the loader changed (S2). retrace could always run Apple's binaries; it could not **open** them.

## What the measurements settled, and what they changed

- The fix is confined to the loader. Replay re-derives PAC posture from the snapshot's own mach
  header, not from the file, so arm64e guests replay **by construction** (S2). No trace-format
  change, no new recorded field, `TRACE_MAGIC` unmoved, no existing recording invalidated.
- The payoff is 34 of 54 sampled system binaries, from a baseline of zero, and the 20 failures are
  **four** named causes rather than a long tail (S3). That distribution is what makes this a
  milestone rather than a curiosity: the wall behind it is narrow and nameable.
- Most of the implementation already exists. `slice_arm64e` has handled big-endian fat headers, both
  `FAT_MAGIC` variants, both strides and thin passthrough since M2 — it was simply never called on
  the main executable, and it panics rather than accepting a plain-arm64 slice (S4).

## The mechanism

### One picker, called from the loader

`slice_native(b)` returns the slice this machine would execute; `parse_macho` calls it as its first
statement. Placing it *in the loader* rather than at the two CLI call sites is deliberate: every
caller — the CLI's `record` and `record-dyn`, and ~40 test call sites — gets it without knowing which
shape it holds, and a future caller cannot forget it.

`fat_find(fat, want_sub)` is the shared finder extracted from the old `slice_arm64e` body, so both
pickers walk one implementation of the big-endian table and the two `fat_arch` strides.

### Preference: arm64e, then arm64 — and it is not cosmetic

```
slice_native = fat_find(ARM64E)  or_else  fat_find(ARM64_ALL)  or_else  panic
```

`cpusubtype` is what `pac_posture` reads. Taking the plain-arm64 slice of a file that carries both
would run an arm64e guest **with PAC off** — M7's wall by a new road, and silent rather than loud.
No Apple binary on this machine carries both, so only a synthetic fixture can pin this (S5); one
does.

### Passthrough is total, so nothing else moves

`slice_native` returns its input unchanged for a thin Mach-O **and** for anything that is not a fat
file at all. That keeps `parse_macho`'s existing `MH_MAGIC_64` assert as the single place a
non-Mach-O is rejected, with the message that already says so — the error text for garbage input does
not change. `slice_arm64e` keeps its old signature, its old panic, and its dyld call sites.

## Fail-loud boundaries

- A fat file carrying **neither** arm64e nor arm64 (e.g. `x86_64` alone) panics naming the
  constraint, rather than falling through to parse an x86_64 header as arm64.
- `slice_arm64e` still panics on a fat file with no arm64e slice. Its callers (dyld) require arm64e
  specifically, and weakening it to `slice_native` would let a plain-arm64 dyld through silently.
- The 13-binary `pc=0x4204` wall is **not** guessed at. It is parked behind an `#[ignore]`d gate
  naming the exact exception text and stating that nothing has measured the cause.

## Exit criterion

An Apple system binary, taken straight from `/bin` with no slicing step, records and replays
bit-for-bit through the rung assertion (clean `exit(0)`, exact stdout, replayed twice) —
`crates/retrace/tests/sysbin_e2e.rs`. The gate asserts its guest is genuinely fat first, so it cannot
pass vacuously if a future macOS ships that binary thin.

## Risk register

| # | Risk | Disposition |
|---|---|---|
| R1 | A fat file whose slice offsets are hostile (overlapping, out of range) panics on a slice index rather than a clear message. | **Accepted.** Identical to today's behaviour for a truncated thin Mach-O; retrace loads files the user names, not untrusted input. |
| R2 | Preferring arm64e diverges from the kernel, which runs arm64e only for platform binaries or opted-in third-party ones. | **Accepted, and deliberate.** retrace supports both postures and derives from the chosen slice, so either choice runs correctly; arm64e reproduces what Apple's own binaries actually do, which is the case that matters here. |
| R3 | The 63% figure drifts as macOS updates. | **Mitigated.** The number is recorded in the measurements doc and in `sysbin_e2e.rs` with its date and base commit, so drift is distinguishable from regression. |
| R4 | The parked wall is really several causes wearing one error message. | **Open.** S3 groups 13 binaries by identical text, not by traced cause. The parked gate says so. |

## Components

| Component | Change |
|---|---|
| `retrace-arch` | `CPU_SUBTYPE_ARM64_ALL` (1 line) |
| `retrace-guest` | `fat_find` / `is_thin` / `is_fat` extracted; `slice_native` added; `slice_arm64e` rewritten onto the shared finder; one line in `parse_macho` |
| `crates/retrace/tests/sysbin_e2e.rs` | the headline gate + the parked wall |
| `retrace-guest` unit tests | 3 fat-header tests (real dyld; synthetic dual-arm64; synthetic x86_64+arm64) |

Net production change is ~30 lines; the rest is tests and documentation.

## Open questions for a later milestone

Diagnosing `pc=0x4204` (13 binaries) is the highest-value follow-up — it is plausibly the difference
between 63% and ~87%. Then `msgh_id` 412 (4 binaries), then `ps`'s divergence.
