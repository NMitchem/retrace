# Pinned regression seeds (never remove)

Each row is a seed that once made `record -> (fault) -> replay` end in something other than
exit 0 (byte-identical `hello\n`) or exit 3 (named divergence) — a panic, a wrong answer, or
some other exit code. Once fixed, the seed stays pinned in `tests/seeded_swarm.rs` forever so
the same universe can never regress.

| seed | fault | symptom | fixed in |
|------|-------|---------|----------|
| _(none yet — 200/200 seeds hold on the M0 gate)_ | | | |
