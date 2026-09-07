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

# A pre-loop setup failure below exits before any per-binary line or the closing TALLY
# is ever printed. A consumer that only greps its captured output for "TALLY" would see
# nothing and could mistake that silence for an empty-but-successful sweep, so each exit
# here also emits a line starting with "TALLY" that a `pass=`/`fail=`/`skip=` parser
# cannot mistake for the real thing.
[ -x "$RAW" ] || {
    echo "no retrace binary at $RAW (cargo build -p retrace first)" >&2
    echo "TALLY ABORTED (no retrace binary at $RAW)"
    exit 2
}

# Every hv_* caller needs the hypervisor entitlement, and a raw cargo output binary
# does not have it — .cargo/config.toml's runner only signs what cargo itself invokes.
# Sign a copy rather than the original: two concurrent users must never write one path.
BIN=$RAW-sweep-$$
cp "$RAW" "$BIN"
codesign -s - -f --entitlements "$ROOT/retrace.entitlements" "$BIN" >/dev/null 2>&1 || {
    echo "codesign failed for $BIN" >&2
    echo "TALLY ABORTED (codesign failed)"
    exit 2
}
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
# CRITICAL (fix round 1): this function's scratch variable for the child's exit status
# must NOT be named `rc` or `rp` — those are the outer loop's own names for the record
# and replay phases' exit codes. sh has no function-local scoping by default, so a
# same-named scratch here silently clobbers whichever of the two the caller had already
# captured: the replay call's internal assignment overwrote the record phase's `rc`
# before `[ "$rc" -eq "$rp" ]` ever ran, making that comparison compare replay's own exit
# status to itself — always true, for every binary, never able to fail. Measured via
# execution: automationmodetool/desdp/dyld_info/flex each genuinely diverge (record
# exits 4, replay independently exits 3), and the pre-fix script called all four PASS.
# `_tmo_status` is scoped by name alone (a fresh, otherwise-unused identifier) rather
# than by `local`, which POSIX sh does not guarantee.
TIMEOUT_SECS=${RETRACE_SWEEP_TIMEOUT:-30}
run_timeout() {
    rm -f "$TMP/.timedout"
    "$@" &
    cpid=$!
    ( sleep "$TIMEOUT_SECS"; kill -9 "$cpid" 2>/dev/null && touch "$TMP/.timedout" ) &
    wpid=$!
    wait "$cpid" 2>/dev/null
    _tmo_status=$?
    kill "$wpid" 2>/dev/null
    wait "$wpid" 2>/dev/null
    return "$_tmo_status"
}

