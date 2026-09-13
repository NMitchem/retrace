#!/bin/sh
# Record and replay every binary in apple-sweep-binaries.txt.
#
# PASS means: both commands completed, their exit codes are equal, and their stdout
# is byte-identical. Exit codes are compared to each other, NOT to zero — /bin/false
# exits 1 on both runs and is a pass. A recorder panic is a FAIL even if the codes
# happen to match, so stderr is checked for it explicitly.
#
# M36: the human line says WHY a row failed, and every row that ran is followed by one
# machine-readable line. Labels, in evaluation order (spec §3a):
#   FAIL … (timed out after Ns recording)              unchanged
#   FAIL … (recorder panicked: <line>)                 <line> = rec.err's first `panicked at` line
#       joined with the line after it (Rust prints the panic message on its own line)
#   FAIL … (record error, rc=4: <line>)                new; <line> = rec.err's first `RECORD ERROR:`
#       line. The CLI exits 4 on RECORD ERROR and leaves a trace with no terminal event, so its
#       replay ALWAYS prints a DIVERGENCE line; replay is still run (that line is evidence, kept
#       on the ROW line) but the label is the record error, not "replay diverged".
#   FAIL … (timed out after Ns replaying)              unchanged
#   FAIL … (replay diverged at landmark N)             the landmark added
#   PASS … (identical fault, rc=N)                     new; rc = rp ≥ 128 (a signal death on both
#       sides) with equal stdout is still counted in `pass` (TALLY stays comparable with
#       M33–M35's) but it is said on the line
#   PASS …                                             rc = rp < 128, stdout equal (/bin/false's 1 is
#       the guest's own exit status, not a fault)
#   FAIL … (record=rc replay=rp)                       unchanged
#   ROW<TAB>path<TAB>result<TAB>rc<TAB>rp<TAB>recpid<TAB>landmark<TAB>rec_reason<TAB>rp_line
#       rp/landmark are `n/a` when there was no replay/divergence; recpid is the recorder's own
#       pid (see the record invocation); rec_reason/rp_line are the stderr lines the labels quote
#       (rec_reason: the `RECORD ERROR:` line, else the `panicked at` line joined with the message
#       line after it, cut to 300 chars), empty when none. TALLY is unchanged in shape.
#
# Usage: tools/apple-sweep.sh [path-to-retrace-binary]
#        defaults to target/aarch64-apple-darwin/debug/retrace
#   RETRACE_SWEEP_LIST=<file>   sweep this list instead of tools/apple-sweep-binaries.txt
#   RETRACE_SWEEP_KEEP=<dir>    copy every non-clean row's rec.err, rp.err and trace to
#                               <dir>/<basename>.{rec.err,rp.err,bin} (identical faults included)
#   RETRACE_SWEEP_TIMEOUT=<s>   per-phase watchdog, default 30
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RAW=${1:-$ROOT/target/aarch64-apple-darwin/debug/retrace}
LIST=$ROOT/tools/apple-sweep-binaries.txt
# M36: a caller may sweep a different list (a three-binary control, a single-row probe) and may
# keep every non-clean row's evidence. Both opt-in; the defaults are the committed corpus and
# nothing kept — the EXIT trap still destroys $TMP.
LIST=${RETRACE_SWEEP_LIST:-$LIST}
KEEP=${RETRACE_SWEEP_KEEP:-}
if [ -n "$KEEP" ]; then mkdir -p "$KEEP" || { echo "TALLY ABORTED (cannot create $KEEP)"; exit 2; }; fi

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

