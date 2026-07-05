fn main() {
    println!("cargo:rustc-link-lib=framework=Hypervisor");
    println!("cargo:rerun-if-changed=wrapper.h");
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_item("hv_.*|HV_.*");
    // libclang resolves `#include <Hypervisor/...>` from the macOS SDK's framework
    // search path, which the link flag above does not imply. Point it at the SDK.
    if let Ok(out) = std::process::Command::new("xcrun")
        .args(["--show-sdk-path"])
        .output()
    {
        if out.status.success() {
            let sdk = String::from_utf8(out.stdout).unwrap();
            builder = builder.clang_arg("-isysroot").clang_arg(sdk.trim().to_string());
        }
    }
    let bindings = builder.generate().expect("bindgen");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("bindings.rs")).unwrap();
}
