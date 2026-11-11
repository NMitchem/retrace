# Every test, then clippy with warnings as errors. Tests run one at a time: Hypervisor.framework
# allows one VM per process.
gate:
    cargo test --workspace -- --test-threads=1
    cargo clippy --workspace --all-targets -- -D warnings
