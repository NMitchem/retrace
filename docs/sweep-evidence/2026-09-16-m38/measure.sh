#!/bin/sh
# M38 Task 5 Step 5: choose MACH_RCV_REFUSAL by measurement. For each candidate code, build the
# signed CLI with that code as the refusal, then record+replay each of the six M37-parked Apple
# binaries under it. One ROW per (binary, code): 18 cells. Run from the worktree root, detached:
#   nohup sh docs/sweep-evidence/2026-09-16-m38/measure.sh > docs/sweep-evidence/2026-09-16-m38/measure.log 2>&1 &
# The constant is edited with sed and restored with `git checkout` of the file (NOT a stash — the
# stash stack is shared), so Steps 1-4 must be committed first.
set -u
K=docs/sweep-evidence/2026-09-16-m38; mkdir -p "$K"
BINS="/bin/launchctl /usr/bin/automationmodetool /usr/bin/desdp /usr/bin/dyld_info /usr/bin/flex /usr/bin/dddiagnose"
TMPBIN=/tmp/m38-t5; mkdir -p "$TMPBIN"
for code in MACH_RCV_TIMED_OUT MACH_RCV_INVALID_NAME MACH_RCV_PORT_DIED; do
  sed -i '' "s/^pub const MACH_RCV_REFUSAL: u64 = .*/pub const MACH_RCV_REFUSAL: u64 = $code;/" crates/retrace-core/src/machmsg.rs
  grep -n '^pub const MACH_RCV_REFUSAL' crates/retrace-core/src/machmsg.rs
  cargo build -p retrace 2>&1 | tail -1
  cp target/aarch64-apple-darwin/debug/retrace "$TMPBIN/retrace-$code"
  codesign -s - -f --entitlements retrace.entitlements "$TMPBIN/retrace-$code"
  for b in $BINS; do n=$(basename $b)
    # recpid: the sh that echoes is the process that exec's into the recorder (apple-sweep.sh idiom).
    perl -e 'alarm shift; exec @ARGV' 180 sh -c 'echo "recpid=$$" >&2; exec "$0" record-dyn "$1" -o "$2"' "$TMPBIN/retrace-$code" "$b" "$TMPBIN/m38-$n-$code.bin" >"$K/$n.$code.rec.out" 2>"$K/$n.$code.rec.err" </dev/null; rc=$?
    perl -e 'alarm shift; exec @ARGV' 180 "$TMPBIN/retrace-$code" replay "$TMPBIN/m38-$n-$code.bin" >"$K/$n.$code.rp.out" 2>"$K/$n.$code.rp.err" </dev/null; rp=$?
    refusals=$(grep -ac 'refusing mach_msg2 message-queue receive' "$K/$n.$code.rec.err")
    wall=$(grep -a 'RECORD ERROR\|panicked at\|DIVERGENCE' "$K/$n.$code.rec.err" "$K/$n.$code.rp.err" | head -1 | cut -c1-160)
    printf 'ROW\t%s\t%s\trc=%s\trp=%s\treceive-refusals=%s\tstdout-equal=%s\t%s\n' "$n" "$code" "$rc" "$rp" "$refusals" "$(cmp -s "$K/$n.$code.rec.out" "$K/$n.$code.rp.out" && echo y || echo n)" "$wall"
  done
done | tee "$K/measure.tsv"
git checkout crates/retrace-core/src/machmsg.rs   # restore the committed default before choosing
echo MEASURE_DONE
