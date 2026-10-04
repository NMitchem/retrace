#!/bin/bash
# usage: run.sh <tag> <profile> -- node args...   (records node; logs to $P/logs/<tag>.*)
P=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/4b336710-6279-49f2-be58-212ed366d476/scratchpad/m48-probe
tag=$1; prof=$2; shift 3
mkdir -p $P/logs $P/traces
B=$P/wt/target/aarch64-apple-darwin/$prof/retrace
codesign -s - -f --entitlements $P/wt/retrace.entitlements $B >/dev/null 2>&1
s=$(date +%s)
export RETRACE_TRACE=1 RETRACE_PROBE=1
perl -e 'alarm 900; exec @ARGV' $B record-dyn /opt/homebrew/Cellar/node/25.6.1/bin/node -o $P/traces/$tag.bin -- "$@" < /dev/null 2> $P/logs/$tag.err | cat > $P/logs/$tag.out
rc=${PIPESTATUS[0]}
echo "rc=$rc secs=$(( $(date +%s) - s )) trace=$(stat -f %z $P/traces/$tag.bin 2>/dev/null) traps=$(grep -ac '^\[trap\]' $P/logs/$tag.err)" | tee $P/logs/$tag.status
