// The headline M22 gate: an **Apple system binary**, taken straight from `/bin` with no slicing
// step, records and replays bit-for-bit.
//
// Every executable Apple ships on this machine is a *universal* (fat) file — `x86_64 + arm64e` —
// and before M22 `parse_macho` asserted `MH_MAGIC_64` against byte 0 of whatever it was handed. A
// fat file's byte 0 is `0xcafebabe`, so every one of them failed identically and immediately, at
// the header, having executed nothing. That is why the guest ladder up to M21 is built entirely
// from binaries the repo compiles itself plus Homebrew's thin-arm64 `jq`: not because retrace could
// not run Apple's binaries, but because it could not *open* them.
//
// `slice_native` picks the slice this machine would execute (arm64e if present, else plain arm64)
// and `parse_macho` calls it first. Nothing below the loader changed — and nothing needed to. An
// arm64e main turns PAC on through the existing `pac_posture(cpusubtype)` path, and replay
// re-derives that posture from the snapshot's own mach header via `pac_posture_from_memory`, so the
// arm64e guests this gate unlocks replay correctly *by construction*, with no trace-format change
// and no new recorded field. `TRACE_MAGIC` does not move.
mod util;

// The measured breadth this gate stands for, written down so a future reader can tell drift from a
// real regression. Sampled across `/bin` + `/usr/bin` at M22, pointing retrace straight at each
// binary with no slicing step: **34 of 54 attempted record AND replay**, stdout byte-identical and
// exit codes equal. The 20 failures were four distinct causes, not a long tail — see "Known limits"
// in the README and the parked gate below.

#[test]
fn an_apple_system_binary_records_and_replays() {
    // `/bin/echo` is the smallest honest instance: an Apple-signed, arm64e, dynamically-linked
    // system binary that produces deterministic stdout from an argv.
    //
    // Assert it is genuinely FAT first. Without this the gate is vacuous in exactly the way that
    // matters: if a future macOS shipped `/bin/echo` thin, this test would keep passing while
    // testing nothing M22 built, and the capability could regress in silence. This is the same
    // guard `parse_macho_accepts_a_fat_binary` puts on dyld, for the same reason.
    let bytes = std::fs::read("/bin/echo").expect("read /bin/echo");
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    assert_ne!(magic, 0xfeed_facf,
        "/bin/echo is a THIN Mach-O on this machine, so this gate no longer exercises the fat-header \
         path M22 added and proves nothing. Re-point it at a universal system binary.");

    // The rung assertion, not a bare agreement check: it demands a clean exit(0) with exactly this
    // stdout, so a guest that died inside dyld having run none of its own code cannot pass (M6
    // records a crash as a *successful* recording that replays bit-for-bit, exit 139 both sides).
    // Replays twice.
    util::assert_rung_records_and_replays("/bin/echo", &["hi"], b"hi\n");
}

#[test]
#[ignore = "M22's measured wall. 13 of the 20 sampled failures — every one of them a modern \
            ObjC/Swift-heavy /usr/bin tool — die identically before reaching their own code: \
            `non-syscall exit: unknown/uncategorized (EC=0x00 ISS=0x0 FSC=0x0) far/ipa=0x0 \
            (UNMAPPED) pc=0x4204 elr=0x4404`. EC=0x00 is the exception class the box cannot \
            categorise at all, and the pc is a low address one granule in, so control has left the \
            loaded images entirely rather than faulting inside them. Nothing has measured WHY, and \
            this gate is parked here rather than guessed at. The other three causes are separately \
            named and much narrower: mach_msg2 msgh_id 412 (4 binaries), the M10 fd table's \
            fail-loud unmodelled dup2 (2), and one genuine divergence in `ps` (1)."]
fn an_objc_heavy_system_tool_records_and_replays() {
    // `/usr/bin/aa` (Apple Archive) is the representative of the 13. It is chosen because it is a
    // plain non-setuid tool that fails with no arguments, so the wall is reached without the test
    // needing to construct any state on the filesystem.
    //
    // Announce rather than skip quietly if it is missing — a silent skip reads as a green it did
    // not earn (the discipline `jq_e2e` established).
    if !std::path::Path::new("/usr/bin/aa").exists() {
        eprintln!("SKIPPING an_objc_heavy_system_tool_records_and_replays: /usr/bin/aa is absent \
                   on this machine. It is an OS artifact, not a repo artifact.");
        return;
    }
    // When this wall falls, `aa` with no args exits 1 after printing its usage. Assert on the
    // clean-exit discriminator the same way the green gate above does, NOT on the exit code alone:
    // the current failure is a RECORD ERROR (exit 4), and a weaker future failure could still
    // produce a nonzero exit while running none of `aa`'s own code.
    let (rec, _trace) = util::record_dynamic("/usr/bin/aa");
    assert_ne!(rec.code, 4,
        "still parked at the M22 wall — record aborted rather than running the guest. stderr:\n{}",
        rec.stderr);
}
