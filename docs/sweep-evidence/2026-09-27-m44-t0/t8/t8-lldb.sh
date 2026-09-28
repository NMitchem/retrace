#!/bin/bash
# M44 t0 M5(iii)/(iv): one bounded lldb session against `retrace gdbserver`, with gdb-remote packet
# logging. usage: t0-m5-lldb.sh <label> <trace> <body-file>
# <body-file> holds the lldb commands that go between the connect/import lines and the END line.
# Writes $L/t0-m5-<label>.{cmds,packets,out,err,srv.err} and prints the counts.
set -u
label=$1; trace=$2; body=$3
ROOT=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed
L=$ROOT/.superpowers/sdd/2026-09-27-retrace-m44-owed
BIN=$ROOT/target/aarch64-apple-darwin/debug/retrace
P=$L/t8-lldb-$label
rm -f "$P".packets "$P".out "$P".err "$P".srv.err "$P".cmds
codesign -s - -f --entitlements "$ROOT/retrace.entitlements" "$BIN" >/dev/null 2>&1 || { echo "codesign failed"; exit 2; }
"$BIN" gdbserver "$trace" --port 0 </dev/null >/dev/null 2>"$P".srv.err &
srv=$!
port=""
for _ in $(seq 1 600); do
    port=$(sed -n 's/.*listening on 127\.0\.0\.1:\([0-9][0-9]*\).*/\1/p' "$P".srv.err | head -1)
    [ -n "$port" ] && break
    kill -0 "$srv" 2>/dev/null || break
    sleep 0.1
done
if [ -z "$port" ]; then echo "server never listened"; cat "$P".srv.err; kill -9 "$srv" 2>/dev/null; exit 2; fi
{
    echo "log enable -f $P.packets gdb-remote packets"
    echo "gdb-remote 127.0.0.1:$port"
    echo "command script import $ROOT/crates/retrace/lldb/retrace.py"
    cat "$body"
    echo 'script print("END")'
} > "$P".cmds
perl -e 'alarm shift; exec @ARGV' 120 /usr/bin/lldb -x -b -s "$P".cmds </dev/null >"$P".out 2>"$P".err
rc=$?
kill -9 "$srv" 2>/dev/null
wait "$srv" 2>/dev/null
end=no; grep -a -q -x 'END' "$P".out && end=yes
echo "LABEL=$label port=$port lldb_exit=$rc END=$end vCont_s=$(grep -a -c 'vCont;s' "$P".packets) vCont_any=$(grep -a -c 'vCont;' "$P".packets)"
