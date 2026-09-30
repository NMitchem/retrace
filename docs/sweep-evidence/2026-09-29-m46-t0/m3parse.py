#!/usr/bin/env python3
"""Parse an M3 lldb log into the ordered kevent_qos / __workq_kernreturn call list."""
import re, sys

FILT = {-7: "TIMER", -8: "MACHPORT", -10: "USER", -14: "MEMORYSTATUS", -1: "READ", -2: "WRITE"}

def s16(v):
    return v - 0x10000 if v & 0x8000 else v

def clean(text):
    text = re.sub(r'\x1b\[[0-9;]*m', '', text)
    return ''.join(ch for ch in text if ch in '\t\n\r' or 32 <= ord(ch) < 127)

def entries(qwords, n):
    out = []
    for i in range(n):
        q = qwords[i * 9:(i + 1) * 9]
        if len(q) < 9:
            out.append("(entry %d not captured)" % i)
            continue
        ident, w1, udata, ff, data, e0, e1, e2, e3 = q
        filt = s16(w1 & 0xffff); flags = (w1 >> 16) & 0xffff; qos = (w1 >> 32) & 0xffffffff
        fflags = ff & 0xffffffff; xflags = ff >> 32
        out.append("ident=%#x filter=%d(%s) flags=%#06x qos=%#010x udata=%#x fflags=%#x xflags=%#x data=%#x ext=[%#x,%#x,%#x,%#x]" % (
            ident, filt, FILT.get(filt, "?"), flags, qos, udata, fflags, xflags, data, e0, e1, e2, e3))
    return out

def main(path):
    text = clean(open(path, 'rb').read().decode('latin-1'))
    blocks = re.split(r'\(lldb\)\s+register read ', text)[1:]
    idx = 0
    for b in blocks:
        regs = dict((k, int(v, 16)) for k, v in re.findall(r'\b(x[0-7]) = (0x[0-9a-f]+)', b))
        qwords = []
        mm = re.search(r'memory read[^\n]*\n((?:0x[0-9a-f]+:(?: 0x[0-9a-f]+)+\n)+)', b)
        if mm:
            for line in mm.group(1).splitlines():
                qwords += [int(x, 16) for x in line.split(':', 1)[1].split()]
        ti = re.search(r'thread #(\d+): tid = (0x[0-9a-f]+), 0x[0-9a-f]+ \S+`(\w+)(?:, queue = \'([^\']*)\')?', b)
        if not ti:
            continue
        idx += 1
        th, tid, fn, q = ti.group(1), ti.group(2), ti.group(3), ti.group(4) or ""
        if fn == "kevent_qos":
            n = regs.get('x2', 0)
            print("%2d. kevent_qos  thread #%s (%s) [%s] x0=%#x x2=%d x3=%#x x4=%#x x5=%#x x6=%#x x7=%#x" % (
                idx, th, tid, q, regs['x0'], n, regs['x3'], regs['x4'], regs['x5'], regs['x6'], regs['x7']))
            for e in entries(qwords, min(n, 1)):
                print("       change: " + e)
        elif fn == "__workq_kernreturn":
            op = regs['x0']
            print("%2d. workq_kernreturn op=%#x thread #%s (%s) [%s] x1=%#x x2=%#x x3=%#x" % (
                idx, op, th, tid, q, regs['x1'], regs['x2'], regs['x3']))
            if op == 0x40 and regs['x1']:
                for e in entries(qwords, regs['x2']):
                    print("       change: " + e)
        else:
            print("%2d. %s thread #%s (%s) [%s] regs=%s" % (idx, fn, th, tid, q, {k: hex(v) for k, v in regs.items()}))
    m = re.search(r'Process \d+ exited with status = (\d+)', text)
    print("exit status:", m.group(1) if m else "not seen")
    print("program output:", [l for l in text.splitlines() if re.match(r'^(fired|done|A|B|tick \d|clock \w+)$', l)])

if __name__ == '__main__':
    main(sys.argv[1])
