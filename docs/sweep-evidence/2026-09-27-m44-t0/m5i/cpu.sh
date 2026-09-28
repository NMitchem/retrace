#!/bin/bash
# M44 t0 M5(i) / Task 6 Step 7: user CPU of one test target (optionally one test), three runs.
# usage: cpu.sh <tree> <test-target> <test-name-or-empty> <out>
# Run with nothing else busy on the machine. Builds first (not timed).
cd "$1" || exit 2
cargo test -p retrace --test "$2" --no-run > "$4.build" 2>&1 || { echo "build failed" >> "$4"; exit 2; }
for i in 1 2 3; do
  echo "run $i" >> "$4"
  if [ -n "$3" ]; then
    /usr/bin/time -p cargo test -p retrace --test "$2" "$3" -- --test-threads=1 > "$4.run$i" 2>> "$4"
  else
    /usr/bin/time -p cargo test -p retrace --test "$2" -- --test-threads=1 > "$4.run$i" 2>> "$4"
  fi
  echo "exit=$?" >> "$4"
done
