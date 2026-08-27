# M20-symbolops design

**Goal:** `break _main` and `delete _main` accept a symbol name wherever they accept an address
today — or park a gate at a wall M20 actually reached and measured.

**Measurements:** `2026-08-27-retrace-m20-symbolops-measurements.md`, cited as **S1–S6**. M19's are
cited as M1–M7 / P1–P3.

## The problem, precisely

M19 made the debugger *print* names. Every debugger **operand** is still a raw address, so the tool
is asymmetric in a way that costs the user real work: it reports `in _child+0x30`, and to put a
breakpoint there you must go read `nm` yourself and type the hex back. The information the CLI just
printed is the information it will not accept.

## What the measurements settled, and what they changed

- **S1** is the binding constraint: `parse_script` completes before `Exec::new` opens the trace, so
  the symbol table does not exist at parse time. This decides *where* resolution happens and
  forecloses the obvious implementation.
- **S4** is the hard problem, and it was not visible from M19's code: **name → address is not a
  function.** 19 ambiguous names in `threadrust`, **3255** in dyld.
- **S2** removed the compatibility worry: exit codes already coincide at 5. What changes is ordering.
- **S5** put `watch <name>` out of scope on evidence rather than taste: `nlist_64` has no size field.

## The mechanism

### Resolution happens at execution, not at parse (S1)

