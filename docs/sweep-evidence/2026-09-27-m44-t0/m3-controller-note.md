# t0 M3 — controller-run (Ruling P5), 2026-09-27

Method: `git archive ebd0266` extracted to the session scratchpad (`m3tree`), throwaway rows added
there only (the worktree was never edited): `SYS_OPENAT | 464 => [Fd, Path, Scalar, Scalar]` (F),
`345 => [Path, Ptr]`, `461 => [Fd, Ptr, Dest(Reg(3)), Scalar, Scalar]`. `cargo build -p retrace`
(exit 0), then `tools/apple-sweep.sh <m3tree binary>` with RETRACE_SWEEP_LIST = the 7 targets,
RETRACE_SWEEP_KEEP_ALL=1. Round b added two more throwaway rows the round-a walls named:
`10 => [Path]` (unlink) and `128 => [Path, Path]` (rename), rebuilt, re-swept the 4 affected.

Scratch evidence (copy what the measurements file cites into docs/sweep-evidence/<t0 date>-m44-t0/,
EXCLUDING *.bin — Ruling P1):
- round a: /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/1996e432-75ea-4d65-a52b-ed70d24a05b8/scratchpad/m3-sweep.log and m3-evidence/
- round b: .../scratchpad/m3b-sweep.log and m3b-evidence/
- native: .../scratchpad/m3-native-{ed,ls,desdp}.{out,err}

## Round a (rows 464, 345, 461)

| binary | label | rec/rp rc | wall |
|---|---|---|---|
| /bin/ed | FAIL | 101 / n/a | recorder panic: M33 `syscall 10 (10) has no arg_kinds row` (unlink) |
| /bin/ls | PASS | 0 / 0 | — clean |
| /usr/bin/desdp | FAIL | 101 / n/a | M33 `syscall 128 (128) has no arg_kinds row` (rename) |
| /usr/bin/dyld_info | FAIL | 101 / n/a | same, 128 |
| /usr/bin/flex | FAIL | 101 / n/a | same, 128 |
| /usr/bin/dddiagnose | FAIL | 4 / 3 | `RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 205 dest 0x1d03 (guest task port Some(515)) send_size 24` — msgh_id 205 = `host_get_io_main` (SDK mach/mach_host.h:1313 `{ "host_get_io_main", 205 }`), the I/O Kit main port: class C |
| /usr/bin/automationmodetool | FAIL | 101 / n/a | M33 `syscall 374 (374) has no arg_kinds row` — unchanged (kevent_qos) |

TALLY pass=1 fail=6.

## Round b (+ rows 10, 128)

| binary | label | rec/rp rc | note |
|---|---|---|---|
| /bin/ed | PASS | 0 / 0 | clean; native `ed </dev/null` rc 0, 0 bytes stdout, 0 bytes stderr |
| /usr/bin/desdp | PASS* | 71 / 71 | *identical, but NOT the native outcome: recorder stderr ends `[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14 without forwarding`; the guest prints `desdp: error: couldn't spawn '/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild' …` and exits 71. Native `desdp </dev/null` exits 2 (usage). So the wall is the exec refusal (class C) — spec §2b's inference holds, one missing row (128) later than predicted. |
| /usr/bin/dyld_info | PASS* | 71 / 71 | same |
| /usr/bin/flex | PASS* | 71 / 71 | same |

Native `ls` of crates/retrace: rc 0, 46 bytes.

## Controller ruling (in the ledger as Ruling T0-a)

A2's row set grows by `10 unlink` and `128 rename` (both SDK-verified: `SYS_unlink 10`, `SYS_rename
128` — confirm in the header). They are plain path rows the targets measurably reach, the class A2
exists for. Outcome with them: `ls` and `ed` green; `desdp`/`dyld_info`/`flex` re-parked at the
posix_spawn refusal (class C, exec-in-place); `dddiagnose` re-parked at `host_get_io_main` (class C);
`automationmodetool` per M1.
