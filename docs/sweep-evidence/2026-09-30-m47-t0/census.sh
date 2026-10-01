#!/bin/sh
# M47 t0 M1(a)/M2(a)/M2(c): every madvise (75) and __mac_syscall (381) dispatch across the corpus,
# from RETRACE_TRACE's [trap] lines and the census build's [m47]/[probe] lines. The shape of
# tools/destgaps-census.sh (bounded run, line-capped stderr, watchdog by command pattern).
# Usage: census.sh <signed-census-retrace> <guest-out-dir> <ledger-t0-dir> <repo-root> <out.tsv>
set -u
BIN=$1; GUESTS=$2; FIX=$3; ROOT=$4; OUT=$5
TMP=$(mktemp -d -t m47-census); trap 'rm -rf "$TMP"' EXIT INT TERM
: > "$OUT"
TIMEOUT_SECS=${RETRACE_SWEEP_TIMEOUT:-60}
LINE_CAP=400000
one() {
    label=$1; mode=$2; path=$3; shift 3
    rm -f "$TMP/t.bin" "$TMP/trace"
    ( exec env RETRACE_TRACE=1 "$BIN" "$mode" "$path" -o "$TMP/t.bin" "$@" 2>&1 >/dev/null </dev/null ) \
        | head -n "$LINE_CAP" > "$TMP/trace" &
    ppid=$!
    ( sleep "$TIMEOUT_SECS"; pkill -9 -f "^$BIN $mode $path" 2>/dev/null ) &
    wpid=$!
    wait "$ppid" 2>/dev/null; st=$?
    kill "$wpid" 2>/dev/null; wait "$wpid" 2>/dev/null
    total=$(grep -ac '^\[trap\]' "$TMP/trace")
    grep -aE '^\[trap\] num=(75|381) |^\[m47\] |^\[probe\] AMFI' "$TMP/trace" | sed "s|^|$label	|" >> "$OUT"
    last=$(grep -a -m1 -E 'panicked at|RECORD ERROR|M33:' "$TMP/trace" | cut -c1-160)
    echo "$label pipeline_exit=$st traps=$total last=$last"
}
for g in "$GUESTS"/*; do
    case "$g" in *.bin|*.s|*.txt|*.dylib) continue ;; esac
    [ -f "$g" ] && [ -x "$g" ] || continue
    file "$g" | grep -q 'Mach-O' || continue
    if otool -l "$g" 2>/dev/null | grep -q LC_LOAD_DYLINKER; then one "guest:$(basename "$g")" record-dyn "$g"
    else one "guest:$(basename "$g")" record "$g"; fi
done
rm -rf "$TMP/fsops" && mkdir "$TMP/fsops" && one "fix:fsops" record-dyn "$FIX/fsops_dyn" -- "$TMP/fsops"
for m in zero reuse bad; do one "fix:madv-$m" record-dyn "$FIX/madv_dyn" -- "$m"; done
one "fix:rpath" record-dyn "$FIX/rpath_dyn"
one "fix:forkfail" record-dyn "$FIX/forkfail_dyn"
JQ=/opt/homebrew/bin/jq
[ -x "$JQ" ] && one "jq:--version" record-dyn "$JQ" -- --version || echo "SKIP jq"
[ -x "$JQ" ] && one "jq:file" record-dyn "$JQ" -- .name "$ROOT/crates/retrace/tests/fixtures/rung3.json"
PY=/opt/homebrew/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python
[ -x "$PY" ] && one "cpython:print" record-dyn "$PY" -- -c 'print(1)' || echo "SKIP cpython"
[ -x "$PY" ] && one "cpython:crash" record-dyn "$PY" -- "$ROOT/crates/retrace-guest/py/crash.py"
NODE=/opt/homebrew/bin/node
if [ -x "$NODE" ]; then NR=$(cd "$(dirname "$NODE")" && cd "$(dirname "$(readlink "$NODE")")" && pwd)/$(basename "$(readlink "$NODE")")
    one "node:-e" record-dyn "$NR" -- -e 'console.log(1)'; else echo "SKIP node"; fi
while IFS= read -r g <&3; do
    case "$g" in ''|\#*) continue ;; esac
    [ -x "$g" ] || { echo "SKIP $g"; continue; }
    one "apple:$g" record-dyn "$g"
done 3< "$ROOT/tools/apple-sweep-binaries.txt"
echo "DONE matched_total=$(wc -l < "$OUT" | tr -d ' ') -> $OUT"
