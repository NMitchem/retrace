# jq's 300k abort against M47's `mach_vm_map` mask fix — 2026-10-02

`docs/current-state.md` records that `jq -n '[range(0;N)] | add'` exits 134 (`SIGABRT`) at
`N = 300000` under retrace, on record and on replay alike, while native jq exits 0. The last traps
before the abort are a `mach_vm_map` (−15) with a 4 MiB alignment mask and the malloc-large tag.
Task 3b fixed exactly that class: retrace had ignored `mach_vm_map`'s alignment mask. This directory
measures whether that fix clears the abort.

**Result: it does, and the mask fix alone is what clears it.**

- Native jq (`/opt/homebrew/bin/jq`, jq-1.8.2) exits 0 and prints `44999850000`.
- **Base against the swept binary** (`t6-jq300k.sh` → `jq300k.txt`), three rounds, alternating:

  | binary | record | replay | stdout |
  |---|---|---|---|
  | base, `m47-base-retrace` (427fa0a's code) | 134, 134, 134 | 134, 134, 134 | empty |
  | swept, `retrace-t6` (`a8a1ecd`, all of M47) | 0, 0, 0 | 0, 0, 0 | `44999850000`, as native |

  Every base record ends `guest terminated by signal 6`. Every replay matched its own record's stdout
  (`cmp` 0), and none printed a `DIVERGENCE`.
- **The mask fix alone** (`t6-jq300k-iso.sh` → `jq300k-iso.txt`). The swept binary carries all of M47,
  so the same runs were repeated on Task 3b's diagnosis pair. Those two binaries differ only by
  `fix.diff`, the mask fix (`docs/sweep-evidence/2026-09-30-m47-abort/README.md`):

  | binary | record | replay | stdout |
  |---|---|---|---|
  | `rt0`, `090bf5e` unpatched (sha256 `9301e771…`) | 134, 134, 134 | 134, 134, 134 | empty |
  | `rt-fix`, `090bf5e` + `fix.diff` (sha256 `43f00caf…`) | 0, 0, 0 | 0, 0, 0 | `44999850000` |

**Attribution: the 300k abort was the `mach_vm_map` mask class, and Task 3b's fix clears it.** That
was measured on a pair that differs by the fix and nothing else. The current-state entry
("aborts … the cause is not diagnosed") is now owed a rewrite (Task 7).

**Method.**
- Each binary recorded and replayed only its own trace. `TRACE_MAGIC` is `RT\x00\x0b` on the swept
  binary and `RT\x00\x0a` on the others.
- Every phase was bounded by a 900 s watchdog, which never fired. Each record in `jq300k.txt` took
  4–9 s.
- Traces lived in the session scratchpad and were removed after each run, so **no `.bin` is here**.
- The runs started after the sweep and its controls had finished, with no `cargo` running. The
  1-minute load was 1.30–5.06.
- Files:
  - `native.{out,err}`;
  - `r{1,2,3}-{base,t6}.{rec,rp}.{out,err}` and `i{1,2,3}-{rt0,rt-fix}.{rec,rp}.{out,err}`;
  - the two summaries;
  - the two scripts, `t6-jq300k.sh` and `t6-jq300k-iso.sh`.

  Every stderr is kept whole. None is longer than 8 lines, so each non-zero run's last 30 lines are
  all there. Every aborting record ends `guest terminated by signal 6`.
