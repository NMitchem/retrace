#!/bin/sh
# M38 t5 Step 6 follow-up: re-record one binary under the chosen code from the shell, the way
# measure.sh did (worktree root as cwd, stdin /dev/null), tagged; used to check whether the wall a
# binary stops at is stable run-to-run in one launcher. Usage: rerun.sh <bin> <tag> [cwd]
set -u
K=docs/sweep-evidence/2026-09-16-m38
b=$1; tag=$2; n=$(basename "$b"); cwd=${3:-.}
R=/tmp/m38-t5/retrace-MACH_RCV_INVALID_NAME
( cd "$cwd" && sh -c 'echo "recpid=$$" >&2; exec "$0" record-dyn "$1" -o "$2"' "$R" "$b" "/tmp/m38-t5/m38-$n-$tag.bin" ) >"/tmp/m38-t5/$n.$tag.rec.out" 2>"/tmp/m38-t5/$n.$tag.rec.err" </dev/null; rc=$?
wall=$(grep -a 'RECORD ERROR\|M33: syscall\|guest crashed' "/tmp/m38-t5/$n.$tag.rec.err" | head -1 | cut -c1-60)
printf 'RERUN\t%s\t%s\tcwd=%s\trc=%s\t%s\n' "$n" "$tag" "$cwd" "$rc" "$wall"
