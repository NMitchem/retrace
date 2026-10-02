#!/bin/bash
# M47 Task 6 Step 2 (the brief's, in substance): csh and tcsh past the refused fork, traced, then —
# whatever the record did — a replay of the recording with its exit code, the first DIVERGENCE line,
# and stdout compared with cmp. Native runs (stdin /dev/null, as the sweep runs them) for the
# native outcome. The recorder's console arm merges the guest's fd 1 and 2 into the CLI's stdout
# (Ruling T5-a), so the shell's own messages land in <s>.rec.out.
W=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite
E=$W/docs/sweep-evidence/2026-09-30-m47
cd "$W" || exit 2
echo "commit=$(git rev-parse --short HEAD) date=$(date '+%Y-%m-%d %H:%M:%S %Z') load=$(sysctl -n vm.loadavg)"
for s in csh tcsh; do
  echo "=== $s"
  /bin/$s < /dev/null > $E/$s.native.out 2> $E/$s.native.err
  echo "$s native exit=$? stdout-bytes=$(wc -c < $E/$s.native.out | tr -d ' ') stderr-bytes=$(wc -c < $E/$s.native.err | tr -d ' ')"
  rm -f /private/tmp/claude-501/m47-$s.bin
  export RETRACE_TRACE=1
  tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace record-dyn /bin/$s -o /private/tmp/claude-501/m47-$s.bin < /dev/null > $E/$s.rec.out 2> $E/$s.rec.err
  echo "$s record exit=$?"
  unset RETRACE_TRACE
  echo "$s traps=$(grep -a -c '^\[trap\] ' $E/$s.rec.err)"
  grep -a -E 'refusing fork|panicked at|RECORD ERROR|M33:|M47:' $E/$s.rec.err | head -4
  echo "$s rec.out bytes=$(wc -c < $E/$s.rec.out | tr -d ' ')"
  if [ -e /private/tmp/claude-501/m47-$s.bin ]; then
    tools/codesign-run.sh target/aarch64-apple-darwin/debug/retrace replay /private/tmp/claude-501/m47-$s.bin < /dev/null > $E/$s.rp.out 2> $E/$s.rp.err
    echo "$s replay exit=$?"
    grep -a -m1 'DIVERGENCE' $E/$s.rp.err
    cmp $E/$s.rec.out $E/$s.rp.out; echo "$s cmp=$?"
  else
    echo "$s: no trace written"
  fi
done
echo "load-end=$(sysctl -n vm.loadavg)"
