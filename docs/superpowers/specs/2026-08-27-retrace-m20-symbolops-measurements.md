# M20-symbolops measurements

Numbers taken before the design was written, on this machine, at `88aa758` (the M19 close). Tagged
**S1–S6** and cited by tag from the design spec and plan rather than restated there.

M19's measurements are tagged M1–M7 / P1–P3 in
`2026-08-25-retrace-m19-symbols-measurements.md`; where this document leans on one it says so by
that tag.

## Why measure before designing

M19's own risk register named "confidently wrong names" as the failure mode to fear, and then
prescribed a mitigation that contained the bug it was guarding against — caught only when a number
was put next to it (P3). M20 points the same machinery in the opposite direction, where a wrong
answer is *acted on* rather than merely printed: a breakpoint set at the wrong address stops
somewhere else, and the transcript looks entirely normal. So the same discipline applies harder.

## S1. Parsing completes before the symbol table exists

`crates/retrace/src/debug.rs:737`:

```rust
pub fn run_script(trace: &Path, script: &str, out: &mut impl Write) -> Result<(), String> {
    let cmds = parse_script(script)?;          // all parsing, including parse_addr
    let segs: Vec<&str> = segments(script).collect();
    let mut ex = Exec::new(trace)?;            // trace opened; Exec::syms built here (M19)
```

`parse_script` runs to completion **before** the trace is opened, so `Exec::syms` does not exist
during parsing. **M20 cannot resolve names inside `parse_addr`.** This is the binding constraint on
the design, not an implementation detail.

The alternative — construct `Exec` first, then parse with the table in hand — is foreclosed by a
contract stated at `debug.rs:11-12`:

> The `x <addr> <len>` length ceiling: a larger span is a *parse* error (deterministic Err → exit
> 5), guarding the inherited u64 span-overflow edge at the CLI boundary **before any VM work**.

Reordering would move every parse diagnostic behind VM setup.

## S2. The exit code does not change; the *timing* does

`main.rs:106-108` has a single `Err` arm, so parse errors and execution errors already share exit 5.
Measured on a real `jq` recording:

```sh
retrace debug t.bin --script "break zzz"
#   DEBUG ERROR: bad hex address: zzz          EXIT=5
retrace debug t.bin --script "frobnicate"
#   DEBUG ERROR: unknown command: frobnicate   EXIT=5
retrace debug t.bin --script "where; break zzz"
#   DEBUG ERROR: bad hex address: zzz          EXIT=5      <-- NO `where` output at all
```

The third is the one that matters. Today a bad operand rejects the **whole script before any command
runs**, so `where` never executes. Under M20 — resolution deferred to execution (S1) — `where` runs
and echoes first, and the failure arrives after it.

That is a real, observable behaviour change and the honest cost of keeping the parse pure. It is
**not** an exit-code change, which is the compatibility question one would expect to be hard and
turns out not to be.

## S3. Exactly one existing assertion changes meaning

Every `parse_script(...).is_err()` in the tree:

```sh
grep -rn "parse_script(" crates/retrace/src crates/retrace/tests | grep is_err
```

| site | token | under M20 |
|---|---|---|
| `debug.rs:762` | `frobnicate` — unknown **verb** | **unchanged**, still a parse error |
| `debug.rs:763` | `break zzz` — non-hex **address operand** | **changes**: `zzz` becomes a name that may not resolve |

Every other `is_err()` assertion in `debug.rs` (arity, watch length/alignment, thread-id syntax) stays
parse-time. Blast radius on existing tests is therefore **one line** — small enough that the
temptation is to delete it. It must be **replaced** by an assertion of the new rule, or the rule ships
untested.

## S4. Name → address is NOT a function — the milestone's hard problem

```sh
nm <bin> | grep ' [TtSs] ' | awk '{print $NF}' | sort | uniq -d | wc -l
```

| image | defined syms (M3's `[TtSs]` rule) | **names bound to >1 address** |
|---|---:|---:|
| `crashthread` (C) | 2 | **0** |
| `threadrust` (Rust) | 969 | **19** |
| `/usr/lib/dyld` | 6331 defined text | **3255** |

M19's direction is total and unambiguous — an address falls inside exactly one symbol's range. **The
reverse is not.** Duplicates in `threadrust` are compiler-generated locals (`_OUTLINED_FUNCTION_0`,
`GCC_except_table0`, …) repeated per translation unit, which Mach-O keeps all of; dyld has 3255 such
names.

So `break <name>` is not a lookup — it is a query that may return many rows, and the design must say
what happens then. "Pick the lowest" would fire silently on real names 3255 times over in dyld alone.

**The reassuring half**, also measured: the names anyone actually types are unique. `_main` is unique
in every guest checked, `_child` is unique in `crashthread`, and

```sh
nm /usr/lib/dyld | grep ' [tT] _main$'     # (no output)
```

dyld does **not** define `_main`, so the common case has no cross-image collision to arbitrate.

## S5. `nlist_64` carries no size — which is what puts `watch <name>` out of scope

From this machine's SDK header
(`$(xcrun --show-sdk-path)/usr/include/mach-o/nlist.h`):

```c
struct nlist_64 {
    union { uint32_t n_strx; } n_un;
    uint8_t  n_type;
    uint8_t  n_sect;
    uint16_t n_desc;
    uint64_t n_value;
};
```

Five fields, 16 bytes, **no size**. A symbol therefore supplies an address and nothing else.

`watch` takes `<addr> [len]` and `x` takes `<addr> <len>`. Neither length can come from a symbol
name, so `watch _global` would have to invent a default — and a watch of the wrong width silently
misses writes to the bytes it failed to cover, which is the same class of quiet wrongness S4 rules
out for addresses. **Out of scope for M20, on this measurement rather than on taste.**

## S6. Data symbols are kept by the reader but unreachable through `resolve`

Source-read, not a runtime measurement, and labelled as such. `symbols.rs`'s filter
(`for_image`) keeps any defined symbol:

```rust
if n_type & N_TYPE != N_SECT || n_sect == 0 { continue; }   // ANY section, not just __TEXT
```

while `resolve` clamps:

```rust
if addr >= self.text_end { return None; }
```

So a `__DATA` symbol is **in** `syms` but can never be returned by `resolve`, whose contract is
address → name. A future name → address lookup would reach data symbols essentially for free, which
is worth knowing but is **not** licence to ship `watch <name>`: S5 is the blocker there, and it is
independent of this.

To be confirmed by a test during implementation rather than asserted here.

## What this measures *against* — the counts M20 must not disturb

At `88aa758`: **463 `#[test]`**, of which **461 run** and **2 are `#[ignore]`d**
(`stackoverflow_rust_e2e` at M8 R3, `cache_symbol_e2e` at the M19 shared-cache wall), across **105
test binaries**. `TRACE_MAGIC` is `RT\x00\x08` and must not move: M20, like M19, is presentation-layer
and records nothing.

## Open questions the measurements did *not* settle

1. Should the resolved address be echoed back (`breakpoint at 0x100000460 (_main)`)? A presentation
   choice no measurement decides.
2. Should `Operand` be threaded through *every* address-taking `Cmd`, or only `break`/`delete`? A
   uniform type is tidier but widens the diff into commands whose symbol support S5 rules out — and
   a type that permits what the CLI rejects invites the reader to assume support that is not there.
3. Linear scan or an index for name lookup? dyld's 6331 defined text symbols bound the cost; nothing
   has measured whether a scan is perceptible. Measure before optimising.
