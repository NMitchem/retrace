#!/bin/bash
# M46 Task 6 fix round 1 (m2): symbolicate both of `_dispatch_kq_poll`'s kevent call returns with
# lldb against the host shared cache (target not running: unslid addresses) — M45's `kevent_qos`
# return 0x180359a2c (x30 in docs/sweep-evidence/2026-09-28-m45/automationmodetool.entry.txt) and
# M46's `kevent_id` return 0x180359a64 (x30 in automationmodetool.entry.txt) — plus the two call
# sites. Measurement only; reads no trace.
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers || exit 2
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/docs/sweep-evidence/2026-09-29-m46
{
  echo "# t6-kqpoll.sh: _dispatch_kq_poll's two kevent call returns, by lldb (host shared cache, unslid)."
  lldb --batch -o 'target create /usr/bin/automationmodetool' -o 'image lookup -a 0x180359a2c' -o 'image lookup -a 0x180359a64' -o 'disassemble -s 0x180359a24 -e 0x180359a30' -o 'disassemble -s 0x180359a5c -e 0x180359a68' 2>&1 | grep -a -v '^Current executable'
} > $E/automationmodetool.kqpoll.txt 2>&1; echo "lldb exit=$?"
cat $E/automationmodetool.kqpoll.txt