pass=0; fail=0; skip=0
# The list is read on fd 3, not fd 0: several sweep guests (bash, csh, dash, ksh, sh,
# tcsh, zsh) run with no arguments and, non-interactively, read a *script* from stdin.
# If the loop's `read` shared fd 0 with those children, a shell guest would consume
# the rest of this very file as its own script — measured happening (M29: /bin/bash
# swallowed the next line, /bin/cat vanished entirely, and the sweep quietly stopped
# after 6 of 59 lines with a TALLY that looked complete). Explicitly pointing every
# child's stdin at /dev/null is what closes the hole: a shell guest reads its script from
# /dev/null and gets immediate EOF, and no guest blocks waiting on a real terminal.
# NOT because fd 3 is hidden from children -- it is not. Descriptors above 2 are inherited
# across exec unless marked close-on-exec, so fd 3 IS present in the child; what makes that
# harmless is that a guest would have to go looking for it, and `Box_::translate_fds`
# returns EBADF for any fd the guest never opened through retrace's own fd table. Stating
# it the old way ("never visible to any child") would leave the next person believing
# inheritance was prevented, and reaching for fd 3 elsewhere on that belief.
while IFS= read -r g <&3; do
    case "$g" in ''|\#*) continue ;; esac
    if [ ! -x "$g" ]; then echo "SKIP $g (not present)"; skip=$((skip+1)); continue; fi

    # A previous iteration's trace must not survive to this one: if record fails via a
    # clean non-panic error path (exit(4) "RECORD ERROR") rather than a panic, it may
    # leave no fresh t.bin (or an old one may still be sitting in $TMP), and replay would
    # then silently replay the PREVIOUS binary's recording — manufacturing both false
    # passes and false failures, exactly what this script exists to prevent.
    rm -f "$TMP/t.bin"
    run_timeout "$BIN" record-dyn "$g" -o "$TMP/t.bin" >"$TMP/rec.out" 2>"$TMP/rec.err" </dev/null; rc=$?
    # M29 fix round 1 (Critical): the recorder's stderr goes to $TMP/rec.err, grepped only
    # for "panicked at" below and destroyed by this script's own EXIT trap — so a caller
    # capturing only this script's OWN stdout/stderr (as the M29 measurement did) never sees
    # a `[M29 DEREFLEN*]` line no matter how many times the guest dispatched that arm.
    # Surface them here, before rec.err is ever at risk of going away, tagged with the guest
    # path the way every PASS/FAIL/SKIP line already is. Runs regardless of what the record
    # phase does next (panic/timeout/success) so a diagnostic emitted just before a crash is
    # not lost either. `grep -qa` first avoids an unconditional (and here pointless) `sed` on
    # every binary when the var is unset or the guest never dispatched the arm.
    if [ -n "${RETRACE_DEREFLEN:-}" ] && grep -qa "\[M29 DEREFLEN" "$TMP/rec.err"; then
        grep -a "\[M29 DEREFLEN" "$TMP/rec.err" | sed "s#^#$g: #"
    fi
    # M30 Task 5: the same hole, one milestone later, in this same file. This task's ENTIRE
    # deliverable is a grep of this script's output for "[M30 CANARY]", and Phase B flips a
    # report-only counter to a hard assert only if that grep reads zero — so unsurfaced, it would
    # read zero for every possible guest behaviour and decide the next task on plumbing rather than
    # on the kernel. "[M28 BANDSHRINK]" is surfaced beside it because it is that measurement's
    # positive control ON THIS PATH: it leaves `forward_and_diff` by the same `eprintln!`, on the
    # same stream, in the same process as the canary line, and is already measured non-zero (29-31
    # lines on one /bin/ps recording), so a sweep run under RETRACE_BANDSHRINK that surfaces those
    # lines is what makes a zero canary count from the sweep mean something about the guests. A
    # control taken on a DIFFERENT path (an in-process `cargo test`, whose stderr is never
    # redirected) would prove nothing about this one.
    if [ -n "${RETRACE_CANARY:-}" ] && grep -qa "\[M30 CANARY\]" "$TMP/rec.err"; then
        grep -a "\[M30 CANARY\]" "$TMP/rec.err" | sed "s#^#$g: #"
    fi
    if [ -n "${RETRACE_BANDSHRINK:-}" ] && grep -qa "\[M28 BANDSHRINK\]" "$TMP/rec.err"; then
        grep -a "\[M28 BANDSHRINK\]" "$TMP/rec.err" | sed "s#^#$g: #"
    fi
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
    # Divergence detection is structural, not inferred from exit codes alone: retrace
    # always prints this on a divergence, so check for it directly rather than relying
    # on the exit-code compare below to happen to disagree.
    if grep -qa "DIVERGENCE at landmark" "$TMP/rp.err"; then
        echo "FAIL $g (replay diverged)"; fail=$((fail+1)); continue
    fi

    if [ "$rc" -eq "$rp" ] && cmp -s "$TMP/rec.out" "$TMP/rp.out"; then
        echo "PASS $g"; pass=$((pass+1))
    else
        echo "FAIL $g (record=$rc replay=$rp)"; fail=$((fail+1))
    fi
done 3< "$LIST"

echo "TALLY pass=$pass fail=$fail skip=$skip"
