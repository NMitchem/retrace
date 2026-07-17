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

    // strip47: signs a pointer with pacda then strips it with objc's 47-bit ISA_MASK; the result
    // equals the original ONLY if the PAC signature lands above bit 46 — i.e. only under a 47-bit
    // guest VA. The M2-va47 property test.
    let src = format!("{}/asm/strip47.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/strip47");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang strip47");
    assert!(status.success(), "strip47 guest build failed");

    // bfamstrip: pacdb-sign + corrupt + autdb -> FEAT_FPAC fault the box emulates by stripping.
    // The M2-bfam strip-on-FPAC property test.
    let src = format!("{}/asm/bfamstrip.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/bfamstrip");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
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
}
