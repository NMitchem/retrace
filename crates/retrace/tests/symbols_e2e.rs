// M19 headline gate: the debugger stops printing bare hex.
//
// The milestone's whole claim is that symbols need no trace-format change and no binary path,
// because `parse_macho` maps `__LINKEDIT` into guest memory (measurements M4) and `Box_::snapshot`
// captures every backing in full (M5) — so the `nlist_64` array and the string table are already
// inside every recording. The unit tests in `retrace-core::symbols` prove the READER against
// synthetic Mach-O images; they cannot prove that claim, because a synthetic image is one the test
// built itself. Only a real recording can, and that is what this file is for.
//
// `crashthread` is the guest, for the reason M19 exists: it is the M18 fast-follow fixture whose
// entire point is *which thread faulted*, and the answer it printed before this milestone was
// `pc=0x10000050c`.

mod util;
use retrace_trace::Event;

/// The design's load-bearing claim, tested against a real recording rather than a synthetic image.
///
/// If this fails, M19's premise is wrong and the milestone needs a format change or an `--exe` flag
/// after all — so this is the test to read first if symbolication ever stops working.
#[test]
fn the_recorded_snapshot_alone_names_the_faulting_function() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHTHREAD);
    assert_eq!(rec.code, 139,
        "the child must actually fault for this trace to carry a Crash landmark; stderr:\n{}",
        rec.stderr);

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let mem = events.iter().find_map(|e| match e {
        Event::Snapshot { mem, .. } => Some(mem), _ => None })
        .expect("a recording opens with a Snapshot");
    let (pc, thread) = events.iter().find_map(|e| match e {
        Event::Crash { pc, thread, .. } => Some((*pc, *thread)), _ => None })
        .expect("a guest that faults with no handler records a terminal Event::Crash");

    // Built from the snapshot's regions ALONE — no path, no file, nothing but the trace.
    let syms = retrace_core::symbols::Symbols::from_snapshot(mem);
    let (name, off) = syms.resolve(pc)
        .unwrap_or_else(|| panic!("the crash pc {pc:#x} must resolve; if this is None, __LINKEDIT \
            is not reaching the snapshot and measurements M4/M5 need re-measuring"));

    // The strong assertion, and the reason the fixture is threaded: the faulting function is the
    // CHILD. `_main` resolving here would mean the nearest-preceding lookup walked to the wrong
    // symbol, which is exactly the confidently-wrong-name failure R3 names.
    assert_eq!(name, "_child",
        "the faulting pc must resolve to the child's function, got {name}+{off:#x}");
    assert_ne!(thread, 0, "and the Crash landmark must be tagged with that same child thread");

    // The offset is a real number from a real run, so assert it is sane rather than pinning an exact
    // value a recompile would move: the store is a few instructions into a short function.
    assert!(off < 0x200, "offset into _child should be small, got {off:#x}");

    // The raw address must survive alongside the name — every existing hex-matching assertion in
    // the tree depends on it.
    let formatted = syms.format(pc);
    assert!(formatted.contains("_child") && formatted.contains(&format!("{pc:#x}")),
        "format must carry BOTH the name and the raw address; got {formatted}");
}

