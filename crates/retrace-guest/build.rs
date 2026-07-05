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
}
