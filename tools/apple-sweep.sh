#!/bin/sh
# Record and replay every binary in apple-sweep-binaries.txt.
#
# PASS means: both commands completed, their exit codes are equal, and their stdout
# is byte-identical. Exit codes are compared to each other, NOT to zero — /bin/false
# exits 1 on both runs and is a pass. A recorder panic is a FAIL even if the codes
# happen to match, so stderr is checked for it explicitly.
#
# Usage: tools/apple-sweep.sh [path-to-retrace-binary]
#        defaults to target/aarch64-apple-darwin/debug/retrace
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RAW=${1:-$ROOT/target/aarch64-apple-darwin/debug/retrace}
LIST=$ROOT/tools/apple-sweep-binaries.txt

[ -x "$RAW" ] || { echo "no retrace binary at $RAW (cargo build -p retrace first)" >&2; exit 2; }

# Every hv_* caller needs the hypervisor entitlement, and a raw cargo output binary
# does not have it — .cargo/config.toml's runner only signs what cargo itself invokes.
# Sign a copy rather than the original: two concurrent users must never write one path.
BIN=$RAW-sweep-$$
cp "$RAW" "$BIN"
codesign -s - -f --entitlements "$ROOT/retrace.entitlements" "$BIN" >/dev/null 2>&1 \
  || { echo "codesign failed for $BIN" >&2; exit 2; }
TMP=$(mktemp -d -t retrace-sweep)
trap 'rm -rf "$TMP" "$BIN"' EXIT INT TERM

# macOS ships no timeout(1)/gtimeout(1), so a bounded run is hand-rolled: launch "$@"
# (inheriting whatever redirections the caller already applied), race a watchdog
# `sleep` against it, and SIGKILL the child if the watchdog wins. Needed because some
# guests never exit on their own — /usr/bin/yes with no arguments floods stdout
# forever and, measured, ran retrace's memory up past 4.6 GiB before being killed by
# hand. Without this, one such binary hangs the whole sweep rather than failing it.
#
# A timeout-kill leaves a "$TMP/.timedout" marker (only when the kill actually landed
# on a still-live process, not a race against one that had already exited). This
# matters because two independent kills are NOT the same outcome as two independent
# completions: /usr/bin/yes, measured, gets SIGKILLed on both the record and replay
# side before producing any captured stdout, so both phases exit 137 with byte-
# identical (empty) output — a match that would otherwise read as a PASS despite the
# guest never finishing. That is exactly the class of false equivalence the repo's own
# gate discipline warns about (an exit code a weaker failure would also produce), so
# each call site below checks the marker and fails loud instead of trusting the compare.
TIMEOUT_SECS=${RETRACE_SWEEP_TIMEOUT:-30}
run_timeout() {
    rm -f "$TMP/.timedout"
    "$@" &
    cpid=$!
    ( sleep "$TIMEOUT_SECS"; kill -9 "$cpid" 2>/dev/null && touch "$TMP/.timedout" ) &
    wpid=$!
    wait "$cpid" 2>/dev/null
    rc=$?
    kill "$wpid" 2>/dev/null
    wait "$wpid" 2>/dev/null
    return $rc
}

pass=0; fail=0; skip=0
# The list is read on fd 3, not fd 0: several sweep guests (bash, csh, dash, ksh, sh,
# tcsh, zsh) run with no arguments and, non-interactively, read a *script* from stdin.
# If the loop's `read` shared fd 0 with those children, a shell guest would consume
# the rest of this very file as its own script — measured happening (M29: /bin/bash
# swallowed the next line, /bin/cat vanished entirely, and the sweep quietly stopped
# after 6 of 59 lines with a TALLY that looked complete). Explicitly pointing every
# child's stdin at /dev/null closes both holes: the fd-3 list is never visible to any
# child, and no guest blocks waiting on a real terminal.
while IFS= read -r g <&3; do
    case "$g" in ''|\#*) continue ;; esac
    if [ ! -x "$g" ]; then echo "SKIP $g (not present)"; skip=$((skip+1)); continue; fi

    run_timeout "$BIN" record-dyn "$g" -o "$TMP/t.bin" >"$TMP/rec.out" 2>"$TMP/rec.err" </dev/null; rc=$?
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $g (timed out after ${TIMEOUT_SECS}s recording)"; fail=$((fail+1)); continue
    fi
    if grep -qa "panicked at" "$TMP/rec.err"; then
        echo "FAIL $g (recorder panicked)"; fail=$((fail+1)); continue
    fi
    run_timeout "$BIN" replay "$TMP/t.bin" >"$TMP/rp.out" 2>"$TMP/rp.err" </dev/null; rp=$?
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $g (timed out after ${TIMEOUT_SECS}s replaying)"; fail=$((fail+1)); continue
    fi

    if [ "$rc" -eq "$rp" ] && cmp -s "$TMP/rec.out" "$TMP/rp.out"; then
        echo "PASS $g"; pass=$((pass+1))
    else
        echo "FAIL $g (record=$rc replay=$rp)"; fail=$((fail+1))
    fi
done 3< "$LIST"

echo "TALLY pass=$pass fail=$fail skip=$skip"
