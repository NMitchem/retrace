use std::process::Command;
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let src = format!("{}/asm/hello.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/hello");
    println!("cargo:rerun-if-changed={src}");
    // -static (not -no_pie): the macOS 26 linker rejects a -nostdlib dynamic
    // executable ("must link with libSystem.dylib") regardless of -no_pie,
    // which is itself deprecated/ignored on arm64. -static sidesteps dyld
    // entirely, which is what we want anyway: retrace maps each LC_SEGMENT_64
    // at its linked vmaddr itself, so PIE-ness was never load-bearing here.
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang");
    assert!(status.success(), "guest assembly failed");

    // steppy: nop×4; mrs cntvct_el0 (one step whether it traps+emulates or retires natively);
    // nop×3; then hello.s's exit(0) svc sequence. The M3 single-step micro-guest — Box_::step()
    // must advance exactly one instruction per call.
    let src = format!("{}/asm/steppy.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/steppy");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang steppy");
    assert!(status.success(), "steppy guest build failed");

    // spinloop: write(1,"spin!\n",6) after a ~606-insn spin, then exit(0) after a ~4003-insn spin.
    // Landmark 1's window is modest (clears a cost-gate cache threshold); landmark 2's window is
    // deliberately huge — the M4 checkpoint acceleration's synthetic target.
    let src = format!("{}/asm/spinloop.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/spinloop");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang spinloop");
    assert!(status.success(), "spinloop guest build failed");

    // Fixture file with known contents.
    let fixture = format!("{out}/fixture.txt");
    std::fs::write(&fixture, b"retrace-m1-fixture\n").unwrap();
    // Generate the path constant appended to the guest asm.
    let gen = format!("{out}/fileio_gen.s");
    std::fs::write(&gen, format!(".section __DATA,__data\n.p2align 3\n.global path\npath: .asciz \"{fixture}\"\n")).unwrap();
    let src = format!("{}/asm/fileio.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/fileio");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src,&gen])
        .status().expect("clang fileio");
    assert!(status.success(), "fileio guest build failed");

    // mmap guest: allocates via SYS_mmap, writes the mapping with plain stores, munmaps.
    let src = format!("{}/asm/mmapguest.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/mmapguest");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang mmapguest");
    assert!(status.success(), "mmapguest build failed");

    // unaligned guest: an unaligned qword store faults MMU-off (Device memory), works
    // MMU-on with Normal memory; proves the stage-1 identity map is live with Normal attrs.
    let src = format!("{}/asm/unaligned.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/unaligned");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang unaligned");
    assert!(status.success(), "unaligned guest build failed");

    // pacguest: signs and authenticates a code pointer with pacia/autia; proves PAC engaged
    // (SCTLR_EL1.EnIA=1 + fixed APIA keys) and the sign/auth round-trip recovers the original.
    let src = format!("{}/asm/pacguest.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/pacguest");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang pacguest");
    assert!(status.success(), "pacguest build failed");

    // failsys: opens a path that does not exist; the failing open sets the carry flag and
    // returns errno (ENOENT=2) in x0, which the guest then exits with. Exercises the raw-svc
    // forwarder's error-ABI carry recording.
    let src = format!("{}/asm/failsys.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/failsys");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang failsys");
    assert!(status.success(), "failsys guest build failed");

    // remap: mmap A, store, munmap A, mmap B, store, load-back, exit x0=0 on match. Proves
    // honored munmap (debt #2) lets the guest go on to reuse address space afterward.
    let src = format!("{}/asm/remap.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/remap");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang remap");
    assert!(status.success(), "remap guest build failed");

    // mmapfile: opens a fixture, mmap()s it PROT_READ file-backed (no MAP_ANON), reads the first
    // byte, writes it to stdout. Proves Task 8's anon-staged file-backed mmap: record pread()s
    // the file into anon guest pages and stages the bytes as recorded writes; replay reproduces
    // them with zero file access (the fixture may be deleted).
    let fixture = format!("{out}/mmapfile_fixture.txt");
    std::fs::write(&fixture, b"MMAPFILE-OK\n").unwrap();
    let gen = format!("{out}/mmapfile_gen.s");
    std::fs::write(&gen, format!(".section __DATA,__data\n.p2align 3\n.global path\npath: .asciz \"{fixture}\"\n")).unwrap();
    let src = format!("{}/asm/mmapfile.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/mmapfile");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src,&gen])
        .status().expect("clang mmapfile");
    assert!(status.success(), "mmapfile guest build failed");

    // execmap: mmap()s a tiny FILE of code PROT_READ|PROT_EXEC (prot=5, MAP_PRIVATE) and blr's
    // into it, exiting with its return value (42). Proves runtime exec-mmap promotion: the VMM
    // installs RO+exec (ATTR_CODE) stage-1 pages for a PROT_EXEC mmap by editing the live page
    // tables, so the guest can execute mmap'd code under W^X. The fixture is the raw machine code
    // of `movz x0, #42 ; ret` (0xD2800540, 0xD65F03C0), little-endian.
    let fixture = format!("{out}/execmap_fixture.bin");
    std::fs::write(&fixture, [0x40u8, 0x05, 0x80, 0xD2, 0xC0, 0x03, 0x5F, 0xD6]).unwrap();
    let gen = format!("{out}/execmap_gen.s");
    std::fs::write(&gen, format!(".section __DATA,__data\n.p2align 3\n.global path\npath: .asciz \"{fixture}\"\n")).unwrap();
    let src = format!("{}/asm/execmap.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/execmap");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src,&gen])
        .status().expect("clang execmap");
    assert!(status.success(), "execmap guest build failed");

    // tlbiexec: the M9 capability fixture. mmaps an anon RW region, TOUCHES it (so the block is
    // translated), then MAP_FIXED-exec-maps a file of code over it and blr's in. Proves the guest-side
    // TLBI oracle: without it, place_fixed refused the exec-over-live-backing map and the recorder
    // aborted. Same code fixture as execmap: `movz x0, #42 ; ret`.
    let fixture = format!("{out}/tlbiexec_fixture.bin");
    std::fs::write(&fixture, [0x40u8, 0x05, 0x80, 0xD2, 0xC0, 0x03, 0x5F, 0xD6]).unwrap();
    let gen = format!("{out}/tlbiexec_gen.s");
    std::fs::write(&gen, format!(".section __DATA,__data\n.p2align 3\n.global path\npath: .asciz \"{fixture}\"\n")).unwrap();
    let src = format!("{}/asm/tlbiexec.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/tlbiexec");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src,&gen])
        .status().expect("clang tlbiexec");
    assert!(status.success(), "tlbiexec guest build failed");

    // machmsg: hand-builds a wire-format _kernelrpc_mach_vm_map (4811) MIG request and issues
    // mach_msg2 (svc -47); the box must service it on guest IPAs. Proves the M2-mach codec +
    // dispatch without dyld/libSystem in the loop.
    let src = format!("{}/asm/machmsg.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/machmsg");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang machmsg");
    assert!(status.success(), "machmsg guest build failed");

    // hello_dyn: a real dynamically-linked arm64 executable (normal toolchain, links libSystem).
    // Plain -arch arm64 (NOT arm64e — third-party arm64e builds are gated; the arm64e dyld loads a
    // plain-arm64 exe fine). Task 7 maps this + /usr/lib/dyld and builds dyld's process-start stack.
    let src = format!("{}/c/hello_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/hello_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang hello_dyn");
    assert!(status.success(), "hello_dyn guest build failed");

    // crashy: the M6 planted-bug dynamic guest — same recipe as hello_dyn (real toolchain, links
    // libSystem, plain -arch arm64). No -O, so the volatile off-by-one OOB store survives. Records
    // through real /usr/lib/dyld, then faults at 0x4000_DEAD_0000 => Stop::Fault (a recordable crash).
    let src = format!("{}/c/crashy.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/crashy");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang crashy");
    assert!(status.success(), "crashy guest build failed");

    // sigcatch_dyn: the M12 guest that catches SIGSEGV through APPLE's _sigtramp (libc's
    // sigaction() installs its own sa_tramp). Same recipe as crashy — real toolchain, links
    // libSystem, no -O so the volatile faulting store survives — and it faults at the same
    // 0x4000_DEAD_0000, a stage-1 fault that reaches Stop::Fault.
    let src = format!("{}/c/sigcatch_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/sigcatch_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang sigcatch_dyn");
    assert!(status.success(), "sigcatch_dyn guest build failed");

    // argv_echo: prints argv[1]. The M9 argv fixture — a real dynamic guest, same recipe as
    // hello_dyn (real toolchain, links libSystem, plain -arch arm64).
    let src = format!("{}/c/argv_echo.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/argv_echo");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang argv_echo");
    assert!(status.success(), "argv_echo guest build failed");

    // stdio_dyn: printf, whose flush reaches the kernel as write_nocancel (397). The M9 console
    // fixture — same recipe as hello_dyn (real toolchain, links libSystem, plain -arch arm64).
    let src = format!("{}/c/stdio_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/stdio_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang stdio_dyn");
    assert!(status.success(), "stdio_dyn guest build failed");

    // closefd_dyn: prints, then closes its own stdout — jq's exit shape. Same recipe as hello_dyn.
    let src = format!("{}/c/closefd_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/closefd_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang closefd_dyn");
    assert!(status.success(), "closefd_dyn guest build failed");

    // fdtable_dyn: the M10 fd-table semantics fixture — asserts the guest sees fd 3 rather than
    // retrace's 17, and EBADF after close. Same recipe as hello_dyn.
    let src = format!("{}/c/fdtable_dyn.c", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/fdtable_dyn");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-o",&bin,&src])
        .status().expect("clang fdtable_dyn");
    assert!(status.success(), "fdtable_dyn guest build failed");

    // strip47: signs a pointer with pacda then strips it with objc's 47-bit ISA_MASK; the result
    // equals the original ONLY if the PAC signature lands above bit 46 — i.e. only under a 47-bit
    // guest VA. The M2-va47 property test. -arch arm64e (Task 7, M7): with PAC posture now DERIVED
    // from the main executable's arch (Task 6), a plain-arm64 guest boots PAC-off and this
    // assertion goes vacuously true (pacda a no-op). This is the repo's first arm64e guest — it
    // never executes on the real host, only inside the VM, so the host's arm64e-runtime gating
    // does not apply; only the build needed to work (confirmed: `otool -hv` reports `ARM64 E`).
    // Making it genuinely arm64e is what exercises the PAC-ON branch of `pac_posture` end to end,
    // which would otherwise be dead code in every test the gate runs.
    let src = format!("{}/asm/strip47.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/strip47");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64e","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang strip47");
    assert!(status.success(), "strip47 guest build failed");

    // bfamstrip: pacdb-sign + corrupt + autdb -> FEAT_FPAC fault the box emulates by stripping.
    // The M2-bfam strip-on-FPAC property test. -arch arm64e (Task 7, M7): same reasoning as
    // strip47 above — with PAC posture derived from the main executable's arch, a plain-arm64
    // guest never FPAC-faults, so the strip-on-FPAC path never fires. This guest never executes
    // on the real host (VM-only), so arm64e-runtime gating doesn't apply; only the build needed
    // to work (confirmed: `otool -hv` reports `ARM64 E`).
    let src = format!("{}/asm/bfamstrip.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/bfamstrip");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64e","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang bfamstrip");
    assert!(status.success(), "bfamstrip guest build failed");

    // reservecommit: reserves a PROT_NONE region via _kernelrpc_mach_vm_map_trap (svc -15,
    // cur_protection=0), then first-touches two different pages inside it. Each touch faults (the
    // reservation is unbacked) and must be demand-committed by commit_reserved_page. The
    // M2-mmapcommit Task 1 micro-guest: proves reserve -> fault -> zero-fill commit -> store -> load.
    let src = format!("{}/asm/reservecommit.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/reservecommit");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang reservecommit");
    assert!(status.success(), "reservecommit guest build failed");

    // wildstore: stores to a wild unbacked, unreserved address (0xB_0000_0000). The fault must stay
    // fatal — the M2-mmapcommit fail-loud negative guest (commit_reserved_page must refuse it).
    let src = format!("{}/asm/wildstore.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/wildstore");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang wildstore");
    assert!(status.success(), "wildstore guest build failed");

    // carveout: reserves a PROT_NONE band (svc -15, cur_protection=0), punches an interior hole with
    // mach_vm_deallocate (svc -12), then commits ANYWHERE with hint = reservation base. The commit
    // must be forced into the carveout hole (base+0x10000), not honored at the raw hint — libmalloc's
    // guarded-metadata protocol in miniature. The M2-carveout Task 1 micro-guest.
    let src = format!("{}/asm/carveout.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/carveout");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang carveout");
    assert!(status.success(), "carveout guest build failed");

    // watchloop: 8 same-pc 8-byte stores to `target`, one strb to `target2` byte 0 (the BAS
    // negative), write(1, target, 8) — publishing target's address in the trace args — exit(0).
    // The M5 watchpoint guest: deterministic first-writer/last-writer ground truth.
    let src = format!("{}/asm/watchloop.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/watchloop");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang watchloop");
    assert!(status.success(), "watchloop guest build failed");

    // crash: stores to VA 0x4000_DEAD_0000 (bit 46 set), which no stage-1 table entry covers => a
    // stage-1 translation fault delivered via the EL1 trampoline => Stop::Fault (a recordable crash,
    // not a retrace bug). The M6 data-abort crash guest. Contrast wildstore.s (stage-2, stays fatal).
    let src = format!("{}/asm/crash.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/crash");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang crash");
    assert!(status.success(), "crash guest build failed");

    // crashjmp: branches to the same never-mapped VA => the instruction FETCH takes a stage-1
    // translation fault (EC 0x20, lower-EL instruction abort). The M6 instruction-abort crash guest.
    let src = format!("{}/asm/crashjmp.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/crashjmp");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang crashjmp");
    assert!(status.success(), "crashjmp guest build failed");

    // hello_rust: M7 rung 1 — a real Rust binary from the real toolchain, full std. rustc on a
    // single file takes no cargo lock, so there is no build recursion; RUSTC is the toolchain cargo
    // is already using (pinned 1.95.0), so the guest can't drift to a different compiler than the
    // workspace. Plain --target aarch64-apple-darwin (NOT arm64e, per the ladder's premise that
    // self-built binaries are arm64); links libSystem via /usr/lib/dyld like hello_dyn.
    let src = format!("{}/rs/hello_rust.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/hello_rust");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-o", &bin, &src])
        .status().expect("rustc hello_rust");
    assert!(status.success(), "hello_rust guest build failed");

    // usrstack: the M8-stack fixture. Issues sysctl(KERN_USRSTACK64), getrlimit(RLIMIT_STACK), and
    // an anonymous MAP_FIXED mmap, then publishes all four results as u64s on stdout. Plain
    // -arch arm64 freestanding, like the other micro-guests.
    let src = format!("{}/asm/usrstack.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/usrstack");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang usrstack");
    assert!(status.success(), "usrstack guest build failed");

    // fixedinner: the M8-stack straddle-cover fixture. mmaps two 4-page anon regions, fills them,
    // then punches a MAP_FIXED page into the middle of one and at the base of the other, and
    // publishes the returned addresses plus the surrounding bytes -- proving a partial FIXED
    // overwrite trims the backing instead of dropping it wholesale.
    let src = format!("{}/asm/fixedinner.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/fixedinner");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang fixedinner");
    assert!(status.success(), "fixedinner guest build failed");

    // wildfixed: the M8-stack fast-follow fixture. mmaps MAP_FIXED at an address outside the
    // guest's 36-bit IPA space (the one libstd's install_main_guard actually computes) and
    // publishes the carry + errno, proving the guest gets EINVAL back instead of the recorder
    // aborting inside hv_vm_map.
    let src = format!("{}/asm/wildfixed.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/wildfixed");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang wildfixed");
    assert!(status.success(), "wildfixed guest build failed");

    // panicky: M11's headline — a real full-std Rust binary whose panic reaches abort()/SIGABRT.
    // Same recipe as hello_rust (same RUSTC, same target) plus -C panic=abort, which is REQUIRED
    // and was measured: with the default panic=unwind this program exits 101 without ever raising a
    // signal, exercising nothing M11 added. With panic=abort it exits 134 (128 + SIGABRT).
    let src = format!("{}/rs/panicky.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/panicky");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-C", "panic=abort", "-o", &bin, &src])
        .status().expect("rustc panicky");
    assert!(status.success(), "panicky guest build failed");

    // segvy: M12's headline — a stock full-std Rust binary that faults on a wild pointer, so
    // libstd's OWN SIGSEGV handler runs, resets to SIG_DFL and returns, and the re-executed store
    // kills it. Same recipe as panicky MINUS -C panic=abort: a hardware fault is not a panic, so
    // no flag is needed to reach a signal here.
    let src = format!("{}/rs/segvy.rs", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/segvy");
    println!("cargo:rerun-if-changed={src}");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(rustc)
        .args(["--target", "aarch64-apple-darwin", "-o", &bin, &src])
        .status().expect("rustc segvy");
    assert!(status.success(), "segvy guest build failed");

    // M11 signal guests. raise: kill(getpid(), SIGABRT) — the terminal mechanism, and it exercises
    // the self-pid check as a side effect. sigign: the same raise with SIGABRT set to SIG_IGN first,
    // proving the guest keeps running (the branch the terminal gate cannot reach). killother:
    // kill(1, SIGKILL) — the safety boundary; the recorder must abort rather than signal launchd.
    // M12 delivery fixtures. Freestanding with their OWN trampolines, so they test retrace's entry
    // contract without libc's _sigtramp in the way. sigframe: validates every entry register, one
    // exit code per failed check. segvcatch: faults, and its handler advances __ss.__pc past the
    // store so sigreturn resuming MUTATED state is what lets it finish. altstack: SA_ONSTACK, and
    // the handler checks its own sp is inside the alt stack. vecsurvive: the handler clobbers v8,
    // so only a real vector restore exits 0. blockedfault: faults with SIGSEGV blocked — the
    // fail-loud fixture, which never exits cleanly by design.
    for name in ["raise", "sigign", "killother",
                 "sigframe", "segvcatch", "altstack", "vecsurvive", "blockedfault"] {
        let src = format!("{}/asm/{name}.s", env!("CARGO_MANIFEST_DIR"));
        let bin = format!("{out}/{name}");
        println!("cargo:rerun-if-changed={src}");
        let status = Command::new("clang")
            .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
            .status().expect("clang signal guest");
        assert!(status.success(), "{name} guest build failed");
    }
}
