#!/bin/bash
# M47 Task 6: name the caller of node's refused kevent (363). x30 at the svc (node.entry.txt) is
# 0xa0bd066c0; node links libuv dynamically (otool -L), so the host's libuv is disassembled and the
# `bl _kevent` whose return address has the same offset from a 16 KiB-aligned base is located, with
# the static symbol nm names for the function that contains it. Measurement only.
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/docs/sweep-evidence/2026-09-30-m47
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
UV=/opt/homebrew/opt/libuv/lib/libuv.1.dylib
NODE=$(python3 -c 'import os; print(os.path.realpath("/opt/homebrew/bin/node"))')
{
  echo "# node: $NODE ($($NODE --version)); libuv: $(python3 -c "import os; print(os.path.realpath('$UV'))")"
  echo "# node's libuv linkage (otool -L):"
  otool -L "$NODE" | grep -a libuv
  otool -arch arm64 -tV "$UV" > $S/libuv.dis
  nm -arch arm64 "$UV" | sort > $S/libuv.nm
  echo "# nm: the static symbols bracketing offset 0x66bc"
  grep -a -E ' t _uv__async_fork$| t _uv__kqueue_runtime_detection$' $S/libuv.nm
  echo "# otool -tV: _uv__kqueue_runtime_detection from its kqueue() call to the check of kevent's result"
  sed -n '/^_uv__kqueue_runtime_detection:/,/^00000000000066e8/p' $S/libuv.dis
  echo "# guest x30 at the svc = 0xa0bd066c0 = 0xa0bd00000 + 0x66c0, the return address of the bl _kevent at 0x66bc"
} > $E/node.frame.txt 2>&1
echo "exit=$?"
