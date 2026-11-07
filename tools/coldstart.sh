#!/bin/bash
# M56 L4: the cold start. Taps, installs through Homebrew, records crashy and reverse-debugs it
# through lldb, in a scratch HOME. Run by hand (it needs Homebrew and the network or a local tap).
#
#   tools/coldstart.sh [--head] [--tap <dir-or-url>] [--cleanup]
#
# Prints one `COLDSTART pass` or `COLDSTART fail: <step>` line; exits 0 or 1. On a failure the scratch
# directory is kept and named. Without --cleanup it never untaps or uninstalls.
set -u

HEAD_FLAG=""
TAP_SRC="https://github.com/NMitchem/retrace"
CLEANUP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --head) HEAD_FLAG="--HEAD" ;;
    --tap) shift; TAP_SRC="${1:-}" ;;
    --cleanup) CLEANUP=1 ;;
    *) echo "usage: coldstart.sh [--head] [--tap <dir-or-url>] [--cleanup]" >&2; exit 2 ;;
  esac
  shift
done

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
SCRATCH="$(mktemp -d /private/tmp/retrace-coldstart.XXXXXX)" || { echo "COLDSTART fail: mktemp"; exit 1; }
STEP="start"
TAP_NAME="coldstart/retrace"
TAP_ADDED=0

fail() {
  echo "COLDSTART fail: $STEP"
  echo "coldstart: scratch directory kept: $SCRATCH" >&2
  exit 1
}

# Run one named step; any failure ends the script at it.
step() {
  STEP="$1"; shift
  echo "== $STEP" >&2
  "$@" || fail
}

mkdir -p "$SCRATCH/home" "$SCRATCH/work" || fail
BREW_PREFIX="$(brew --prefix)" || { STEP="brew --prefix"; fail; }
# The scratch HOME keeps the build's own cargo and rustup state out of the user's; brew reads its
# own prefix and caches from its environment, not HOME-relative state it cannot recreate.
export CARGO_HOME="$SCRATCH/home/.cargo"
export RUSTUP_HOME="$SCRATCH/home/.rustup"

if [ -d "$TAP_SRC" ]; then
  # A local checkout: Homebrew's download sandbox cannot read ~/Documents (Ruling P-3), so clone
  # the current branch bare under /private/tmp and point the formula's head line at that.
  STEP="local tap source"
  BRANCH="$(git -C "$TAP_SRC" rev-parse --abbrev-ref HEAD)" || fail
  BARE="$(mktemp -d /private/tmp/retrace-coldstart-bare.XXXXXX)" || fail
  git clone --bare --branch "$BRANCH" "$TAP_SRC" "$BARE" >&2 || fail
  step "scratch tap" brew tap-new --no-git "$TAP_NAME"
  TAP_ADDED=1
  TAP_DIR="$(brew --repository "$TAP_NAME")" || { STEP="tap dir"; fail; }
  mkdir -p "$TAP_DIR/Formula" || fail
  cp "$TAP_SRC/Formula/retrace.rb" "$SCRATCH/retrace.rb.orig" || { STEP="read formula"; fail; }
  sed "s|^  head .*|  head \"file://$BARE\", branch: \"$BRANCH\"|" "$SCRATCH/retrace.rb.orig" > "$TAP_DIR/Formula/retrace.rb" || fail
  echo "== the formula's one rewritten line (head)" >&2
  diff -u "$SCRATCH/retrace.rb.orig" "$TAP_DIR/Formula/retrace.rb" | tee "$SCRATCH/formula-head-rewrite.diff"
  if [ "$(diff "$SCRATCH/retrace.rb.orig" "$TAP_DIR/Formula/retrace.rb" | grep -c '^[<>]')" -ne 2 ]; then
    STEP="formula rewrite touched more than the head line"; fail
  fi
else
  step "brew tap" brew tap nmitchem/retrace "$TAP_SRC"
  TAP_ADDED=1
  TAP_NAME="nmitchem/retrace"
fi

# shellcheck disable=SC2086
step "brew install" brew install $HEAD_FLAG "$TAP_NAME/retrace"
step "brew test" brew test "$TAP_NAME/retrace"

RETRACE="$BREW_PREFIX/bin/retrace"
step "installed binary runs" "$RETRACE" --version
cd "$SCRATCH/work" || { STEP="cd work"; fail; }
step "copy crashy.c" cp "$BREW_PREFIX/share/retrace/examples/crashy.c" crashy.c
step "compile crashy" clang -o crashy crashy.c

STEP="record crashy"
echo "== $STEP" >&2
"$RETRACE" record ./crashy > crashy.stdout 2> crashy.stderr
RC=$?
cat crashy.stderr >&2
[ "$RC" -eq 139 ] || { echo "coldstart: record exited $RC, wanted 139" >&2; fail; }
grep -q 'retrace: saved the recording to crashy.retrace' crashy.stderr || { echo "coldstart: no saved line" >&2; fail; }
[ -s crashy.retrace ] || fail

# crashy writes g.ptr's value (&g.buf[0]) at stdout offset 15 (7 bytes "CRASHY:" plus 8 of st); the
# watched cell, &g.ptr, is buf[4]: 32 bytes on (Ruling P-9).
STEP="read the watch address"
HEX="$(dd if=crashy.stdout bs=1 skip=15 count=8 2>/dev/null | xxd -p -c 8)"
[ "${#HEX}" -eq 16 ] || { echo "coldstart: stdout too short for the address" >&2; fail; }
LE=""
for i in 14 12 10 8 6 4 2 0; do LE="$LE${HEX:$i:2}"; done
ADDR="$(printf '0x%x' $((0x$LE + 32)))"
echo "coldstart: watch address $ADDR" >&2

STEP="lldb script"
sed "s|@ADDR@|$ADDR|" "$REPO/crates/retrace/tests/fixtures/coldstart.lldb" > cmds.lldb || fail
STEP="retrace lldb"
echo "== $STEP" >&2
perl -e 'alarm 300; exec @ARGV' "$RETRACE" lldb crashy.retrace -- -x -b -s cmds.lldb > lldb.out 2>&1
RC=$?
cat lldb.out >&2
[ "$RC" -eq 0 ] || { echo "coldstart: retrace lldb exited $RC" >&2; fail; }
STEP="lldb stopped at the watchpoint"
grep -q 'stop reason = watchpoint 1' lldb.out || fail
STEP="lldb stopped in crashy main"
grep -q 'crashy`main' lldb.out || fail

echo "COLDSTART pass"
if [ "$CLEANUP" -eq 1 ]; then
  brew uninstall "$TAP_NAME/retrace" >&2
  [ "$TAP_ADDED" -eq 1 ] && brew untap "$TAP_NAME" >&2
  rm -rf "$SCRATCH"
else
  echo "coldstart: scratch directory: $SCRATCH" >&2
fi
exit 0