/// A guest whose symbols cannot help must still work. `hello_dyn` is not stripped, but the shared
/// cache it runs through is exactly the wall `cache_symbol_e2e` is parked at, so most of its
/// interesting addresses resolve to nothing — and nothing is what they must resolve to, silently.
#[test]
fn an_address_with_no_symbol_degrades_to_bare_hex() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let mem = events.iter().find_map(|e| match e {
        Event::Snapshot { mem, .. } => Some(mem), _ => None }).expect("opening Snapshot");
    let syms = retrace_core::symbols::Symbols::from_snapshot(mem);

    // An address in the shared-cache window: out of scope by construction, and the parked wall.
    let cache_addr = 0x1_8000_0000u64 + 0x1234;
    assert_eq!(syms.format(cache_addr), format!("{cache_addr:#x}"),
        "a cache address must print as bare hex — never a nearest guess from another image");

    // Vacuity guard, and it is the point of this half of the test: if `syms` were empty the
    // assertion above would pass for the wrong reason — everything formats as bare hex when there
    // is nothing to resolve against. Prove the table is real by finding at least one address in the
    // main executable that DOES resolve. (0x1_0000_0000 is `retrace_box::EXE_BASE`, spelled as a
    // literal because retrace-box is not a dependency of this crate's tests.)
    let resolved = (0..0x4000u64).step_by(4).any(|o| syms.resolve(0x1_0000_0000 + o).is_some());
    assert!(resolved,
        "hello_dyn's own image must yield a usable table, or the bare-hex assertion above is vacuous");
}

/// **The milestone's exit criterion.** `retrace debug` on a recording of `crashthread`, stopped at
/// the crash, names the faulting function — from the trace alone, with no binary path supplied and
/// no format change.
///
/// Per honest-gate discipline the assertion is the **name**, never the address: the raw address was
/// printed before M19 too, so asserting on `0x…` would pass against a no-op. `_child` is the thing
/// this milestone made appear, so `_child` is what this test pins.
#[test]
fn the_debug_cli_names_the_faulting_function() {
    let (rec, trace) = util::record_dynamic(retrace_guest::CRASHTHREAD);
    assert_eq!(rec.code, 139, "the child must fault; stderr:\n{}", rec.stderr);

    let out = std::process::Command::new(util::bin())
        .args(["debug", trace.to_str().unwrap(), "--script", "continue; where"])
        .output().expect("spawn debug");
    assert_eq!(out.status.code(), Some(0), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
    let out = String::from_utf8(out.stdout).unwrap();

    let crash = out.lines().find(|l| l.starts_with("guest crashed:"))
        .unwrap_or_else(|| panic!("no crash line:\n{out}"));
    assert!(crash.contains("_child"),
        "M19's whole difference: the crash line must NAME the faulting function, not just address \
         it. Got:\n{crash}\nfull transcript:\n{out}");

    // The raw address survives alongside the name — `crashy_cli` and friends grep for it.
    assert!(crash.contains("pc=0x") && crash.contains("far=0x") && crash.contains("esr=0x"),
        "the pre-M19 fields must all survive:\n{crash}");

    // And `where`, parked at the fault, names it too — the annotation is on the position line as
    // well as the crash line, which is what makes it useful while stepping.
    let last = out.trim_end().lines().last().unwrap_or("");
    assert!(last.contains("_child") && last.contains("thread="),
        "the parked `where` line must name the function too:\n{last}");
}

/// Task 4: dyld is a second image, and it resolves.
///
/// Same mechanism as the main executable, differing only by slide — which is `DYLD_BASE` itself,
/// because dyld's `__TEXT` vmaddr is `0` and the loader adds `DYLD_BASE` to every segment (P3). R3's
/// failure mode is a confidently WRONG name rather than a missing one, so this asserts that a
/// resolved dyld address lands at a sane offset into a real, non-empty name — not merely that
/// something came back.
///
/// Deliberately does NOT pin a specific dyld symbol or address: those are properties of whatever
/// `/usr/lib/dyld` this machine shipped, and an OS update would turn a genuine pass into a spurious
/// red. What is pinned is the property M19 owns — that the dyld image contributes a usable table.
#[test]
fn dyld_is_a_second_image_and_resolves() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);

    let events = retrace_trace::Reader::open(&trace).unwrap();
    let mem = events.iter().find_map(|e| match e {
        Event::Snapshot { mem, .. } => Some(mem), _ => None }).expect("opening Snapshot");
    let syms = retrace_core::symbols::Symbols::from_snapshot(mem);

    // Re-exported through `symbols` rather than re-declared here: `retrace-box` is not a direct
    // dependency of this crate, but a second copy of the constant would drift silently and fail as
    // a confidently WRONG name (R3), never as a compile error.
    use retrace_core::symbols::DYLD_BASE;
    let hit = (0..0x40000u64).step_by(4)
        .find_map(|o| syms.resolve(DYLD_BASE + o).map(|(n, off)| (o, n.to_string(), off)));
    let (o, name, off) = hit.expect(
        "dyld's image must yield a usable table: its __LINKEDIT is mapped by the same parse_macho \
         and captured by the same snapshot() as the main executable's (M4/M5, M7)");
    assert!(!name.is_empty(), "a resolved dyld symbol must have a real name");
    assert!(off <= o, "the offset cannot exceed the distance from the image base; {name}+{off:#x}");
}

