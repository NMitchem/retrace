#!/bin/zsh
# M47 t0: symbolicate the BASE binary's `log -1` abort frames (fpwalk.py over logflake3/abort-stack.txt).
X=/Applications/Xcode.app/Contents/Developer/usr/bin/git
args=()
for a in 0x1804f18d8 0x1803f8644 0x1802f1a40 0x1802f528c 0x1802fa648 0x1803874dc 0x180387d64 0x180387aa4 0x1803a5ae4 0x1803a5a7c 0x1804edbb4 0x1804f8b98 0x1804f8b6c 0x1804edb50 0x1803a59c4; do
  args+=(-o "image lookup -a $a")
done
perl -e 'alarm 90; exec @ARGV' lldb -b -o "target create $X" $args 2>&1 | grep -a -E 'Summary'
atos -o $X -arch arm64 -l 0x100000000 0x1000fe708 0x1000fe154 0x100182d94 0x100184eec 0x10014200c 0x1001427ec 0x1000582b8 0x100058c9c 0x100002070 0x100001550 0x100000da4 0x1000ba8dc