`Cmd` stops carrying a bare `u64` for the two commands in scope and carries an operand:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Operand { Addr(u64), Sym(String) }
```

`parse_one` classifies the token and **never fails on an unknown name**; `Exec` resolves it when it
runs the command, where `self.syms` is in scope.

The rejected alternative — build `Exec` first, then parse with the table available — is rejected on
the documented contract at `debug.rs:11-12` that an over-long `x` span is *deliberately* a parse
error raised before any VM work (S1). Reordering would move every parse diagnostic behind VM setup
to buy a smaller diff.

**Accepted, measured cost (S2).** A bad operand no longer rejects the script before it starts:
`where; break zzz` today prints nothing and exits 5; under M20 it prints the `where` line, then
fails, and still exits 5. This is stated here, pinned by a test, and listed in the README — not
discovered later.

### Classifying a token — a decided rule, not a fallback chain

`deadbeef` is valid hex *and* a valid identifier. The rule, in order:

1. `0x` / `0X` prefix ⇒ **address**, always. An explicit prefix is explicit intent.
2. Parses completely as hex ⇒ **address**. This is what keeps every existing script working
   verbatim, and it is the reason the rule is ordered this way rather than preferring names.
3. Otherwise ⇒ **symbol name**.

Rule 2 means a symbol literally named `deadbeef` is unreachable. That is **documented, not papered
over**: Mach-O C symbols carry a leading underscore (M1), so `_deadbeef` lands on rule 3 cleanly, and
mangled Rust names (`_ZN…E`) are never all-hex. If an escape hatch is ever needed a sigil is the
additive follow-up — M20 does not build one speculatively.

### Ambiguity is an error, not a guess (S4)

`Symbols` gains the reverse direction, returning **every** match:

```rust
pub fn addrs_of(&self, name: &str) -> Vec<u64>;   // sorted, deduped
```

and the CLI's rule is:

| matches | behaviour |
|---|---|
| 0 | error: `no symbol "foo"` |
| 1 | use it |
| >1 | error naming the count **and the addresses**, so the user re-issues with `break 0x…` |

Picking the lowest silently is rejected: it is R3's confidently-wrong class, and S4 says it would
fire on real names 3255 times over in dyld. The `>1` message must list the addresses, because a
message that only says "ambiguous" leaves the user with no way forward — and the addresses are
exactly what M19 taught the debugger to print.

**Cross-image precedence is a decided rule, not vector order.** `Symbols` holds `Vec<SymbolTable>` as
(exe, dyld). Name lookup searches the **executable first** and returns dyld's matches only when the
executable has none, so a guest symbol shadows a dyld symbol of the same name. Stated and tested
rather than inherited from a field's declaration order. Measured mitigation: dyld does not define
`_main` (S4), so the common case has nothing to arbitrate.

### The break echo closes the loop

`cmd_break` currently prints `breakpoint at {addr:#x}`. It will print `Symbols::format(addr)`
instead — `breakpoint at 0x100001130 (_main)` — so a user who typed a name sees what it resolved to,
and a user who typed hex learns where they landed.

This deliberately extends M19's "pc-bearing lines only" list by one. The justification is specific to
M20: when the operand itself can be a name, confirming the resolution *is* the feedback, and it is
what makes the ambiguity error above actionable. Verified safe: both existing assertions on that line
use `contains("breakpoint at 0x…")`, not `ends_with`, so an appended suffix cannot break them — the
check M19's four broken assertions earned.

### Scope

**In:** `break`, `delete`.

**Out, each on a measured reason, not on effort:**

- `watch <name>` / `unwatch <name>` — **S5**: `nlist_64` carries no size, so a watch width would have
  to be invented, and a watch of the wrong width silently misses writes to the bytes it failed to
  cover. That is the same quiet wrongness S4 rules out for addresses, so it is refused for the same
  reason. A different milestone wearing the same syntax.
- `x <name>` — same missing length (S5).
- Demangling — M3 already ruled it separable. `break _ZN…E` works today by exact match.

## Fail-loud boundaries

- An ambiguous name **errors listing candidates**; it never picks one.
- A name with no match errors; it never falls back to reinterpreting the token as hex. Falling back
  would turn a typo into a breakpoint at a wrong-but-valid address — the failure the whole design is
  arranged against.
- A **stripped** guest resolves nothing, so every name errors with `no symbol`. That is truthful, and
  `jq` (7 symbols, M3) is the adversary that proves it.
- Symbolication still may not fail a *session* — but a command that cannot be carried out must fail.
  These are not in tension: M19's rule is that a missing *name* costs nothing when the debugger is
  merely printing; M20's is that a missing *address* means there is nothing to execute.

## Exit criterion

On a `crashthread` recording, `break _child; continue` stops **at `_child`'s address**, and the
assertion is on the **pc it stops at** — not on the parse succeeding and not on the echo text, both
of which a no-op that merely accepted the token would also produce.

## Risk register

| # | risk | failure mode | mitigation |
|---|---|---|---|
| R1 | An ambiguous name resolves to the wrong address | breakpoint stops somewhere else; transcript looks normal | `addrs_of` returns all; >1 is an error listing them (S4) |
| R2 | An existing script breaks because a hex token is read as a name | previously-working debug scripts fail | Rule 2: anything that parses fully as hex stays an address; `debug_cli`/`watch_cli`/`reverse_debug_e2e` are the regression corpus |
| R3 | Deferred resolution changes observable ordering | partial output before an error where there was none | Accepted and measured (S2); pinned by a test |
| R4 | `debug.rs:763` deleted rather than replaced | the new classification rule ships untested | Plan names the replacement assertion explicitly (S3) |
| R5 | The break-echo change breaks an assertion | gate red late, as in M19 | Pre-checked: both assertions use `contains`, not `ends_with` |

## Components

- `crates/retrace-core/src/symbols.rs` — `addrs_of`, exe-before-dyld precedence. Pure, VM-free,
  unit-tested against synthetic images.
- `crates/retrace/src/debug.rs` — `Operand`, classification in `parse_one`, resolution in `Exec`,
  the break echo.
- `crates/retrace/tests/symbolops_e2e.rs` — the headline gate and the ordering/ambiguity pins.

## Open questions for implementation planning

1. Does `Operand` live in `debug.rs` or `symbols.rs`? It is CLI grammar, not symbol-table
   knowledge — leaning `debug.rs`, keeping `symbols.rs` free of CLI concepts.
2. Where does the resolution error surface — a `Result` from `exec`, matching the existing
   `Err(String)` → exit 5 path, or a printed line and a non-fatal continue? Leaning `Result`, since
   S2 shows exit 5 is already the shared destination and a script whose breakpoint failed should not
   pretend to continue.
3. Should `addrs_of` match mangled names by suffix/substring as a convenience? Leaning **no** for
   M20: substring matching reintroduces ambiguity by construction, which is the thing S4 says to
   refuse. Exact match only, and let a later milestone measure whether it is needed.
