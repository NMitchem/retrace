#!/bin/bash
# M1(b) with a watchdog: lldb is killed after $1 seconds (default 540) if it has not exited.
# $2 = fixture args (optional), $3 = log name (default m1b.log)
cd /Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers
export L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m46-gcdtimers/.superpowers/sdd/2026-09-29-retrace-m46-gcdtimers
LIM=${1:-540}
BIN=${4:-after_dyn}
LOG=${3:-m1b.log}
lldb -b -s $L/t0/m1b.lldb -- $L/t0/$BIN $2 > $L/t0/$LOG 2>&1 &
P=$!
( sleep $LIM; kill $P 2>/dev/null && echo "watchdog killed lldb after ${LIM}s" ) &
W=$!
wait $P; echo "exit=$?"
kill $W 2>/dev/null
grep -a -E 'frame #0' $L/t0/$LOG | sed 's/.*`//' | sort | uniq -c
grep -a -E 'frame #[0-9]+:.*(_dispatch_event_loop_timer_arm|_dispatch_timers_program|_dispatch_timers_run|_dispatch_timers_get_delay|_dispatch_event_loop_drain_timers)' $L/t0/$LOG | head
grep -a -E 'Process [0-9]+ exited' $L/t0/$LOG
