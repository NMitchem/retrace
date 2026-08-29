# M23-xpcpipe plan

Design: [`../specs/2026-08-29-retrace-m23-xpcpipe-design.md`](../specs/2026-08-29-retrace-m23-xpcpipe-design.md).
Measurements: [`../specs/2026-08-29-retrace-m23-xpcpipe-measurements.md`](../specs/2026-08-29-retrace-m23-xpcpipe-measurements.md).

Branch: cut from `main` at `10fcf5a` (post-M22).

> **Note on execution.** The measurements (S1–S5) were taken as throwaway probes and **fully
> reverted**; the tree is clean at plan time. Every task below is a genuine forecast, not a
> post-hoc record. Tasks 2 and 3a were *spiked* during measurement (the one-line allowlist entry
> was observed to work, and the wall behind it observed) — the implementations below must still be
> written TDD, RED first, with the spike treated as evidence the approach is viable, **not** as
> code to keep.

## t1 — Trampoline padding traps, and the fall-through is counted

**RED.** `crates/retrace-box/src/lib.rs` unit tests:

1. `vector_padding_traps_rather_than_undefs` — build a dynamic box and read back every vector slot's
   words 1..0x20 from guest memory; require each to be a trapping instruction, not `0x00000000`.
   Observed failure today: reads `00000000`.
2. `fall_through_counter_starts_at_zero_and_is_observable` — a `Box_` accessor for the count exists
   and reads 0 on a fresh box. Observed failure today: no such method.

**GREEN.** Fill each slot's padding at build time in **both** vector construction sites
(`Box_::new` ~1065 and `load_dynamic` ~1622 — there are two, and `record-dyn` uses the *second*; a
change to only one is silently ineffective). Add the counter to `Box_`, incremented in `run()` when an
exit is the padding trap, with the existing dispatch performing the recovery.

**Verify.** `cargo test -p retrace-box -- --test-threads=1`.

**Mutation check.** Revert the `load_dynamic` site only → test 1 must still fail. This is the specific
trap that cost four misdirected probes during measurement, so the plan pins it.

## t2 — The record/replay fall-through invariant

**RED.** `crates/retrace/tests/` — record and replay a guest that exercises a fall-through, assert the
counts are equal and that a *forced* mismatch fails loud. Fixture: one of the 13 (S1). If no guest can
yet be recorded end-to-end (likely until t4), assert the mismatch path directly by injecting a count
delta, and note in the test why the end-to-end version is deferred.

**GREEN.** Compare the count at the replay's terminal comparison; on mismatch, report it as a
divergence naming both counts.

**Why this is not optional:** t1's recovery is silent self-healing without it, which is precisely the
failure a determinism oracle cannot see (design, Task 1).

## t3 — `host_get_special_port` (msgh_id 412)

**RED.** `crates/retrace-core/src/machmsg.rs` tests:

1. `route_forwards_host_get_special_port` — a 412 `Msg2` in the KOBJECT shape routes to
   `Forward("host_get_special_port")`. Fails today: `Unsupported`.
2. `decode_host_get_special_port_reads_node_and_which` — golden 40-byte request from S3 decodes to
   `(node, which) == (-1, 1)`. Fails today: no decoder.
3. `decode_host_get_special_port_rejects_other_ports` — `which = 2` (HOST_PRIV_PORT) is an error.

**GREEN.** One `FORWARD_ALLOWLIST` entry plus the decoder and its assert, mirroring 3409/3410.

**Verify.** `/bin/date` advances past 412 to the XPC wall (S4's observed line). Sweep all 17 and
confirm uniformity — do not generalise from `date`.

## t4 — Measure libxpc's response to a refusal (**decides t5**)

Not a code task. Return a refusal for the `MACH64_SEND_MQ_CALL` send and observe: does libxpc take a
no-service fallback, retry, or abort? Try the plausible refusals (a MIG error reply; a send failure)
and record what each produces, for **all 17** — risk R2 is that they stop being uniform here.

**Output:** a measurements addendum (S6) naming the posture t5 must implement. If refusal is not
survivable, t5 becomes the proxy design and the milestone re-scopes rather than forcing it.

## t5 — Service the XPC pipe

Shape determined by t4. Whichever posture, it must obey symmetry rule 1: the record arm and the replay
mirror recompute *identical* bytes by calling the same `Box_` method with the same arguments, placed
before the generic forward arm — **and add the eighth `verify_thread` call site if the mirror
`return`s**, since every new mirror silently creates an oracle hole until it does.

## t6 — The gate, and honest parking

`crates/retrace/tests/xpc_e2e.rs`: record and replay `/bin/date`, asserting the `"@XPC"` send was
*serviced* — the route taken and its reply bytes — not merely a clean exit, since a guest that never
reached XPC also exits 0 (the `protnone_rust_e2e` rule).

If a wall remains, park it `#[ignore]`d with the reason naming **that** wall precisely, never a
generic "XPC unsupported". Re-sweep all 54 binaries and report the new inventory.

## t7 — Close

Chunked gate (the full `--workspace` run exceeds the tool ceiling), **including the `--bins` chunk**,
reconciled file-by-file against M22's **480 passed / 0 failed / 3 ignored over 107 binaries** — not
M20's 476/0/2, which is the number a stale reading of the README would give. Then append a Status
section to `docs/status-log.md` and **edit** the README's "What works today" / "Known limits".

## Sequencing

t1 → t2 → t3 → t4 → t5 → t6 → t7. t3 is independent of t1/t2 and can land first if convenient, but
t1 must precede any re-sweep of the 13, and t4 must precede t5 by construction.

## Coordination

M21-stackgrow is in flight on `m21-stackgrow` (task 1 of 5, 4 commits, unmerged) and its task 4 edits
the same README lines as t7. Whichever lands second reconciles.
