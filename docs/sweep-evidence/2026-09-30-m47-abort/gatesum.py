#!/usr/bin/env python3
# Sum the libtest result lines of a gate log, and count test binaries (result lines).
import re, sys
t = open(sys.argv[1], errors='replace').read()
p = f = i = n = 0
for m in re.finditer(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored', t):
    n += 1; p += int(m.group(2)); f += int(m.group(3)); i += int(m.group(4))
print(f'binaries={n} passed={p} failed={f} ignored={i}')
for m in re.finditer(r'^test (\S+) \.\.\. FAILED', t, re.M):
    print('FAILED', m.group(1))
