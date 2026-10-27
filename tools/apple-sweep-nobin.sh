#!/bin/sh
# Record and replay every binary in apple-sweep-binaries.txt.
#
# PASS means: both commands completed, their exit codes are equal, and their stdout
# is byte-identical. Exit codes are compared to each other, NOT to zero — /bin/false
# exits 1 on both runs and is a pass. A retrace bug, on record or replay, is a FAIL even if the codes
# happen to match, so both stderr files are checked for it explicitly.
#
# M36: the human line says WHY a row failed, and every row that ran is followed by one
# machine-readable line. Labels, in evaluation order (spec §3a):
#   FAIL … (timed out after Ns recording)              unchanged
#   FAIL … (retrace bug on record: <line>)            M50: rec.err has retrace's bug banner, or a Rust
#       panic located in retrace's own crates (`panicked at crates/(retrace|hv-sys)`); <line> is the
#       banner's message line, or the panic line joined with the message after it. A Rust guest's own
#       panic prints its own path and never matches.
#   FAIL … (wall, rc=N: <line>)                      M50: an in-scope row stopped at a wall; <line> is
#       rec.err's `retrace: unsupported:` line, and the line is the test, never rc (a guest may exit 4).
#   REFUSED … (<line>)                               M50: an out-of-scope row (see the list format)
#       refused cleanly: exactly one wall line, exit 4, and a replay that ends at the
#       incomplete-recording line with exit 4 and the recording's stdout. Counted in `refused`.
#   FAIL … (retrace bug on replay: <line>)           M50: the bug check on rp.err, right after replay.
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
#       (rec_reason: the bug line if there is one, else the wall line, else empty; the result field is
#       PASS, FAIL or REFUSED). TALLY gains `refused=` after `pass=`.
#
# Usage: tools/apple-sweep.sh [path-to-retrace-binary]
#        defaults to target/aarch64-apple-darwin/debug/retrace
#   RETRACE_SWEEP_LIST=<file>   sweep this list instead of tools/apple-sweep-binaries.txt
#   RETRACE_SWEEP_KEEP=<dir>    copy every non-clean row's rec.err, rp.err and trace to
#                               <dir>/<basename>.{rec.err,rp.err,bin} (identical faults included)
#   RETRACE_SWEEP_KEEP_ALL=1   with KEEP: keep PASS rows too (the M37 audit's full-corpus baseline)
#   RETRACE_SWEEP_TIMEOUT=<s>   per-phase watchdog, default 30
#   RETRACE_SWEEP_NATIVE=0      M49, the default since M51 (spec R9): each row also runs natively, outside
#                               retrace, first, and passes only if the recording exits as native did and
#                               prints what native printed; one whose record and replay agree but differ
#                               from native prints `FAIL … (differs from native: record=N native=…)`.
#                               `=0` turns it off. With it on, every ROW line gains a tenth field, native:
#                               `<native rc>:same` or `<native rc>:differs` (the recording's stdout against
#                               native's), or `timeout:n/a`; and an eleventh, stderr: `same`, `differs` or
#                               `n/a` (native timed out), the recording's stderr against native's after
#                               `mask_err`.
#   RETRACE_SWEEP_STDERR=1      M51 (spec §3j): a row whose stderr differs from native's fails,
#                               `FAIL … (stderr differs from native)`. Tier 2 sets it; the Apple sweep's
#                               rows were never curated for stderr, so it only logs the column.
#   RETRACE_SWEEP_ENV=<names>   variables a row may see beside SWEEP_ENV's, space-separated (a census
#                               list holding `fc_env` needs `FC_ENV_PROBE`). See clean_run.
#
# Since the M53 scrub every run of a row, native, record and replay, starts under `env -i` with only
# SWEEP_ENV's variables and RETRACE_SWEEP_ENV's, at the caller's values; retrace's own two runs also
# get the caller's RETRACE_*, which the recorder strips from the guest's environment. Nothing else of
# the caller's environment reaches a row.
#
# M49 (spec §3c): a list line may carry arguments after the path, each preceded by a TAB. They are
# passed verbatim: the line is split on TAB only, with globbing off, and never evaluated by a shell,
# so an argument may hold spaces, quotes or `$` but not a TAB, and an empty argument cannot be
# written. A row with arguments is named `<path> <args…>` on its PASS/FAIL and ROW lines, and its
# kept evidence carries its list line number. Every row runs from the repo root, so a relative path
# in an argument names a repo file. A line with no TAB is a row with no arguments, which is every
# line of apple-sweep-binaries.txt, and its record command is the one this script always ran.
# M50 (spec §3g): a list line whose path starts `out:` is a row out of scope (charter §3). It passes by
# matching native, like any row, or by refusing cleanly (REFUSED above); anything else fails it. The
# mark is not part of the row's name, so its PASS/FAIL/ROW lines stay comparable with older sweeps.
# M51 (spec §3j, R9): `TALLY-RR` follows `TALLY`: the same rows judged record against replay alone, as
# before M51, so the charter's record-against-replay figure stays checkable. With native on, an
# out-of-scope row is REFUSED only if what it printed before the wall, stdout and stderr, is a prefix of
# what native printed (deferred #8). A row that fails only on stderr (RETRACE_SWEEP_STDERR=1) still counts as
# a pass in TALLY-RR, which is intended; a wall row that fails only the prefix rule stays a failure there.
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RAW=${1:-$ROOT/target/aarch64-apple-darwin/debug/retrace}
LIST=$ROOT/tools/apple-sweep-binaries.txt
# M36: a caller may sweep a different list (a three-binary control, a single-row probe) and may
# keep every non-clean row's evidence. Both opt-in; the defaults are the committed corpus and
# nothing kept — the EXIT trap still destroys $TMP.
LIST=${RETRACE_SWEEP_LIST:-$LIST}
KEEP=${RETRACE_SWEEP_KEEP:-}
KEEP_ALL=${RETRACE_SWEEP_KEEP_ALL:-}
NATIVE=${RETRACE_SWEEP_NATIVE:-1}
[ "$NATIVE" = 0 ] && NATIVE=
STDERR=${RETRACE_SWEEP_STDERR:-}
if [ -n "$KEEP" ]; then mkdir -p "$KEEP" || { echo "TALLY ABORTED (cannot create $KEEP)"; exit 2; }; fi
# M49: the loop `cd`s to the repo root before every row, so each path it uses after that is made
# absolute here, against the caller's directory. ROOT and TMP are absolute already, and so is every
# default.
case "$LIST" in /*) ;; *) LIST=$PWD/$LIST ;; esac
case "$KEEP" in ''|/*) ;; *) KEEP=$PWD/$KEEP ;; esac

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
case "$BIN" in /*) ;; *) BIN=$PWD/$BIN ;; esac
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

# The M53 scrub: a row's environment. Before it every run inherited the caller's, so the `env` and
# `printenv` rows' kept evidence carried whatever token or key the caller's environment held,
# verbatim, and native and the guest saw different variables (the harness's own `_` and `OLDPWD`,
# Ruling T8-env, and a different order). The names a row may see are listed, never the ones it may
# not: a list of secrets' names misses the next one. `__CF_USER_TEXT_ENCODING` is left out on
# purpose: CoreFoundation rewrites a value it does not expect (Claude Code's `0x0:0:0`) to
# `0x<uid>:0x0:0x0` inside the recorder before the guest's environment is built, so the guest saw
# the rewritten value and native the original (measured; unset, neither sees one).
SWEEP_ENV='PATH HOME USER LOGNAME SHELL TMPDIR TERM LANG LC_ALL LC_CTYPE TZ'

# clean_run native|retrace|recorder CMD… — exec CMD under `env -i` with each variable of SWEEP_ENV and
# RETRACE_SWEEP_ENV that the caller has set, in that order; retrace's runs add the caller's RETRACE_*
# after them, and `recorder` prints `recpid=` first (see the record invocation). `env -i` is the last
# exec before CMD, so no shell between them adds a variable of its own. It execs, so it is only ever
# run_timeout's backgrounded command, whose pid is then CMD's own and the one the watchdog kills.
clean_run() {
    _mode=$1; shift; _ncmd=$#
    _names="$SWEEP_ENV ${RETRACE_SWEEP_ENV:-}"
    [ "$_mode" = native ] || _names="$_names $(env | sed -n 's/^\(RETRACE_[A-Za-z0-9_]*\)=.*/\1/p')"
    for _v in $_names; do
        eval "[ -z \"\${$_v+x}\" ] || set -- \"\$@\" \"$_v=\${$_v}\""
    done
    # The assignments went on after CMD; rotate CMD's words behind them.
    while [ "$_ncmd" -gt 0 ]; do set -- "$@" "$1"; shift; _ncmd=$((_ncmd-1)); done
    [ "$_mode" = recorder ] && exec sh -c 'echo "recpid=$$" >&2; exec env -i "$@"' sh "$@"
    exec env -i "$@"
}

