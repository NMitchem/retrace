#!/bin/bash
# census.sh <tag>: summarise a walk's stderr log
P=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/4b336710-6279-49f2-be58-212ed366d476/scratchpad/m48-probe
L=$P/logs/$1.err
echo "== $1: $(cat $P/logs/$1.status)"
echo "-- syscall histogram (top 40)"; grep -a '^\[trap\]' $L | sed -E 's/.*num=(-?[0-9]+) .*/\1/' | sort | uniq -c | sort -rn | head -40 | tr '\n' ' '; echo
echo "-- bsdthread_create (360): $(grep -ac '^\[trap\] num=360 ' $L)  workq_kernreturn(368): $(grep -ac '^\[trap\] num=368 ' $L)  sem_wait(-36): $(grep -ac '^\[trap\] num=-36 ' $L)  sem_signal(-33): $(grep -ac '^\[trap\] num=-33 ' $L)  ulock_wait(515): $(grep -ac '^\[trap\] num=515 ' $L)  ulock_wake(516): $(grep -ac '^\[trap\] num=516 ' $L)"
echo "-- kevent calls: $(grep -ac '^\[probe\] kevent#[0-9]* call' $L); changes by (kq,filter,flags,fflags):"
grep -a '^\[probe\] kevent#[0-9]* tid=.* change' $L | sed 's/.*tid=\([0-9]*\) kq=\([0-9]*\) change ident=\([^ ]*\) filter=\([^ ]*\) flags=\([^ ]*\) fflags=\([^ ]*\).*nev=\([0-9]*\) timeout=\(.*\)/tid=\1 kq=\2 filter=\4 flags=\5 fflags=\6 nev=\7 timeout=\8/' | sort | uniq -c
echo "-- kevent call shapes (tid,kq,nch,nev,timeout,immediate):"
grep -a '^\[probe\] kevent#[0-9]* call' $L | sed 's/.*call //' | sort | uniq -c
echo "-- kevent blocks/wakes/timeouts: BLOCK $(grep -ac 'kevent#[0-9]* BLOCK' $L) wake $(grep -ac 'kevent wake' $L) timeout $(grep -ac 'kevent timeout' $L)"
echo "-- psynch by op: $(grep -a '^\[probe\] psynch#' $L | awk '{print $3}' | sort | uniq -c | tr '\n' ' ')"
echo "-- psynch cvwait shapes (cv,flags,sec,nsec,mutex):"; grep -a '^\[probe\] psynch#[0-9]* cvwait' $L | sed 's/.*tid=\([0-9]*\) cv=\([^ ]*\) .*mutex=\([^ ]*\) .*flags=\([^ ]*\) sec=\([^ ]*\) nsec=\([^ ]*\)/tid=\1 cv=\2 mutex=\3 flags=\4 sec=\5 nsec=\6/' | sort | uniq -c
echo "-- cvwait timeouts: $(grep -ac 'cvwait timeout' $L)  prepost consumed: $(grep -ac 'consumes prepost' $L)  cvsignal-no-waiter: $(grep -ac 'no waiter, prepost' $L)"
echo "-- idle jumps: $(grep -ac 'idle jump' $L)"
echo "-- MAP_JIT: $(grep -a 'MAP_JIT mmap' $L | sed 's/\[probe\] //' | tr '\n' ';')"
echo "-- mprotect over JIT: $(grep -ac 'mprotect over MAP_JIT' $L)  munmap over JIT: $(grep -ac 'munmap over MAP_JIT' $L) non-JIT exec mmaps: $(grep -ac 'non-JIT exec' $L)"
echo "-- sprr writes logged (first 20 + every 1000th): last $(grep -a 'sprr write' $L | tail -1)"
echo "-- view flips: last $(grep -a 'view flip' $L | tail -1)"
echo "-- partial munmaps: $(grep -ac 'partial munmap' $L)"
echo "-- refusals/warnings: $(grep -a 'refusing\|retrace warn' $L | sort | uniq -c | head)"
