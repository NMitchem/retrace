#!/bin/bash
# t0 extract: the [probe] lines of a walk's full stderr, each prefixed with T<n>, the ordinal of the
# [trap] line it follows (the trap count, which tracks the landmark index), plus the trap lines of
# the calls the censuses tabulate (kevent 363, psynch 301-305/312, munmap 73, mmap 197, mprotect 74,
# madvise 75, setsockopt 105, getsockname 32, bsdthread_create 360, close 6/399, kqueue 362).
# Usage: extract.sh <tag>  (reads m2-<tag>.err, writes m2-<tag>.probe.txt)
P=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/.superpowers/sdd/2026-10-02-retrace-m48-node/t0
LC_ALL=C awk '
  /^\[trap\]/ { n++; if ($2 ~ /^num=(363|30[1-5]|312|73|197|74|75|105|32|360|6|399|362|-36|-33)$/) print "T" n " " $0; next }
  /^\[probe/ || /^\[fault\]/ || /refusing/ || /forwarding mach_msg2/ { print "T" n " " $0 }
' $P/m2-$1.err > $P/m2-$1.probe.txt
wc -l $P/m2-$1.probe.txt
