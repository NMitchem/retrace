#!/bin/bash
# usage: run-remote.sh <tag> <retrace-binary> <trace> <cmds-body-file>
# Starts `<retrace> gdbserver <trace> --port 0`, waits for its port, then runs
# /usr/bin/lldb -x -b -s <tag>.cmds </dev/null, bounded at 120 s, exactly as lldb_e2e's session()
# does (gdb-remote line prepended, retrace.py imported, END sentinel appended).
set -u
D=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed/t9diag
PY=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/crates/retrace/lldb/retrace.py
tag=$1; rt=$2; trace=$3; body=$4
cd "$D"
rm -f "$tag".srv.err "$tag".out "$tag".err "$tag".packets "$tag".unwind
"$rt" gdbserver "$trace" --port 0 </dev/null >/dev/null 2>"$tag".srv.err &
srv=$!
port=""
for i in $(seq 1 600); do
  port=$(sed -n 's/.*listening on 127.0.0.1:\([0-9]*\).*/\1/p' "$tag".srv.err)
  [ -n "$port" ] && break
  sleep 0.1
done
if [ -z "$port" ]; then echo "no port"; cat "$tag".srv.err; kill $srv; exit 1; fi
{
  # optional 5th arg: commands to run BEFORE the connect (the no-path route's `target create`)
  [ -n "${5:-}" ] && sed -e "s#@D@#$D#g" -e "s#@TAG@#$tag#g" "$5"
  echo "gdb-remote 127.0.0.1:$port"
  echo "command script import $PY"
  sed -e "s#@D@#$D#g" -e "s#@TAG@#$tag#g" "$body"
  echo 'script print("END")'
} > "$tag".cmds
perl -e 'alarm 120; exec @ARGV' /usr/bin/lldb -x -b -s "$tag".cmds </dev/null >"$tag".out 2>"$tag".err
echo "lldb exit=$?"
kill $srv 2>/dev/null; wait $srv 2>/dev/null
