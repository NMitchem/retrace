# t0 M5(i) — controller-run (Ruling P5), 2026-09-27

Method: `cpu.sh` (scratchpad) — `cargo test -p retrace --test <t> [<name>] --no-run` first (untimed),
then three serial `/usr/bin/time -p cargo test -p retrace --test <t> [<name>] -- --test-threads=1`
runs per tree, nothing else running on the machine. Trees: `git archive c652cf1` (M42 merge, before
the pre-decode) and `git archive ebd0266` (code-identical to 64e471e). All 12 runs exit 0.
Raw files: scratchpad `cpu-m42-oracle.txt`, `cpu-m44-oracle.txt`, `cpu-m42-cpy.txt`, `cpu-m44-cpy.txt`
(+ `.build`, `.run1..3`). Times include cargo's own startup and the codesign runner, identically on
both trees.

| target | tree | user s (run 1 / 2 / 3) | median | real median |
|---|---|---|---|---|
| hitorder_e2e `oracle_threadrust_breakpoints_at_both_switches` | c652cf1 | 21.16 / 23.24 / 21.27 | **21.27** | 23.89 |
| same | 64e471e (ebd0266) | 31.67 / 30.69 / 30.90 | **30.90** | 33.72 |
| cpython_crash_e2e (whole target) | c652cf1 | 41.34 / 41.24 / 41.23 | **41.24** | 42.31 |
| same | 64e471e (ebd0266) | 40.56 / 40.92 / 41.22 | **40.92** | 41.75 |

Gap on oracle_threadrust: 30.90 − 21.27 = **9.63 s user (+45%)**. cpython_crash_e2e: flat (−0.32 s,
inside run-to-run spread). Spec premise (B1: M43 regressed oracle_threadrust CPU) **holds**.

**B1 pass bar** (spec §3c: close at least half the gap): median user CPU of
`oracle_threadrust_breakpoints_at_both_switches`, measured the same way, **≤ 26.08 s**
(30.90 − 9.63/2 = 26.085). Task 6 measures its tree with the same script, three runs, quiet machine.