# M36: keep_row <result> [fault] — copy this row's evidence when asked, then emit the ROW line.
# Evidence is kept for every result that is not a clean PASS; the optional second argument marks
# an identical-fault PASS, whose evidence is kept too (its crash is the thing to read). Reads the
# loop's own variables (g rc rp recpid landmark rec_reason rp_line) at call time; it is called
# before the loop's next `rm -f`, so what it copies is this row's own, never a neighbour's. A cp
# failure is left audible on stderr — silently missing evidence is the failure this milestone
# exists to close.
keep_row() {
    if [ -n "$KEEP" ] && { [ "$1" != "PASS" ] || [ -n "${2:-}" ]; }; then
        b=$(basename "$g")
        cp "$TMP/rec.err" "$KEEP/$b.rec.err"
        [ -f "$TMP/rp.err" ] && cp "$TMP/rp.err" "$KEEP/$b.rp.err"
        [ -f "$TMP/t.bin" ] && cp "$TMP/t.bin" "$KEEP/$b.bin"
    fi
    printf 'ROW\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$g" "$1" "$rc" "$rp" "$recpid" "$landmark" "$rec_reason" "$rp_line"
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
    # M36: rp.out/rp.err too — a row whose recorder timed out or panicked never runs replay, so
    # without this the rp.err keep_row copies as that row's evidence would be the previous row's.
    rm -f "$TMP/t.bin" "$TMP/rp.out" "$TMP/rp.err"
    # M36: the recorder's pid decides what several Apple binaries do (M34 §4b: a pid inside
    # [0x4000, 0x10000) is forwarded as a host pointer by forward_and_diff's per-register probe,
    # so every self-pid csops/proc_info answers ESRCH; M35 measured dddiagnose taking a different
    # wall on each side of that line). Print it into rec.err before the recorder prints anything,
    # from the shell that becomes the recorder — M34's probe shape. run_timeout launches "$@"
    # verbatim, so the `sh` it backgrounds is the process that `exec`s into the recorder: the pid
    # it printed IS the recorder's, and the watchdog's kill still lands on the recorder.
    run_timeout sh -c 'echo "recpid=$$" >&2; exec "$0" record-dyn "$1" -o "$2"' "$BIN" "$g" "$TMP/t.bin" >"$TMP/rec.out" 2>"$TMP/rec.err" </dev/null; rc=$?
    recpid=$(grep -a '^recpid=' "$TMP/rec.err" | head -1 | cut -d= -f2)
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
    # M30: the same hole, one milestone later, in this same file. M30's Phase A measurement was a
    # grep of this script's output for "[M30 CANARY]", and its Phase B flipped a report-only counter
    # to a hard assert because that grep read zero — so unsurfaced, it would have read zero for every
    # possible guest behaviour and decided the flip on plumbing rather than on the kernel. "[M28 BANDSHRINK]" is surfaced beside it because it is that measurement's
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
    # M36: derive what the old ladder threw away, before deciding anything. A RECORD ERROR is one
    # line; a panic is two — Rust prints `thread 'main' … panicked at <file>:<line>:` and then the
    # message on the next line, and the message (which assert, which syscall) is the part worth
    # reading — so the panic case joins the pair with a space. Tried in that order because a
    # recorder that hits a RECORD ERROR does not also panic, and vice versa.
    rec_reason=$(grep -a -m1 'RECORD ERROR:' "$TMP/rec.err" | cut -c1-200)
    if [ -z "$rec_reason" ]; then
        rec_reason=$(grep -a -m1 -A1 'panicked at' "$TMP/rec.err" | tr '\n' ' ' | sed 's/ *$//' | cut -c1-300)
    fi
    rp_line=""; landmark="n/a"; rp="n/a"
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $g (timed out after ${TIMEOUT_SECS}s recording)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if grep -qa "panicked at" "$TMP/rec.err"; then
        echo "FAIL $g (recorder panicked: $rec_reason)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    run_timeout "$BIN" replay "$TMP/t.bin" >"$TMP/rp.out" 2>"$TMP/rp.err" </dev/null; rp=$?
    rp_line=$(grep -a -m1 'DIVERGENCE at landmark' "$TMP/rp.err" | cut -c1-200)
    case "$rp_line" in
        'DIVERGENCE at landmark '*) landmark=$(printf '%s' "$rp_line" | sed 's/^DIVERGENCE at landmark \([0-9]*\).*/\1/') ;;
    esac
    # M36: a recorder that exited 4 printed `RECORD ERROR:` and wrote a trace with no terminal
    # event, so its replay ALWAYS prints a DIVERGENCE line (it runs out of events, or reports the
    # exception the recorder could not record). That line is evidence, not the label: M35 measured
    # dddiagnose's "replay diverged" this way, and M36's first reading found all five of the
    # long-standing "replay diverged" rows are the same recorder-side brk. Label the record error.
    # Checked before the replay-timeout marker (spec §3a's evaluation order): the record error is
    # the cause, whatever the replay of its truncated trace then did; the ROW line still carries rp.
    if [ "$rc" -eq 4 ]; then
        echo "FAIL $g (record error, rc=4: $rec_reason)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $g (timed out after ${TIMEOUT_SECS}s replaying)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    # Divergence detection is structural, not inferred from exit codes alone: retrace
    # always prints this on a divergence, so check for it directly rather than relying
    # on the exit-code compare below to happen to disagree.
    if [ -n "$rp_line" ]; then
        echo "FAIL $g (replay diverged at landmark $landmark)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if [ "$rc" -eq "$rp" ] && cmp -s "$TMP/rec.out" "$TMP/rp.out"; then
        if [ "$rc" -ge 128 ]; then
            # M36: an identical FAULT on both sides is still counted a pass (the tally stays
            # comparable with M33–M35's), but it is said on the line: M35 found dddiagnose's
            # rc=139 passes are retrace-induced crashes (M34 §4b), not the guest's own. "Fault"
            # means signal death — the CLI exits 128+signo when the guest dies of one (139 =
            # SIGSEGV) — so the threshold is 128: a designed non-zero exit (/bin/false's 1, a
            # usage error's 64) is the guest's own status and stays a bare PASS, as before M36.
            echo "PASS $g (identical fault, rc=$rc)"; pass=$((pass+1)); keep_row PASS fault; continue
        fi
        echo "PASS $g"; pass=$((pass+1)); keep_row PASS; continue
    fi
    echo "FAIL $g (record=$rc replay=$rp)"; fail=$((fail+1)); keep_row FAIL
done 3< "$LIST"

echo "TALLY pass=$pass fail=$fail skip=$skip"
