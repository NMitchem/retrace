#!/bin/bash
# M48 Task 9: one walk's census from its traced stderr: every distinct syscall number (Task 10
# reads the `nums=` line), the counts M48's subsystems are measured by, the MAP_JIT mmaps, and
# the SPRR writes by thread and by value. Usage: t9-census.sh <tag>
E=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m48-node/docs/sweep-evidence/2026-10-02-m48
f=$E/walk-$1.err
echo "walk=$1 traps=$(grep -ac '^\[trap\]' $f)"
echo "nums=$(grep -a '^\[trap\] num=' $f | sed 's/^\[trap\] num=\([-0-9]*\) .*/\1/' | sort -n -u | tr '\n' ' ')"
for n in 363 303 304 305 297 298 299 300 301 302 306 307 308 309 312 360 361 105 32 73 74 75; do
  echo "num $n: $(grep -ac "^\[trap\] num=$n " $f)"
done
perl -ne 'if (/^\[trap\] num=197 .*args=\[([^\]]*)\]/) { my @a = split /,/, $1; if (hex($a[3]) & 0x800) { $n++; print "map_jit len=$a[1] prot=$a[2] flags=$a[3]\n" } } END { print "map_jit=", ($n // 0), "\n" }' $f
echo "sprr writes=$(grep -ac '^\[M48 SPRR\] ' $f)"
grep -a '^\[M48 SPRR\] ' $f | awk '{print $4}' | sort -n | uniq -c | awk '{print "sprr thread " $2 ": " $1}'
grep -a '^\[M48 SPRR\] ' $f | awk '{print $6}' | sort | uniq -c | awk '{print "sprr value " $2 ": " $1}'
echo "msgh_ids=$(grep -a '^\[mach_msg2\]' $f | grep -a -o 'msgh_id=[0-9]*' | sort -u | tr '\n' ' ')"
