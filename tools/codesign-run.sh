#!/bin/sh
# cargo invokes: codesign-run.sh <binary> [args...]
set -e
bin="$1"; shift
ent="$(dirname "$0")/../retrace.entitlements"
codesign -s - -f --entitlements "$ent" "$bin" >/dev/null 2>&1
exec "$bin" "$@"