/// **PARKED at the shared-cache wall.** Kept as a test rather than a comment so the wall is a thing
/// that reports itself, per honest-gate discipline: a milestone that parks a new gate for a
/// capability it does not have has regressed nothing.
///
/// Most of a dynamically-linked guest's executing pcs are in the dyld shared cache, not in its own
/// image or dyld — so this is the difference between "M19 names your functions" and "M19 names
/// everything", and a reader deserves to see exactly where it stops.
///
/// The wall, measured (M7) rather than assumed: cache images carry no `LC_SYMTAB` in the region that
/// is mapped into the guest. The cache's local-symbol area lives in a separate part of the on-disk
/// cache file, which `cache.rs` demand-pages for its *page contents* but never stages into guest
/// memory. So unlike the main executable and dyld — whose `__LINKEDIT` the loader maps and
/// `snapshot()` captures, which is the whole reason M19 needed no format change (M4/M5) — a cache
/// symbol is simply not in the recording.
///
/// **What clearing it would owe.** Reading the on-disk cache at debug time would reintroduce exactly
/// the external-file dependency M6 eliminated, and with it the stale-file mis-symbolication that
/// dependency makes possible: the cache on disk at debug time need not be the cache that was
/// recorded. A real fix therefore has to either (a) stage the cache's local-symbol area into guest
/// memory at record time, which is a recording-size and determinism question, not a formatting one,
/// or (b) record a cache identity the debugger can verify a local file against before trusting it.
/// Neither has been measured, and this test asserts nothing until one is.
#[test]
#[ignore = "M19 wall: shared-cache addresses carry no symbols in the recording. Cache images have \
no LC_SYMTAB in the mapped region, and the cache's local-symbol area lives in the on-disk cache file \
that cache.rs demand-pages but never stages into guest memory — so unlike the exe and dyld, whose \
__LINKEDIT is mapped and snapshotted (M4/M5), a cache symbol is not in the trace at all. Clearing \
this owes a measurement: either stage the local-symbol area at record time (a determinism and \
recording-size question), or record a cache identity the debugger can verify a local file against. \
Until then a cache pc prints as bare hex, which an_address_with_no_symbol_degrades_to_bare_hex pins."]
fn cache_symbol_e2e() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "clean exit; stderr:\n{}", rec.stderr);
    let events = retrace_trace::Reader::open(&trace).unwrap();
    let mem = events.iter().find_map(|e| match e {
        Event::Snapshot { mem, .. } => Some(mem), _ => None }).expect("opening Snapshot");
    let syms = retrace_core::symbols::Symbols::from_snapshot(mem);

    // The assertion this gate will make when the wall falls: a pc inside the shared-region window
    // resolves to a libSystem symbol. Today it resolves to nothing, which is why this is #[ignore]d
    // rather than deleted — the shape of the eventual assertion is the useful part of parking it.
    let cache_pc = 0x1_8000_0000u64 + 0x1234;
    let (name, _off) = syms.resolve(cache_pc)
        .expect("a shared-cache pc must resolve once the cache's local symbols reach the recording");
    assert!(!name.is_empty(), "and it must be a real name");
}
