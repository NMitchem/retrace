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
}