# M36: keep_row <result> [fault] — copy this row's evidence when asked, then emit the ROW line.
# Evidence is kept for every result that is not a clean PASS; the optional second argument marks
# an identical-fault PASS, whose evidence is kept too (its crash is the thing to read). Reads the
# loop's own variables (g rc rp recpid landmark rec_reason rp_line) at call time; it is called
# before the loop's next `rm -f`, so what it copies is this row's own, never a neighbour's. A cp
# failure is left audible on stderr — silently missing evidence is the failure this milestone
# exists to close.
# M37: RETRACE_SWEEP_KEEP_ALL=1 (with KEEP) keeps clean PASS rows too — the full-corpus baseline
# a later fix is diffed against needs every trace, not just the ones that were already interesting.
keep_row() {
    if [ -n "$KEEP" ] && { [ -n "$KEEP_ALL" ] || [ "$1" != "PASS" ] || [ -n "${2:-}" ]; }; then
        b=$(basename "$g")
        # M49: two rows may run one path with different arguments; their evidence must not collide.
        [ "$nargs" -gt 0 ] && b="$b.$lno"
        cp "$TMP/rec.err" "$KEEP/$b.rec.err"
        [ -f "$TMP/rp.err" ] && cp "$TMP/rp.err" "$KEEP/$b.rp.err"
        [ -e "$TMP/rp.out" ] && cp "$TMP/rp.out" "$KEEP/$b.rp.out"
        [ -n "$NATIVE" ] && [ -e "$TMP/nat.out" ] && cp "$TMP/nat.out" "$KEEP/$b.nat.out"
        [ -n "$NATIVE" ] && [ -e "$TMP/nat.err" ] && cp "$TMP/nat.err" "$KEEP/$b.nat.err"
    fi
    if [ -n "$NATIVE" ]; then
        printf 'ROW\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$row" "$1" "$rc" "$rp" "$recpid" "$landmark" "$rec_reason" "$rp_line" "$native" "$errcmp"
    else
        printf 'ROW\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$row" "$1" "$rc" "$rp" "$recpid" "$landmark" "$rec_reason" "$rp_line"
    fi
}

