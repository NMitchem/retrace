#!/bin/zsh
# pc trace: break at <bp>, continue <hits> times, then N x (stepi; where) and optionally regs.
# Usage: pctrace.sh <bin> <trace> <bp> <hits> <N> [regs]
R=$1; T=$2; BP=$3; H=$4; N=$5; REGS=${6:-}
s="break $BP"
for i in $(seq 1 $H); do s="$s; continue"; done
s="$s; delete $BP; where"
for i in $(seq 1 $N); do
  if [ -n "$REGS" ]; then s="$s; stepi; regs"; else s="$s; stepi; where"; fi
done
$R debug $T --script "$s" 2>&1
