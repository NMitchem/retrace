# One-command M0 gate.
m0:
    cargo test --workspace -- --test-threads=1
    cargo clippy --workspace --all-targets -- -D warnings
