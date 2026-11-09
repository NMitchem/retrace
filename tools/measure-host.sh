#!/bin/bash
# M57 Task K: the re-measurement kit. Run it on a macOS 26 volume to re-measure 26, and on any new macOS
# build before claiming support for it. From the repository root:
#   tools/measure-host.sh [--skip-gate] [--skip-sweeps] <out-dir>
# Writes <out>/environment.txt, baseline/, gate/ (unless --skip-gate), sweeps/ (unless --skip-sweeps) and
# README.md. Every program runs under an alarm, with an explicit minimal environment; nothing is written
# outside <out> (cargo's target dirs are <out>/target and <out>/gate/target). Exits 0 when it completes, whatever it measured.
set -u
SKIP_GATE=0; SKIP_SWEEPS=0; OUT=
for a in "$@"; do
  case "$a" in
    --skip-gate) SKIP_GATE=1 ;;
    --skip-sweeps) SKIP_SWEEPS=1 ;;
    -*) echo "usage: $0 [--skip-gate] [--skip-sweeps] <out-dir>" >&2; exit 2 ;;
    *) OUT=$a ;;
  esac
done
[ -n "$OUT" ] || { echo "usage: $0 [--skip-gate] [--skip-sweeps] <out-dir>" >&2; exit 2; }
[ -f Cargo.toml ] && [ -d tools ] || { echo "run from the repository root" >&2; exit 2; }
mkdir -p "$OUT" || exit 2
OUT=$(cd "$OUT" && pwd)
ROOT=$(pwd)
export CARGO_TARGET_DIR=$OUT/target
export TMPDIR=$OUT/tmp
mkdir -p "$TMPDIR"
lim() { perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$@"; }
# Delete retrace's leaked trace files, only ever inside the kit's own private TMPDIR (under <out>).
leak() { case "$TMPDIR" in "$OUT"/*) find "$TMPDIR" -maxdepth 1 -name 'retrace-*' -type f -delete 2>/dev/null ;; esac; }
sec() { printf '\n# %s\n' "$1"; }
CACHE_DIR=/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld
CACHE=$CACHE_DIR/dyld_shared_cache_arm64e

# ---- environment.txt ----
mkdir -p "$OUT/probe"
cc -o "$OUT/probe/ipa" -x c - -framework Hypervisor <<'CEOF' 2>"$OUT/probe/build.err"
#include <stdio.h>
#include <Hypervisor/Hypervisor.h>
int main(void){ uint32_t mx=0, df=0; hv_return_t r1=hv_vm_config_get_max_ipa_size(&mx); hv_return_t r2=hv_vm_config_get_default_ipa_size(&df); printf("max_ipa=%u (r=%d) default_ipa=%u (r=%d)\n", mx, r1, df, r2); return 0; }
CEOF
codesign --sign - --force --entitlements retrace.entitlements "$OUT/probe/ipa" >/dev/null 2>&1
u32() { od -An -t u4 -j "$2" -N 4 "$1" 2>/dev/null | tr -d ' '; }
u64hex() { od -An -t x8 -j "$2" -N 8 "$1" 2>/dev/null | tr -d ' '; }
{
  sec host; sw_vers; uname -a; sysctl -n machdep.cpu.brand_string hw.ncpu hw.memsize
  sec repo; git rev-parse HEAD; git status --short | head -5
  sec toolchain; xcode-select -p; pkgutil --pkg-info=com.apple.pkg.CLTools_Executables 2>/dev/null | sed -n 's/^version: /CLT version: /p'
  clang --version | head -1; swiftc --version 2>&1 | head -2; rustc --version; cargo --version
  sec lldb; lldb --version 2>&1 | head -2
  sec "homebrew (formulae the tests use)"
  brew --version 2>/dev/null | head -1; brew list --versions jq node python@3.10 python@3.13 python@3.14 xz 2>/dev/null
  sec /usr/lib/dyld; dwarfdump --uuid /usr/lib/dyld 2>&1; shasum -a 256 /usr/lib/dyld
  sec "shared cache files"; ls -l "$CACHE_DIR"/dyld_shared_cache_arm64e* 2>&1
  sec "shared cache header (hex via od)"
  echo "uuid(0x58) $(od -An -t x1 -j 88 -N 16 "$CACHE" 2>/dev/null | tr -d ' \n')"
  echo "mappingOffset(0x10) $(u32 "$CACHE" 16)"
  echo "sharedRegionStart(0xE0) 0x$(u64hex "$CACHE" 224)"
  echo "sharedRegionSize(0xE8) 0x$(u64hex "$CACHE" 232)"
  echo "subCacheArrayCount(0x18C) $(u32 "$CACHE" 396)"
  sec "hv_vm_config_get_max_ipa_size"; lim 30 "$OUT/probe/ipa" 2>&1
  sec "Apple binaries in the sweep: versions"
  for b in /bin/ls /bin/ed /usr/bin/git /bin/echo; do echo "$b $(dwarfdump --uuid $b 2>&1 | head -1 | awk '{print $2}')"; done
} > "$OUT/environment.txt" 2>&1

# ---- the release binary ----
cargo build --release -p retrace > "$OUT/build.log" 2>&1
R=$(find "$OUT/target" -path '*/release/retrace' -type f 2>/dev/null | head -1)
if [ -z "$R" ]; then echo "STOP: release build failed, see $OUT/build.log" > "$OUT/README.md"; exit 0; fi
codesign --sign - --force --entitlements retrace.entitlements "$R" >/dev/null 2>&1
BINSHA=$(shasum -a 256 "$R" | cut -d' ' -f1)

# ---- baseline ----
B=$OUT/baseline; P=$B/progs
mkdir -p "$B/traces" "$P"
for n in crashy hello_dyn; do clang -arch arm64 -o "$P/$n" "$ROOT/crates/retrace-guest/c/$n.c" 2>>"$B/build.err"; done
SUM=$B/summary.txt; : > "$SUM"
G=$(xcrun -f git 2>/dev/null)
cleanenv() { lim 300 env -i PATH=/usr/bin:/bin HOME="${HOME:-/}" TMPDIR="$TMPDIR" "$@"; }
{ echo "# $(sw_vers -productVersion) $(sw_vers -buildVersion)"; echo "# retrace: ${R#"$OUT"/}"; echo "$(shasum -a 256 < "$R" | cut -d" " -f1)  ${R#"$OUT"/}"; } >> "$SUM"
run() {
  local n=$1; shift; local t=$B/traces/$n.retrace
  # stdin is /dev/null for all three: an inherited stdin can be a socket (an agent's or an IDE's shell), and node's
  # libuv then calls getsockname on fd 0, a wall unrelated to the host being measured (M57 Task K review).
  cleanenv "$@" < /dev/null > "$B/$n.native.out" 2> "$B/$n.native.err"; local ne=$?
  cleanenv "$R" record -o "$t" "$@" < /dev/null > "$B/$n.record.out" 2> "$B/$n.record.err"; local re=$?
  lim 300 "$R" replay "$t" < /dev/null > "$B/$n.replay.out" 2> "$B/$n.replay.err"; local pe=$?
  local same=no
  cmp -s "$B/$n.native.out" "$B/$n.record.out" && cmp -s "$B/$n.record.out" "$B/$n.replay.out" && same=yes
  local sz; sz=$(stat -f %z "$t" 2>/dev/null || echo -)
  printf '%-9s native=%s record=%s replay=%s stdout-same=%s trace-bytes=%s\n' "$n" $ne $re $pe $same "$sz" | tee -a "$SUM" >/dev/null
}
run crashy    "$P/crashy"
run hello_dyn "$P/hello_dyn"
run echo      /bin/echo hi
run ls        /bin/ls /usr/bin
if [ -n "$G" ]; then run git "$G" --version; else echo "git       SKIPPED (xcrun -f git found nothing)" >> "$SUM"; fi
run jq        /opt/homebrew/bin/jq -n '[range(0;100000)] | add'
run python    /opt/homebrew/bin/python3 -c 'print(1)'
run node      /opt/homebrew/bin/node -e 'console.log(1)'
lim 300 "$R" debug "$B/traces/crashy.retrace" < /dev/null --script 'continue; where; regs; reverse-stepi; where; reverse-continue; where' \
  > "$B/crashy.debug.out" 2> "$B/crashy.debug.err"
echo "debug     exit=$?" >> "$SUM"
leak

# ---- gate ----
GT=
if [ "$SKIP_GATE" = 0 ]; then
  GD=$OUT/gate; mkdir -p "$GD/logs"; GT=$GD
  : > "$GD/summary.txt"; : > "$GD/chunks.txt"
  export TMPDIR=$GD/tmp; mkdir -p "$TMPDIR"
  export CARGO_TARGET_DIR=$GD/target
  echo "start $(date '+%Y-%m-%d %H:%M:%S %Z') commit=$(git rev-parse --short HEAD)" >> "$GD/summary.txt"
  grun() {
    local name=$1; shift
    local av; av=$(df -k "$OUT" | awk 'NR == 2 {print $4}')
    if [ "$av" -lt 8388608 ]; then echo "STOP before $name: $((av / 1024)) MiB available" >> "$GD/summary.txt"; return 1; fi
    local t0 t1 rc; t0=$(date +%s)
    "$@" < /dev/null > "$GD/logs/$name.log" 2>&1; rc=$?
    t1=$(date +%s)
    leak
    if [ "$rc" = 142 ]; then
      pkill -f "$GD/target/aarch64-apple-darwin/debug/deps/"
      pkill -f "$GD/target/aarch64-apple-darwin/debug/retrace-signed-"
      echo "$name: the alarm fired (a hang); the kit's leftover test processes killed" >> "$GD/chunks.txt"
    fi
    echo "$name exit=$rc" >> "$GD/summary.txt"
    echo "$name start=$(date -r "$t0" '+%H:%M:%S') end=$(date -r "$t1" '+%H:%M:%S') secs=$((t1 - t0)) load_end=$(sysctl -n vm.loadavg)" >> "$GD/chunks.txt"
    return 0
  }
  grun ws cargo test --workspace --exclude retrace-box --exclude retrace --no-fail-fast -- --test-threads=1 &&
  grun box cargo test -p retrace-box --no-fail-fast -- --test-threads=1 &&
  grun bins cargo test -p retrace --bins --no-fail-fast -- --test-threads=1 &&
  {
    for f in crates/retrace/tests/*.rs; do
      n=$(basename "$f" .rs)
      case "$n" in
        node_e2e) grun "e2e-$n" perl -e 'alarm 1800; exec @ARGV' cargo test -p retrace --test "$n" --no-fail-fast -- --test-threads=1 || break ;;
        node_crash_e2e) grun "e2e-$n" perl -e 'alarm 3000; exec @ARGV' cargo test -p retrace --test "$n" --no-fail-fast -- --test-threads=1 || break ;;
        *) grun "e2e-$n" cargo test -p retrace --test "$n" --no-fail-fast -- --test-threads=1 || break ;;
      esac
    done
    grun clippy cargo clippy --workspace --all-targets -- -D warnings
  }
  echo "end $(date '+%Y-%m-%d %H:%M:%S %Z')" >> "$GD/summary.txt"
  tools/measure-host-tally.sh "$GD/logs" > "$GD/tally.txt"
  export TMPDIR=$OUT/tmp; export CARGO_TARGET_DIR=$OUT/target
fi

# ---- sweeps ----
bugs() { grep -a -c 'retrace bug\|panicked at\|internal error' "$1"; }
tal() { grep -a '^TALLY' "$1" | tr '\n' ' '; }
SWL=
if [ "$SKIP_SWEEPS" = 0 ]; then
  SD=$OUT/sweeps; mkdir -p "$SD"
  HDR="binary=${R#"$OUT"/} commit=$(git rev-parse --short HEAD) sha256=$BINSHA date=$(date '+%Y-%m-%d %H:%M:%S %Z')"
  { echo "$HDR"; RETRACE_SWEEP_LIST=tools/firstcontact-sweep.txt RETRACE_SWEEP_STDERR=1 sh tools/apple-sweep.sh "$R"; echo "SWEEP_EXIT=$?"; } > "$SD/tier2.log" 2>&1
  leak
  grep -a '^TALLY' "$SD/tier2.log" > "$SD/tier2.tally"
  { echo "$HDR"; sh tools/apple-sweep.sh "$R"; echo "SWEEP_EXIT=$?"; } > "$SD/apple.log" 2>&1
  leak
  grep -a '^TALLY' "$SD/apple.log" > "$SD/apple.tally"
  SWL="tier2: $(tal "$SD/tier2.log") bug-lines=$(bugs "$SD/tier2.log")
apple: $(tal "$SD/apple.log") bug-lines=$(bugs "$SD/apple.log")"
fi

# ---- README.md ----
{
  echo "# measure-host: $(sw_vers -productName) $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
  echo
  echo "- date: $(date '+%Y-%m-%d %H:%M:%S %Z')"
  echo "- commit: $(git rev-parse HEAD)"
  echo "- release binary: $BINSHA"
  echo "- baseline: baseline/summary.txt; environment: environment.txt"
  if [ -n "$GT" ]; then
    echo "- gate: $(grep -a '^TOTAL' "$GT/tally.txt")"
    echo "- gate skips: $(grep -a '^SKIPPED' "$GT/tally.txt")"
    echo "- gate bug-lines: $(cat "$GT"/logs/*.log | grep -a -c 'retrace bug\|panicked at\|internal error')"
  else echo "- gate: skipped"; fi
  if [ -n "$SWL" ]; then printf '%s\n' "$SWL" | sed 's/^/- /'; else echo "- sweeps: skipped"; fi
  echo "- baseline bug-lines: $(cat "$B"/*.err | grep -a -c 'retrace bug\|panicked at\|internal error')"
} > "$OUT/README.md"
exit 0