# M50 §3g: retrace's own bug in a stderr file, as the line to quote, or nothing. Since M50 the CLI
# prints a banner whose third line is the message; a panic before the hook is installed, or a build
# without it, still prints Rust's `panicked at <file>` with the message on the next line. Anchored on
# retrace's own crates, because a guest's own stderr can carry a Rust panic of its own (fc_rs_tests_fail
# prints one natively), and a tool built from a `crates/` workspace would print `panicked at crates/`.
bug_reason() {
    [ -f "$1" ] || return 0
    if grep -qa '^retrace: internal error' "$1"; then
        grep -a -m1 -A2 '^retrace: internal error' "$1" | tail -1 | sed 's/^ *//' | cut -c1-300
    elif grep -qaE 'panicked at crates/(retrace|hv-sys)' "$1"; then
        grep -a -m1 -A1 -E 'panicked at crates/(retrace|hv-sys)' "$1" | tr '\n' ' ' | sed 's/ *$//' | cut -c1-300
    fi
}

# M51 §3j: a stderr file as compared with native's, on stdout: retrace's own lines (`retrace: …`, and this
# script's `recpid=`) dropped, then t0 M7's pid masks, the same as firstcontact_e2e's `mask_pid`.
mask_err() {
    grep -av -e '^retrace: ' -e '^recpid=' "$1" \
        | sed -E -e 's/^([A-Za-z0-9_.-]+)\[[0-9]+\]:/\1[<PID>]:/' -e 's/\(([0-9]+),0x[0-9a-f]+\)/(<PID>,<TID>)/g'
}

