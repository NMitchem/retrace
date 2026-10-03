#!/bin/bash
# t0 M6: the thread timeline of a walk, from its full stderr. Lines that carry tid= are the probe's
# own (cvwait, cvsignal, cvbroad, kevent calls, SPRR writes); the trap lines for bsdthread_create
# (360), bsdthread_terminate (361), __ulock_wait/wake (515/516), semaphore_wait/signal (-36/-33) and
# workq_kernreturn (368) carry no tid, so each is shown as tid=? (the trap log does not say which
# thread issued it). Usage: thread-timeline.sh <tag>
P=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node/t0
LC_ALL=C awk '
  /^\[trap\]/ { n++
    if ($2 ~ /^num=(360|361|515|516|-36|-33|368)$/) {
      nm = $2; sub(/num=/, "", nm)
      name = (nm=="360")?"bsdthread_create":(nm=="361")?"bsdthread_terminate":(nm=="515")?"ulock_wait":(nm=="516")?"ulock_wake":(nm=="-36")?"semaphore_wait":(nm=="-33")?"semaphore_signal":"workq_kernreturn"
      match($0, /args=\[[^,]*,[^,]*/); a = substr($0, RSTART+6, RLENGTH-6)
      printf "T%d tid=? %s args=%s\n", n, name, a
    }
    next }
  /^\[probe\]/ && /tid=/ {
    match($0, /tid=[0-9]+/); last = substr($0, RSTART+4, RLENGTH-4)
    if ($0 ~ /sprr write|view flip/) next
    line = $0; sub(/^\[probe\] /, "", line)
    printf "T%d tid=%s %s\n", n, last, substr(line, 1, 110)
  }
' $P/m2-$1.err
