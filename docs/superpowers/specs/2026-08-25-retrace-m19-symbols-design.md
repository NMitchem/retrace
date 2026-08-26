# retrace M19-symbols — the debugger learns to say `_child+0x30`

Design spec. Companion measurements: `2026-08-25-retrace-m19-symbols-measurements.md`, cited by
number (M1–M7) throughout rather than restated.

## The problem, precisely

Every address retrace prints is raw hex. `crates/retrace/src/debug.rs` renders positions as
`pc={pc:#x}`, breakpoint hits as `hit {pc:#x}`, watchpoint writers as
`hit watch {watched:#x} (write at {pc:#x})`, and crashes as
`guest crashed: pc={pc:#x} far={far:#x} esr={esr:#x}`. The README names this under Known limits:
debugging is address-level only.

The cost is concrete and it lands on exactly the work M18 just finished. The fast-follow's whole
story is *which thread faulted* — and the answer it prints is `pc=0x10000050c`. A reader must run
`nm` by hand, on the right binary, and subtract, to learn that this is `_child+0x30`: the child
thread's faulting store, the thing the fixture was built to produce. The debugger knows the address
and has the symbol table sitting in memory; it simply never looks.

## What the measurements settled, and what they changed

The design question was *where the names come from*. M6 shows the trace records no path, UUID, or
image identity — which reads, at first, as a forced choice between a format break (add a path, bump
`TRACE_MAGIC`, invalidate every recording) and a `--exe <path>` flag (which can name a *different
build* than the one recorded — silent mis-symbolication, worse than no symbolication).

**M4 and M5 dissolve that choice, and they are the reason this milestone is small.** `parse_macho`
maps every `LC_SEGMENT_64` except `__PAGEZERO`, so `__LINKEDIT` — which contains the `nlist_64` array
and the string table — is mapped into guest memory; and `snapshot()` captures every backing in full.
**The symbol table is already inside every recording ever made in the current format.**

Three consequences, and the third is the one that matters on this codebase:

1. **No format break.** `TRACE_MAGIC` stays `RT\x00\x08`. Existing traces gain symbols retroactively.
2. **No external file.** Nothing to pass, lose, or mismatch. The symbols are *by construction* those
   of the binary that was actually recorded — a stale-binary mis-symbolication is not merely avoided,
   it is unrepresentable.
3. **No determinism surface at all.** Symbolication is a pure function of recorded bytes and
   compile-time layout constants. It runs in the debug CLI, **below nothing and above everything**:
   it never touches `record_box`, `ReplaySession::advance`, or `Box_::run()`. Neither symmetry rule
   is engaged, no new landmark exists, `verify_thread`'s seven call sites are untouched, and the
   divergence oracle cannot see this milestone. **M19 cannot make a recording diverge.**

That third point is the argument for doing symbols now rather than later: it is the rare capability
on this project that carries no determinism risk whatsoever.

## The mechanism

### Reading the table

For an image based at `B`:

1. Find the Mach-O header in the snapshot at `B`. The precedent exists — `retrace-box`'s PAC-posture
   probe already locates a region covering `EXE_BASE` and checks `MH_MAGIC_64` there
   (`crates/retrace-box/src/lib.rs:183`), refusing to guess when it is absent. M19 reuses that shape.
2. Walk load commands for **`LC_SYMTAB` (0x2)** — `symoff`, `nsyms`, `stroff`, `strsize` — and for
   the `__LINKEDIT` and `__TEXT` `LC_SEGMENT_64`s.
3. Convert `symoff`/`stroff` from **file** offsets to guest VAs through `__LINKEDIT`'s own
   `(fileoff → vmaddr)` mapping, plus the image slide (M4 gives the arithmetic; for `crashthread`,
   `symoff 32960` becomes guest VA `0x1000080c0`).
