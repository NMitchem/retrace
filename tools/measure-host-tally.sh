#!/bin/bash
# Tally a chunked gate's logs in M56's g3-tally.txt format.
# usage: measure-host-tally.sh <log-dir> [<prefix>] > tally.txt
#   Reads <log-dir>/<prefix>*.log; the chunk name is the file name without the prefix and ".log".
#   (M56's logs are g3-gate-<chunk>.log: pass "g3-gate-" as the prefix.)
set -u
D=${1:?usage: measure-host-tally.sh <log-dir> [<prefix>]}
PFX=${2:-}
strip() { LC_ALL=C sed -e $'s/\x1b\\[[0-9;]*[A-Za-z]//g' | LC_ALL=C tr -cd '\11\12\15\40-\176'; }
echo "# gate tally: every libtest result line from the chunk logs ($PFX<chunk>.log), ANSI and non-ASCII stripped."
TMP=$(mktemp "${TMPDIR:-/tmp}/tally.XXXXXX") || exit 0
for f in "$D"/"$PFX"*.log; do
  [ -f "$f" ] || continue
  n=$(basename "$f" .log); n=${n#"$PFX"}
  grep -a 'test result:' "$f" | strip | sed 's/^ *//' | while IFS= read -r l; do printf '%s\t%s\n' "$n" "$l"; done
done > "$TMP"
cat "$TMP"
echo
awk -F'\t' '{ if (match($2,/[0-9]+ passed/)) p+=substr($2,RSTART,RLENGTH-7);
  if (match($2,/[0-9]+ failed/)) f+=substr($2,RSTART,RLENGTH-7);
  if (match($2,/[0-9]+ ignored/)) i+=substr($2,RSTART,RLENGTH-8); b++ }
  END { printf "TOTAL passed=%d failed=%d ignored=%d binaries=%d\n", p, f, i, b }' "$TMP"
sk=0
for f in "$D"/"$PFX"*.log; do [ -f "$f" ] || continue; c=$(grep -a -c 'SKIPPED' "$f"); sk=$((sk+c)); done
echo "SKIPPED lines in all chunk logs: $sk"
fc=$(ls "$D"/"$PFX"*firstcontact_e2e.log 2>/dev/null | head -1)
[ -n "$fc" ] && echo "firstcontact_e2e: $(grep -a -o 'LAUNCH t1 [^|]*' "$fc" | strip | head -1 | sed 's/ *$//')"
rm -f "$TMP"
exit 0
