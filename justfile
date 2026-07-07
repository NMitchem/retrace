# One-command gate: full workspace test suite + clippy, warnings-as-errors.
# Same recipe serves every milestone's exit gate (M0, M1, ...) — the workspace
# just grows more crates/tests under it; keep `m0` as an alias so old references stay valid.
gate:
    cargo test --workspace -- --test-threads=1
    cargo clippy --workspace --all-targets -- -D warnings

m0: gate
m1: gate