4. Read `nsyms` × `nlist_64` (16 bytes: `n_strx:u32, n_type:u8, n_sect:u8, n_desc:u16, n_value:u64`)
   out of snapshot memory, and the string table likewise.
5. Keep entries that are **defined in a section** — `n_type & N_TYPE == N_SECT` and `n_sect != 0` —
   and **skip debug entries** (`n_type & N_STAB != 0`), which otherwise pollute the table with
   source-file records at bogus addresses (measurements' open question 1).

`LC_SYMTAB` and not the exports trie, for M1's reason: `_child` is `static`, a lowercase-`t` local.
It is in `LC_SYMTAB` and nowhere else, and it is precisely the symbol this milestone exists to print.

### Resolving an address

Sort by address; resolve by **nearest preceding symbol**, returning `(name, addr − sym_addr)`.

Three rules keep that honest:

- **Ties are ordered, not arbitrary.** Aliases share an address. Sort by `(addr, name)` and take the
  first, so the same address always prints the same name — a debugger that renames a function
  between two runs of the same query is worse than one that prints hex (open question 2).
- **There is an upper bound.** A pc past the last symbol must not resolve to `last+0x4c9f`. Clamp to
  `__TEXT`'s `vmaddr + vmsize`, read from the same header; past it, resolve to nothing (open
  question 3).
- **Unresolvable prints as the raw address, alone.** No nearest guess, no `??`, no synthesized name.
  This is the M11/M6 rule in a new place: *never print something the data does not support.*

### Rendering

`0x10000050c` → `0x10000050c (_child+0x30)`, and exactly at a symbol → `0x100000460 (_main)`.
The raw address is **always** present, never replaced. Every existing assertion that matches on a
hex address keeps matching, and a reader can still copy the number into `x` or `break`.

## Scope

**In, Stage 1:**

- `LC_SYMTAB` reader over snapshot memory, pure and unit-testable with no VM.
- Nearest-preceding resolution with the three honesty rules above.
- The **main executable** (`EXE_BASE`, slide 0 per M2).
- **dyld** (`DYLD_BASE`) — same mechanism, same `parse_macho`-mapped `__LINKEDIT` (M7), differing
  only by slide, and 4015 symbols of payoff (M3). Sequenced last so it is droppable without
  affecting anything before it.
- Wiring into `debug.rs`'s address-printing sites.

**Out, and named as limits rather than left to be discovered:**

- **Shared-cache addresses** (`0x1_8000_0000`–`0x3_0000_0000`). Cache images carry no `LC_SYMTAB` in
  the mapped region; the cache's local-symbol area lives in the on-disk cache file, which `cache.rs`
  demand-pages but never stages into guest memory. Symbolicating one would require reading that file
  at debug time — reintroducing the external-file dependency M6 eliminated. This is the wall, and it
  gets a parked gate (below).
- **Stripped binaries.** `jq` has 7 defined text symbols (M3). Nothing to fix here; it is a property
  of the binary. The README must not imply uniform coverage.
- **Rust demangling.** 762–969 mangled `_ZN…E` names (M3). Raw mangled names beat hex and need no
  demangler; demangling is a separable later improvement.
- **DWARF, line tables, `.dSYM`, variables, types.** Stage 2+ and a different milestone.

## Where the code lives

