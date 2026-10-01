#!/bin/zsh
# M47 t0 M1(b) follow-up: symbolicate the aborting mode-B run's frames (fpwalk.py over
# m1b-why-stack.txt). Cache frames by a static lldb image lookup (retrace's cache slide is 0, so the
# guest VA is the unslid address); git frames by atos at git's unslid base 0x100000000.
X=/Applications/Xcode.app/Contents/Developer/usr/bin/git
perl -e 'alarm 90; exec @ARGV' lldb -b -o "target create /usr/bin/true" \
  -o "image lookup -a 0x1804f18d8" -o "image lookup -a 0x1803f8644" -o "image lookup -a 0x1802f1a40" \
  -o "image lookup -a 0x1802f528c" -o "image lookup -a 0x1802fa648" -o "image lookup -a 0x19028d918" 2>&1 | grep -a -E 'lookup|Summary'
lipo -archs $X
atos -o $X -arch arm64 -l 0x100000000 0x10012dc8c 0x10015f338 0x10015f1e0 0x1000ed7cc 0x100023f5c 0x100002070 0x100001550 0x100000da4 0x1000ba8dc