# M51 (deferred #8): is file $1 a prefix of file $2?
is_prefix() {
    n=$(wc -c < "$1")
    head -c "$n" "$2" | cmp -s - "$1"
}

pass=0; refused=0; fail=0; skip=0; nd=0; lno=0; OLDIFS=$IFS; TAB=$(printf '\t')
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
    lno=$((lno+1))
    case "$g" in ''|\#*) continue ;; esac
    # M49 (spec §3c): split off the row's arguments (see the header). `set --` reuses the
    # positional parameters, which nothing reads after `RAW` above.
    IFS=$TAB; set -f; set -- $g; set +f; IFS=$OLDIFS
    g=$1; shift; nargs=$#
    scope=in
    case "$g" in out:*) scope=out; g=${g#out:} ;; esac
    row=$g; [ "$nargs" -gt 0 ] && row="$g $*"
    cd "$ROOT" || { echo "TALLY ABORTED (cannot cd to $ROOT)"; exit 2; }
    if [ ! -x "$g" ]; then echo "SKIP $g (not present)"; skip=$((skip+1)); continue; fi

    # A previous iteration's trace must not survive to this one: if record fails via a
    # clean non-panic error path (exit(4) "RECORD ERROR") rather than a panic, it may
    # leave no fresh t.bin (or an old one may still be sitting in $TMP), and replay would
    # then silently replay the PREVIOUS binary's recording — manufacturing both false
    # passes and false failures, exactly what this script exists to prevent.
    # M36: rp.out/rp.err too — a row whose recorder timed out or panicked never runs replay, so
    # without this the rp.err keep_row copies as that row's evidence would be the previous row's.
    rm -f "$TMP/t.bin" "$TMP/rp.out" "$TMP/rp.err"
    # M36: the recorder's pid decides what several Apple binaries do (M34 §4b: a pid that lands in
    # a guest backing is forwarded as a host pointer by forward_and_diff's per-register probe, so
    # every self-pid csops/proc_info answers ESRCH; the window is [0x4000, 0x18000) for the six
    # guests measured — the fixed low backings plus the guest's own os_alloc_once slab at 0x10000,
    # so guest-dependent; see docs/sweep-evidence/2026-09-13-m36/README.md. M36 measured three
    # regimes, docs/sweep-evidence/2026-09-13-m36/). Print it into rec.err before the recorder
    # prints anything,
    # from the shell that becomes the recorder — M34's probe shape. run_timeout backgrounds
    # clean_run, which execs that `sh`, which execs `env -i`, which execs the recorder: the pid
    # it printed IS the recorder's, and the watchdog's kill still lands on the recorder.
    # M49: with RETRACE_SWEEP_NATIVE, the row runs natively first, under the same watchdog and
    # with the same /dev/null stdin; its exit code is the shell's (128 + signo for a signal death,
    # the CLI's own convention for a recorded one).
    if [ -n "$NATIVE" ]; then
        rm -f "$TMP/nat.out" "$TMP/nat.err"
        # M51 (Ruling T8-yes): bounded to 32 MiB of output (`ulimit -f 32768`, 1024-byte blocks in sh; measured: nat.out stops at 33554432 bytes). Native
        # comparison is on by default, and a native `/usr/bin/yes` ran its whole watchdog and wrote a
        # 15 GB file, filling the disk. A run past the limit dies of SIGXFSZ (rc 153) and its row fails,
        # as it always has. Only the native run is bounded: the recorder's trace is written by the
        # same process.
        ( ulimit -f 32768; run_timeout clean_run native "$g" "$@" ) >"$TMP/nat.out" 2>"$TMP/nat.err" </dev/null; nrc=$?
        [ -e "$TMP/.timedout" ] && nrc=timeout
    fi
    if [ "$nargs" -eq 0 ]; then
        run_timeout clean_run recorder "$BIN" record-dyn "$g" -o "$TMP/t.bin" >"$TMP/rec.out" 2>"$TMP/rec.err" </dev/null; rc=$?
    else
        # M49: the same shape, with the row's arguments after `--`, the CLI's guest-argv separator.
        run_timeout clean_run recorder "$BIN" record-dyn "$g" -o "$TMP/t.bin" -- "$@" >"$TMP/rec.out" 2>"$TMP/rec.err" </dev/null; rc=$?
    fi
    recpid=$(grep -a '^recpid=' "$TMP/rec.err" | head -1 | cut -d= -f2)
    native=""
    if [ -n "$NATIVE" ]; then
        if [ "$nrc" = timeout ]; then native="timeout:n/a"
        elif cmp -s "$TMP/rec.out" "$TMP/nat.out"; then native="$nrc:same"
        else native="$nrc:differs"; fi
    fi
    errcmp=n/a
    if [ -n "$NATIVE" ]; then
        mask_err "$TMP/rec.err" > "$TMP/rec.merr"
        mask_err "$TMP/nat.err" > "$TMP/nat.merr"
        if [ "$nrc" != timeout ]; then
            if cmp -s "$TMP/rec.merr" "$TMP/nat.merr"; then errcmp=same; else errcmp=differs; fi
        fi
    fi
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
    # recorder that hits a RECORD ERROR does not also panic, and vice versa. Both panic greps
    # (here and the label test below) anchor on `panicked at crates/` — the recorder's own source
    # paths — because the guest's stderr shares rec.err, so a Rust guest's own panic must never be
    # labelled a recorder panic (fix wave); a recorder panic located in a dependency would print a
    # registry path and miss the anchor — unobserved on the corpus.
    # M50: the bug line first (a bug is never a wall), else the wall line; empty when neither.
    rec_bug=$(bug_reason "$TMP/rec.err")
    rec_wall=$(grep -a -m1 '^retrace: unsupported: ' "$TMP/rec.err" | cut -c1-300)
    rec_reason=${rec_bug:-$rec_wall}
    rp_line=""; landmark="n/a"; rp="n/a"
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $row (timed out after ${TIMEOUT_SECS}s recording)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if [ -n "$rec_bug" ]; then
        echo "FAIL $row (retrace bug on record: $rec_bug)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    run_timeout clean_run retrace "$BIN" replay "$TMP/t.bin" >"$TMP/rp.out" 2>"$TMP/rp.err" </dev/null; rp=$?
    rp_line=$(grep -a -m1 'DIVERGENCE at landmark' "$TMP/rp.err" | cut -c1-200)
    case "$rp_line" in
        'DIVERGENCE at landmark '*) landmark=$(printf '%s' "$rp_line" | sed 's/^DIVERGENCE at landmark \([0-9]*\).*/\1/') ;;
    esac
    rp_bug=$(bug_reason "$TMP/rp.err")
    if [ -n "$rp_bug" ]; then
        echo "FAIL $row (retrace bug on replay: $rp_bug)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    # M50: a wall ends the recording with one line and exit 4, and its replay ends at the
    # incomplete-recording line with exit 4 (C2), so record and replay AGREE; without this check an
    # in-scope wall would pass (measured, ctl-before.log). The line is the test; rc is corroboration,
    # since a guest may exit 4 by design. Checked before the replay-timeout marker, as M36 ordered it:
    # the wall is the cause, whatever its replay then did, and the ROW line still carries rp.
    if [ -n "$rec_wall" ]; then
        if [ "$scope" = out ] && [ "$(grep -ac '^retrace: unsupported: ' "$TMP/rec.err")" -eq 1 ] \
            && [ "$rc" -eq 4 ] && [ "$rp" -eq 4 ] && grep -qa '^retrace: incomplete recording: ' "$TMP/rp.err" \
            && cmp -s "$TMP/rec.out" "$TMP/rp.out" \
            && { [ -z "$NATIVE" ] || { is_prefix "$TMP/rec.out" "$TMP/nat.out" && is_prefix "$TMP/rec.merr" "$TMP/nat.merr"; }; }; then
            echo "REFUSED $row ($rec_wall)"; refused=$((refused+1)); keep_row REFUSED; continue
        fi
        echo "FAIL $row (wall, rc=$rc: $rec_wall)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if [ -e "$TMP/.timedout" ]; then
        echo "FAIL $row (timed out after ${TIMEOUT_SECS}s replaying)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    # Divergence detection is structural, not inferred from exit codes alone: retrace
    # always prints this on a divergence, so check for it directly rather than relying
    # on the exit-code compare below to happen to disagree.
    if [ -n "$rp_line" ]; then
        echo "FAIL $row (replay diverged at landmark $landmark)"; fail=$((fail+1)); keep_row FAIL; continue
    fi
    if [ "$rc" -eq "$rp" ] && cmp -s "$TMP/rec.out" "$TMP/rp.out"; then
        # M49: with RETRACE_SWEEP_NATIVE, agreeing with itself is not enough: the recording must
        # also exit as native did and print what native printed.
        if [ -n "$NATIVE" ] && [ "$native" != "$rc:same" ]; then
            echo "FAIL $row (differs from native: record=$rc native=$native)"; fail=$((fail+1)); nd=$((nd+1)); keep_row FAIL; continue
        fi
        if [ -n "$STDERR" ] && [ "$errcmp" = differs ]; then
            echo "FAIL $row (stderr differs from native)"; fail=$((fail+1)); nd=$((nd+1)); keep_row FAIL; continue
        fi
        if [ "$rc" -ge 128 ]; then
            # M36: an identical FAULT on both sides is still counted a pass (the tally stays
            # comparable with M33–M35's), but it is said on the line: M35 found dddiagnose's
            # rc=139 passes are retrace-induced crashes (M34 §4b), not the guest's own. "Fault"
            # means signal death — the CLI exits 128+signo when the guest dies of one (139 =
            # SIGSEGV) — so the threshold is 128: a designed non-zero exit (/bin/false's 1, a
            # usage error's 64) is the guest's own status and stays a bare PASS, as before M36.
            echo "PASS $row (identical fault, rc=$rc)"; pass=$((pass+1)); keep_row PASS fault; continue
        fi
        echo "PASS $row"; pass=$((pass+1)); keep_row PASS; continue
    fi
    echo "FAIL $row (record=$rc replay=$rp)"; fail=$((fail+1)); keep_row FAIL
done 3< "$LIST"

echo "TALLY pass=$pass refused=$refused fail=$fail skip=$skip"
# M51 (R9): the rows that failed only against native pass when judged record against replay alone.
echo "TALLY-RR pass=$((pass+nd)) refused=$refused fail=$((fail-nd)) skip=$skip"
