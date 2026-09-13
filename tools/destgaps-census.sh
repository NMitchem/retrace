#!/bin/sh
# M34-destgaps measurement: every dispatch of proc_info (336), getattrlist (220),
# fgetattrlist (228), csops (169) and csops_audittoken (170) across the whole corpus,
# with its arguments — so each row's length operand is MEASURED against the 64 KiB
# flat window rather than assumed from a prototype (the charter's M20 warning).
#
# Corpus = the M33 census corpus: every Mach-O in the retrace-guest OUT_DIR (static via
# `record`, dynamic via `record-dyn`, classified by LC_LOAD_DYLINKER), `jq --version`,
# `jq .name <fixture>`, the CPython interpreter and its launcher, and every binary in
# tools/apple-sweep-binaries.txt. Bare argv except where the gate itself passes one.
#
# Output: $OUT is a TSV of `<label>\t<[trap] line>` for the five numbers only. Progress
# lines (`<label> exit=<n> traps=<total> matched=<k>`) go to stdout so a guest that died
# in dyld (tiny `traps=`) is visible next to one that ran.
#
# Usage: destgaps-census.sh <retrace-binary> <guest-out-dir> <out.tsv>
set -u
RAW=$1; GUESTS=$2; OUT=$3
ROOT=$(cd "$(dirname "$0")/.." && pwd)

[ -x "$RAW" ] || { echo "no retrace binary at $RAW" >&2; exit 2; }
BIN=$RAW-census-$$
# The trap is installed BEFORE the copy, so a codesign failure cannot exit past it and leave
# the `$BIN` copy behind in `target/` (M34 review minor, fixed in the fix wave).
TMP=$(mktemp -d -t retrace-census)
trap 'rm -rf "$TMP" "$BIN"' EXIT INT TERM
cp "$RAW" "$BIN"
codesign -s - -f --entitlements "$ROOT/retrace.entitlements" "$BIN" >/dev/null 2>&1 || {
    echo "codesign failed for $BIN" >&2; exit 2; }
: > "$OUT"

TIMEOUT_SECS=${RETRACE_SWEEP_TIMEOUT:-30}
NUMS='336|220|228|169|170'

# Bounded run, in the shape of tools/apple-sweep.sh (no timeout(1) on macOS) with one
# difference forced by RETRACE_TRACE: the trace is stderr, and a guest that floods its
# stdout (/usr/bin/yes) floods the trace with it — MEASURED at 5 GB in the 30 s before
# the watchdog fired, on the first run of this script, which then spent its time grepping
# the file rather than measuring anything. So stderr is streamed through a LINE-CAPPED
# filter instead of landing in a file: `head` closes the pipe at the cap and the recorder
# dies on SIGPIPE at its next trace line, long before the watchdog. The watchdog is still
# needed for the other shape — a guest that blocks silently and writes nothing — and it
# kills by command pattern because `$!` of a backgrounded pipeline is the pipeline's LAST
# process (the filter), not the recorder.
#
# Every `[trap]` line of the five numbers M34 covers is init-time (dyld, libSystem), so a
# cap of 400k trace lines — ~1,500× the ~260 traps a hello-world issues — loses nothing
# these rows need while bounding the flood. Scratch variable deliberately not named
# rc/rp — see the sweep script's fix-round-1 note.
LINE_CAP=400000
one() {
    label=$1; mode=$2; path=$3; shift 3
    rm -f "$TMP/t.bin" "$TMP/trace"
    ( exec env RETRACE_TRACE=1 "$BIN" "$mode" "$path" -o "$TMP/t.bin" "$@" 2>&1 >/dev/null </dev/null ) \
        | head -n "$LINE_CAP" > "$TMP/trace" &
    ppid=$!
    ( sleep "$TIMEOUT_SECS"; pkill -9 -f "^$BIN $mode $path" 2>/dev/null ) &
    wpid=$!
    wait "$ppid" 2>/dev/null
    _tmo_status=$?
    kill "$wpid" 2>/dev/null
    wait "$wpid" 2>/dev/null
    total=$(grep -ac '^\[trap\]' "$TMP/trace")
    capped=""; [ "$(wc -l < "$TMP/trace" | tr -d ' ')" -ge "$LINE_CAP" ] && capped=" CAPPED"
    matched=$(grep -aE "^\[trap\] num=($NUMS) " "$TMP/trace" | wc -l | tr -d ' ')
    grep -aE "^\[trap\] num=($NUMS) " "$TMP/trace" | sed "s|^|$label	|" >> "$OUT"
    echo "$label pipeline_exit=$_tmo_status traps=$total matched=$matched$capped"
}

# RETRACE_CENSUS_ONLY_APPLE=1 skips the repo/jq/CPython sections (re-running a subset of
# the sweep list, via RETRACE_SWEEP_LIST, after a partial run).
SWEEP_LIST=${RETRACE_SWEEP_LIST:-$ROOT/tools/apple-sweep-binaries.txt}
if [ -z "${RETRACE_CENSUS_ONLY_APPLE:-}" ]; then
# ---- repo guests ----------------------------------------------------------------
for g in "$GUESTS"/*; do
    case "$g" in *.bin|*.s|*.txt) continue ;; esac
    [ -f "$g" ] && [ -x "$g" ] || continue
    file "$g" | grep -q 'Mach-O' || continue
    if otool -l "$g" 2>/dev/null | grep -q LC_LOAD_DYLINKER; then
        one "guest:$(basename "$g")" record-dyn "$g"
    else
        one "guest:$(basename "$g")" record "$g"
    fi
done

# ---- jq (rungs 2-3), with the gate's own argv -----------------------------------
JQ=/opt/homebrew/bin/jq
if [ -x "$JQ" ]; then
    one "jq:--version" record-dyn "$JQ" -- --version
    one "jq:file" record-dyn "$JQ" -- .name "$ROOT/crates/retrace/tests/fixtures/rung3.json"
else
    echo "SKIP jq (not present)"
fi

# ---- CPython (rung 7): the interpreter and the launcher shim ---------------------
PY=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
PYL=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/bin/python3.14
[ -x "$PY" ]  && one "cpython:interp"   record-dyn "$PY"  -- -c 'print(1)' || echo "SKIP cpython interp"
[ -x "$PYL" ] && one "cpython:launcher" record-dyn "$PYL" -- -c 'print(1)' || echo "SKIP cpython launcher"
fi

# ---- the Apple sweep corpus, bare argv, stdin from /dev/null ---------------------
while IFS= read -r g <&3; do
    case "$g" in ''|\#*) continue ;; esac
    [ -x "$g" ] || { echo "SKIP $g (not present)"; continue; }
    one "apple:$g" record-dyn "$g"
done 3< "$SWEEP_LIST"

echo "DONE matched_total=$(wc -l < "$OUT" | tr -d ' ') -> $OUT"
