# Contributing

## Building from source

You need a Rust toolchain (`rust-toolchain.toml` pins the version):

```sh
git clone https://github.com/NMitchem/retrace && cd retrace
cargo build --release
codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/release/retrace
```

Anything that uses Hypervisor.framework needs the `com.apple.security.hypervisor` entitlement, which
an ad-hoc signature can carry. Re-sign after every build. For development, `cargo run -p retrace -- ...`
signs for you through the runner in `.cargo/config.toml`. The first run of a freshly signed binary can
pause for a while in macOS's code-signing check; it isn't hung. The examples call the binary `retrace`,
so put its directory on your `PATH` or use the full path.

## Tests

There's no CI: the tests need macOS 26 or 27 on Apple Silicon and a working `hv_vm_create`, which hosted
macOS runners (themselves VMs) don't provide. Run `just gate` locally and paste the counts in your pull
request. [`CLAUDE.md`](CLAUDE.md#commands) explains why `--test-threads=1` is required and how to run the
suite in pieces.

When a test reaches something retrace can't do yet, mark it `#[ignore = "..."]` with the reason rather than
loosening its assertion. Assert on the difference your change makes, and make a skipped test say it skipped.

[`CLAUDE.md`](CLAUDE.md) also covers the architecture and the platform rules the code depends on.