`crates/retrace-core/src/symbols.rs` — a new pure module, on the precedent of `machmsg.rs` ("the pure
`mach_msg2`/MIG codec + router"). It depends only on `retrace_trace::Region` and integer parsing, so
its tests need no VM and run in the fast workspace chunk. The debug CLI consumes it.

It does **not** go in `retrace-box`: the box owns VM state, and this reads a `Vec<Region>` that has
already left the VM. It does not go in `retrace-guest` either — that crate's `parse_macho` reads a
*file* to produce segments to load, whereas this reads *snapshot memory* to produce names, and fusing
them would couple the loader to the debugger for no shared logic.

## Fail-loud boundaries

Consistent with the codebase's posture, but with one deliberate inversion:

- A **malformed** `LC_SYMTAB` (offsets outside `__LINKEDIT`, `nsyms` implying a table past the
  region, `n_strx` past `strsize`) is a **bug in this reader or a corrupt trace** and asserts.
- A **missing** `LC_SYMTAB`, or an image with zero usable symbols, is **normal** (M3: `jq`) and must
  degrade silently to raw-hex output. This is the inversion: absence is data, malformation is a bug.
  Conflating them would make every stripped-binary session panic.
- Symbolication **never fails a replay**. It is presentation; if the reader cannot build a table, the
  debugger prints hex and keeps working. No `Divergence` may originate here.

## Exit criterion

`retrace debug` on a recording of `crashthread`, stopped at the crash, prints the faulting pc as
`_child+0x30` — naming the child thread's faulting store, from the trace alone, with no binary path
supplied and no format change.

## Testing

- **Unit (no VM, fast chunk):** table construction from a synthetic `Region` set; nearest-preceding
  resolution; tie determinism; the `__TEXT` upper clamp; `N_STAB` rejection; missing-`LC_SYMTAB`
  degradation; malformed-table assertion.
- **Headline e2e — `symbols_e2e`:** record `crashthread`, run the debug CLI, assert the crash line
  contains `_child`. **Assert on the difference this milestone makes** (honest-gate rule): the raw
  address was already printed before M19, so asserting on `0x10000050c` would pass on a no-op. The
  assertion is the *name*.
- **Regression:** the raw address must still appear alongside the name, so nothing that greps for it
  breaks.
- **Parked gate — `cache_symbol_e2e`, `#[ignore]`d** at the shared-cache wall, with the reason
  naming the measurement it owes (cache local-symbol area, on-disk, not staged into guest memory).
  Parking a new gate for a capability the milestone does not have is the discipline working.

## Risk register

| # | risk | mitigation |
|---|---|---|
| R1 | `__LINKEDIT` mapped but the symtab range not fully covered by one `Region` | reader spans regions by IPA rather than assuming one; asserts if bytes are genuinely absent |
| R2 | `N_STAB` entries pollute the table | explicit `N_STAB` skip, unit-tested with a synthetic stab entry |
| R3 | dyld's slide computed wrong → confidently wrong names, the worst failure mode | use the loader's own rule, `guest_va = file_vmaddr + slide` with `slide = 0` (exe) / `DYLD_BASE` (dyld) — measured in P3; assert the resolved name of a known dyld export |
| R4 | scope creep into DWARF | Stage 1 ends at `LC_SYMTAB`; line tables are a separate milestone |
| R5 | 444-test baseline disturbed | M19 is additive; the ignored count moves 1→2 only by the *deliberate* parked gate, which the status log must state |

## Components

1. `symbols.rs` — Mach-O header + `LC_SYMTAB` reader over `Vec<Region>`.
2. `SymbolTable::resolve` / `format` — nearest-preceding with the three honesty rules.
3. Image routing over the M7 constants; main exe first, dyld second.
4. `debug.rs` wiring at the address-printing sites.
5. `symbols_e2e` + the parked `cache_symbol_e2e`.
6. Docs: README "What works today" / "Known limits", and an appended `docs/status-log.md` section.

## Open questions for implementation planning

1. Does one `Region` always cover the whole `__LINKEDIT`, or must the reader span regions? (R1 —
   settle by measurement against a real snapshot before writing the reader.)
2. Should `debug.rs` symbolicate *every* address site at once, or only the crash/hit/position lines?
   Leaning: only the pc-bearing lines. `x <addr>` prints memory the user already named, and `far` is
   a data address whose symbol would usually be meaningless.
3. Does the table get built once per session and cached, or per query? Leaning: once, lazily, on the
   snapshot the session already holds — but the seek machinery restores snapshots repeatedly, so the
   cache key needs a moment's thought rather than an assumption.
