#!/bin/bash
# Runs the three routed rows' command sequences (ni, step-out, finish) against both the HEAD
# server (retrace) and the patched one (retrace-nopagezero), on the same recording.
D=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m44-owed/.superpowers/sdd/2026-09-27-retrace-m44-owed/t9diag
for srv in retrace retrace-nopagezero; do
  pfx=head; [ "$srv" = retrace-nopagezero ] && pfx=patched
  for row in ni stepout finish; do
    tag="$pfx-$row"
    echo "=== $tag"
    bash "$D/run-remote.sh" "$tag" "$D/$srv" "$D/crashy.bin" "$D/$row.body"
    sed -n '/process continue/,$p' "$D/$tag.out" | grep -a -E '^\(lldb\)|pc = |^END|stop reason'
    cat "$D/$tag.err"
    echo "--- packets after the breakpoint stop:"
    grep -a -E 'send packet: \$(vCont|Z0|z0|c#|s#|m27|m1000)' "$D/$tag.packets" | sed 's/.*send packet: //'
  done
done
