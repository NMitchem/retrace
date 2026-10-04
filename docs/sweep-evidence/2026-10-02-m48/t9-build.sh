#!/bin/bash
# M48 Task 9: every cargo build this task needs, FIRST, so no cargo runs while anything is
# recorded, benched or swept (M45 T3-a). The debug binary is the sweep's (the gates' build), the
# release one the walk's and the bench's. Each signed copy runs once untimed, because the first run
# of a freshly signed binary can stall for minutes in codesign validation.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node
L=$W/.superpowers/sdd/2026-10-02-retrace-m48-node
cd "$W" || exit 2
echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') commit=$(git rev-parse --short HEAD)"
cargo build -p retrace > $L/t9-build-debug.log 2>&1; echo "debug build exit=$?"
cargo build --release -p retrace > $L/t9-build-release.log 2>&1; echo "release build exit=$?"
cargo test -p retrace --no-run > $L/t9-build-tests.log 2>&1; echo "test build exit=$?"
for v in debug release; do
  b=/private/tmp/claude-501/m48-t9-$v-retrace
  cp target/aarch64-apple-darwin/$v/retrace $b && codesign -s - -f --entitlements retrace.entitlements $b; echo "$v sign=$?"
  perl -e 'alarm 900; exec @ARGV' $b > /dev/null 2>&1; echo "$v warm-up rc=$? (2 is the usage exit)"
  shasum -a 256 $b
done
clang -arch arm64 -bundle -undefined dynamic_lookup -I /opt/homebrew/include/node \
  -o /private/tmp/claude-501/m48-t9-crash_addon.node crates/retrace-guest/node/crash_addon.c; echo "addon=$?"
echo "end $(date '+%Y-%m-%d %H:%M:%S %Z')"
