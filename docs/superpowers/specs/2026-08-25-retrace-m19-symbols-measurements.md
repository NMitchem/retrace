# retrace M19-symbols — what the binaries and the trace actually contain

Measured 2026-08-25 on macOS 26.x / Apple Silicon, at `04bf9ea` plus the uncommitted M18 fast-follow.
Every number below came from the real toolchain against the real artifacts; nothing here is inferred
from documentation. The design spec cites this file rather than restating it, so the two cannot
drift apart.

## Why measure before designing

M19 turns `pc=0x10000050c` into a name. The whole question is *where the names come from*, and there
are three candidate sources with wildly different costs — the on-disk binary (needs a path the trace
does not carry), the recorded snapshot (needs `__LINKEDIT` to be mapped), and the dyld shared cache
(needs the cache's own symbol format). Which one is viable is an empirical question, and the answer
decided the milestone's shape. It is measured here first for the reason M15's plan was wrong six
ways: on this codebase, the plan written before the measurement is the plan that is wrong.

## M1. `LC_SYMTAB` is present, and the interesting symbol is a *local*

```sh
otool -l <guest> | grep -A3 'cmd LC_SYMTAB'
nm -n <guest>
```

For `crashthread` (the M18 fast-follow fixture — the guest whose crash M19 most wants to name):

```
cmd LC_SYMTAB   symoff 32960   nsyms 6
0000000100000000 T __mh_execute_header
0000000100000460 T _main
00000001000004dc t _child        <-- lowercase t: a LOCAL symbol
```

**The lowercase `t` is load-bearing.** `_child` is `static`, so it appears in `LC_SYMTAB` but *not*
in the dynamic symbol table (`LC_DYSYMTAB`'s external range) and not in the exports trie. A
symbolicator built on exports alone would name `_main` and miss the very function M18's fast-follow
exists to name. **M19 must read `LC_SYMTAB`/`nlist_64`, not the exports trie.**

## M2. The recorded crash pc resolves exactly, and the main executable's slide is zero

The `Event::Crash` recorded for `crashthread` (observed in this session's mutation run of
`a_wrong_thread_on_the_crash_landmark_is_a_divergence`):

```
guest crashed: pc=0x10000050c far=0x4000dead0000 esr=0x92000045
```

`0x10000050c − 0x1000004dc = 0x30`, so the faulting pc is **`_child+0x30`** — and `0x1000004dc` is
`nm`'s *un-slid* address, used with no adjustment whatsoever.

That is the second measurement: `retrace-box`'s `EXE_BASE = 0x1_0000_0000`
(`crates/retrace-box/src/lib.rs:143`) is identical to the executable's own `__TEXT` vmaddr, so the
main executable is loaded at **slide 0** and file addresses are guest addresses directly. No slide
arithmetic is needed for the main image. (This is a fact about the fixed IPA layout, not a
coincidence of one binary: the layout constants are chosen, not discovered.)

## M3. Symbol coverage varies enormously, and one rung is effectively stripped

```sh
nm <guest> | grep -c ' [TtSs] '      # defined text/data symbols
```

| binary | defined text syms | `nsyms` | verdict |
|---|---:|---:|---|
| `hello_dyn` (C) | 2 | 3 | enough — names `_main` |
| `crashy` (C) | 2 | 5 | enough |
| `crashthread` (C) | 2 | 6 | enough — names `_main` **and** `_child` |
| `dispatch_dyn` (C) | 4 | 11 | enough |
| `hello_rust` | 762 | 2904 | rich; names are **mangled** |
| `threadrust` | 969 | 3247 | rich; mangled |
| `/usr/lib/dyld` | — | 4015 | rich |
| **`/opt/homebrew/bin/jq`** | **7** | **81** | **effectively stripped** |

**`jq` is the honest limit.** Homebrew ships it stripped, so rungs 2–3 get almost nothing from
`LC_SYMTAB` no matter how good the symbolicator is. This is a property of the *binary*, not of
retrace, and the design must say so rather than implying uniform coverage. It is also the argument
for the fallback format in the design spec: an address that resolves to nothing must still print,
and must print as the raw address rather than as a lie.

Rust's 762–969 symbols are mangled (`_ZN…E` legacy form). Raw mangled names are strictly better than
hex and require no demangler; demangling is therefore a *separable* improvement, not a precondition.

## M4. `__LINKEDIT` is mapped into guest memory — the symbol table is already in the VM

`crates/retrace-guest/src/lib.rs`'s `LC_SEGMENT_64` arm pushes **every** segment with `vmsize > 0`
except `__PAGEZERO`:

```rust
if vmsize > 0 && name != b"__PAGEZERO\0\0\0\0\0\0" {
    segments.push(Segment { vaddr: vmaddr, memsz: vmsize,
        data: b[fileoff..fileoff+filesize].to_vec(), exec: initprot & 0x4 != 0 });
}
```

`__LINKEDIT` is such a segment, and for `crashthread` it lands at:

```
segname __LINKEDIT   vmaddr 0x100008000   fileoff 32768   filesize 792
```

`LC_SYMTAB.symoff` is `32960`, a **file** offset. Since `__LINKEDIT` covers file range
`[32768, 33560)` at vmaddr `0x100008000`, the symbol table's guest address is
`0x100008000 + (32960 − 32768)` = **`0x1000080c0`**. The string table (`stroff`) converts the same
way. The general rule: **a `LC_SYMTAB` file offset becomes a guest VA through the `__LINKEDIT`
segment's own `(fileoff → vmaddr)` mapping**, which is itself read from the same header.

## M5. `snapshot()` captures every backing, so the symbol table is in the *trace*

`crates/retrace-box/src/lib.rs:4195`:

```rust
pub fn snapshot(&self) -> retrace_trace::Event {
    let mut mem = Vec::new();
    for bk in &self.backings {
        let bytes = unsafe { std::slice::from_raw_parts(bk.host, bk.len) }.to_vec();
        mem.push(Region { ipa: bk.ipa, bytes });
    }
    ...
}
```

Every backing, in full. Combined with M4, the `nlist_64` array and the string table are **already
present in `Event::Snapshot`'s `mem`** of every recording ever made in the current format.

## M6. The trace carries no guest path — and, given M4+M5, does not need one

`Event` (`crates/retrace-trace/src/lib.rs`) has no path, UUID, or image-identity field on any
variant; `Snapshot { regs, mem }` carries bytes and nothing that names them.

Read alone, M6 looks like it forces a choice between a **format break** (add a path, bump
`TRACE_MAGIC` to `RT\x00\x09`, invalidate every existing recording) and a **CLI argument**
(`--exe <path>`, which can be wrong, missing, or a *different build* than the one recorded — a
silent mis-symbolication, which is worse than none).

M4 and M5 dissolve the choice. The symbols are in the snapshot, so symbolication reads the bytes the
trace already carries:

- **no format break** — `TRACE_MAGIC` stays `RT\x00\x08`, every existing recording keeps working and
  gains symbols retroactively;
- **no external file** — nothing to pass, lose, or mismatch; the symbols are *by construction* those
  of the binary that was actually recorded;
- **no new nondeterminism** — this is the point that matters most on this codebase. Symbolication is
  a pure function of recorded bytes plus fixed layout constants. It is a **presentation-layer**
  feature that never touches `record_box`, `ReplaySession::advance`, or `Box_::run()`, so neither
  symmetry rule is engaged and the divergence oracle is untouched. `clippy.toml`'s wall-clock and
  thread denials are likewise unthreatened.

## M7. Image routing is a range check over fixed constants

From `crates/retrace-box/src/lib.rs`:

| region | constant | value |
|---|---|---|
| main executable | `EXE_BASE` | `0x1_0000_0000` |
| dyld | `DYLD_BASE` | `0x1_4000_0000` |
| shared cache window | `SHARED_REGION_START` … `_END` | `0x1_8000_0000` … `0x3_0000_0000` |
| nano band | `NANO_BAND_START` … `_END` | `0x4_0000_0000` … `0xA_0000_0000` |
| mmap base | `MMAP_BASE` | `0xA_0000_0000` |

The layout is **fixed and identical on both runs** (it is what makes the whole recorder
deterministic), so deciding which image owns a pc is a comparison against compile-time constants —
no bookkeeping, no recorded state, nothing to keep in sync.

dyld is loaded through the **same** `parse_macho` as the main executable, so M4 applies to it
unchanged: its `__LINKEDIT` is mapped and its 4015 symbols are in the snapshot. The only difference
is the slide — dyld is at `DYLD_BASE`, so its file addresses need `+ (DYLD_BASE − dyld_text_vmaddr)`
where the main executable needed nothing.

**The shared cache is the exception and stays out of scope.** Cache images do not carry their own
`LC_SYMTAB` in the mapped region; the cache has a separate local-symbols area in the on-disk cache
file that `cache.rs` demand-pages but never stages into guest memory. Symbolicating a cache address
therefore *would* require reading the cache file at debug time — reintroducing exactly the external-
file dependency M6 just eliminated. It is deferred, and named as a known limit rather than left to be
discovered.

## What this measures *against* — the counts M19 must not disturb

At `04bf9ea` plus the fast-follow, before any M19 code: **444 `#[test]`** across 99 files, **one**
live `#[ignore]` (`stackoverflow_rust_e2e`, M8 risk R3). M19 is additive and presentation-only; any
change to the ignored count, or any e2e that stops passing, is a regression and not a trade.

## Open questions the measurements did *not* settle

1. **`nlist_64` type filtering.** `N_SECT`/`N_EXT`/`N_STAB` — debug (`N_STAB`) entries must be
   skipped or they pollute the table with source-file entries at address 0. Needs the header
   constants pinned in a test rather than assumed.
2. **Ties and zero-length symbols.** Several symbols can share an address (aliases). Nearest-
   preceding lookup must be deterministic under ties, which means a defined sort order, not
   whatever `sort_unstable` happens to do.
3. **Where the upper bound comes from.** A pc past the last symbol resolves to `last+huge`, which is
   a lie. `__TEXT`'s `vmaddr+vmsize` is the natural clamp and is available in the same header.
4. **Whether dyld lands in Stage 1.** The mechanism is identical (M7) and the payoff is large, but it
   doubles the surface. Resolved in the design spec's Scope section, not here.

---

## Post-design measurements — the three open questions, answered

Measured 2026-08-25, after the design spec, before any M19 code. These close the design spec's
"Open questions for implementation planning" (1–3) and the plan's Task 1.

### P1. `__LINKEDIT` is exactly one `Region` (open question 1 / R1)

Answered **statically**, from the loader rather than from a recording — which is stronger than a
one-guest observation, because it is a property of the code path rather than of one binary.

`Box_::load_with_pac`'s `map` closure (`crates/retrace-box/src/lib.rs:1053`) pushes **exactly one**
`Backing` per call:

```rust
let map = |vm: &Vm, backings: &mut Vec<Backing>, ipa: u64, src: &[u8], memsz: usize| {
    let (host, len) = alloc_pages(memsz.max(src.len()).max(GRANULE));
    ...
    backings.push(Backing { host, ipa, len });
};
```

and the image loaders call it **once per segment**:

```rust
for s in &exe.segments  { map(&vm, &mut backings, s.vaddr,             &s.data, s.memsz); }  // :1614
for s in &dyld.segments { map(&vm, &mut backings, s.vaddr + DYLD_BASE, &s.data, s.memsz); }  // :1616
```

So for the main executable and dyld, **one segment is one `Backing`, hence one `Region`**, and
`__LINKEDIT` is contiguous and never split.

**The gathering helper is still built**, for a reason this measurement makes precise rather than
hypothetical: *other* backings in the same snapshot genuinely are per-page (the mmap and
demand-commit paths at `:1209`, `:1229`, `:1241`, `:1249`, `:1288` each push a page-sized `Backing`).
A reader that assumed "one region per lookup" would be correct for `__LINKEDIT` today and wrong the
first time anything else is read. The helper costs ~15 lines and removes the assumption entirely.

### P2. `nlist_64` constants, verified on this machine (open question 2 / R2)

From `/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk/usr/include/mach-o/nlist.h`
(lines 117–135) — **measured, not attributed**:

| constant | value | meaning |
|---|---|---|
| `N_STAB` | `0xe0` | any bit set ⇒ symbolic **debugging** entry (skip) |
| `N_PEXT` | `0x10` | private external |
| `N_TYPE` | `0x0e` | mask for the type bits |
| `N_EXT`  | `0x01` | external |
| `N_UNDF` | `0x0` | undefined (`n_sect == NO_SECT`) |
| `N_ABS`  | `0x2` | absolute |
| **`N_SECT`** | **`0xe`** | **defined in section `n_sect`** — what M19 keeps |
| `N_PBUD` | `0xc` | prebound undefined |
| `N_INDR` | `0xa` | indirect |

**One footgun, written down because it is exactly the kind of thing that silently half-works:**
`N_SECT` (`0xe`) is numerically equal to the `N_TYPE` mask (`0x0e`). The test is
`n_type & N_TYPE == N_SECT`, and a reader that slips into `n_type & N_SECT != 0` compiles, looks
plausible, and accepts `N_PBUD` (`0xc`) and `N_INDR` (`0xa`) as if they were defined symbols — both
of which have `n_value`s that are not addresses. The unit test for `N_STAB` rejection should carry a
`N_INDR` case alongside it for this reason.

### P3. dyld's slide is `DYLD_BASE` itself (open question 3 / R3) — *and the design spec was wrong*

```sh
otool -l /usr/lib/dyld | awk '/segname __TEXT/{f=1} f&&/vmaddr/{print; exit}'
#   vmaddr 0x0000000000000000
```

dyld's `__TEXT` vmaddr is **`0x0`**, and the loader adds `DYLD_BASE` to each segment's vmaddr
(`:1616`). So the slide is exactly **`DYLD_BASE` = `0x1_4000_0000`**.

**This corrected the design spec's R3 mitigation**, which as first drafted said to derive the slide
as `DYLD_BASE − dyld __TEXT vmaddr`. That expression happens to yield the right number here *only
because the vmaddr is zero*; it is the wrong rule, and it would produce a confidently-wrong slide for
any image whose vmaddr is not zero — R3's own named failure mode, reached by the mitigation meant to
prevent it. The spec's R3 row was **edited in place**: append-only is the discipline for
`docs/status-log.md`, which records what was believed at a past milestone, and neither spec had been
committed or acted on when the measurement landed. This paragraph is the record that it changed.

The correct rule is the one the loader itself uses, and it is uniform across both images:

> **`guest_va = file_vmaddr + slide`**, where `slide` is `0` for the main executable and `DYLD_BASE`
> for dyld.

The file-offset conversion in the plan's Task 2 Step 3 —
`va = linkedit_vmaddr + (fileoff − linkedit_fileoff) + slide` — is unaffected and correct as written.

Worked example for dyld, to be asserted in the plan's Task 4:
`__LINKEDIT` vmaddr `0xe0000` + slide `0x1_4000_0000` ⇒ guest VA **`0x1_400e_0000`**.
