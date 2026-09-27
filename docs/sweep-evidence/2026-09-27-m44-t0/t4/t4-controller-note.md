# Task 4 Step 1 — controller-run re-measure, 2026-09-27

Method: `t4-sweep.sh` (scratchpad) — `git archive 5ac50f7` (Task 2's commit, the committed rows)
into scratchpad `t4tree`, `cargo build -p retrace` (exit 0), then `tools/apple-sweep.sh` with
RETRACE_SWEEP_KEEP_ALL=1:
- sweep a: /bin/ed, /bin/ls, /usr/bin/dddiagnose, /usr/bin/automationmodetool (one list);
- each xcrun trio member ALONE, after `rm -f /var/tmp/xcrun_db` (Ruling T4-a: a cold xcrun cache
  per member, so each member's own evidence is the path that reaches 464/128 — Ruling T0-e).

Scratch paths (copy into docs/sweep-evidence/2026-09-27-m44-t0/t4/, EXCLUDING *.bin — Ruling P1
amended: dir-anchored `':(exclude)docs/sweep-evidence/2026-09-27-m44-t0/t4/*.bin'`):
- evidence: /private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/1996e432-75ea-4d65-a52b-ed70d24a05b8/scratchpad/t4-evidence/  (<name>.rec.err, .rp.err, .rp.out, .bin)
- sweep logs: same scratchpad, t4-sweep-a.log, t4-sweep-desdp.log, t4-sweep-dyld_info.log, t4-sweep-flex.log
- the script: scratchpad t4-sweep.sh; the lists: t4-list-*.txt

## Result (labels identical to t0 M3 round b — no row differs)

| binary | label | rec/rp rc | wall (verbatim from the ROW line / rec.err) |
|---|---|---|---|
| /bin/ed | PASS | 0 / 0 | clean |
| /bin/ls | PASS | 0 / 0 | clean |
| /usr/bin/desdp | PASS* | 71 / 71 | `[retrace] refusing posix_spawn (syscall 244): exec-in-place is unmodelled; returning errno 14 without forwarding` |
| /usr/bin/dyld_info | PASS* | 71 / 71 | same refusal line |
| /usr/bin/flex | PASS* | 71 / 71 | same refusal line |
| /usr/bin/dddiagnose | FAIL | 4 / 3 | `RECORD ERROR: unsupported mach_msg2 at pc 0x1804adc34: msgh_id 205 dest 0x1c03 (guest task port Some(515)) send_size 24`; replay `DIVERGENCE at landmark 454 pc=0x1804adc34` |
| /usr/bin/automationmodetool | FAIL | 101 / n/a | `panicked at crates/retrace-arch/src/lib.rs:982:38: M33: syscall 374 (374) has no arg_kinds row …` |

*PASS in the sweep's sense (rc = rp < 128, stdout equal) but NOT the native outcome (native desdp
exits 2, usage — t0 M3): Ruling T0-b parks the trio at the refusal.

Every trio rec.err and dddiagnose/automationmodetool rec.err also carries the two known
`refusing mach_msg2 message-queue send/receive` lines (M38) — not walls; they are refused and the
run continues.

**Not yet measured (the Task 4 implementer does it):** landmark numbers for each binary's M44 rows
in these traces — 464/128 for the trio (cold cache), 461 for ls, 10/464 for ed, 345 for dddiagnose —
and the landmark of each wall. Use t0's throwaway reader (`t0-m3-nums-test.rs` in this directory;
recreate under crates/*/tests, run, DELETE before committing) over scratchpad t4-evidence/*.bin.
