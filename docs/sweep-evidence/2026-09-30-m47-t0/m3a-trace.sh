#!/bin/zsh
# M47 t0 M3(a) fallback (deviation): lldb cannot launch a debuggee on this host (Developer mode
# disabled; m2b-rpath.log hung at `run`), so the 3403 REQUEST is read from RETRACE_TRACE's mach_msg2
# send decode on the base binary instead. The request bytes are built by the guest's own libxpc /
# libsystem_kernel code, so their layout is what native sends; the port NAMES are retrace's.
R=/private/tmp/claude-501/m47-base-retrace
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
export RETRACE_TRACE=1
$R record-dyn $L/t0/forkfail_dyn -o /private/tmp/claude-501/m47-m3a.bin > $L/t0/m3a-trace.out 2> $L/t0/m3a-trace.err; echo "rc=$?"
unset RETRACE_TRACE
grep -a -n -E 'num=(195|333|2|-31|-47) |mach_msg2|3403|RECORD ERROR' $L/t0/m3a-trace.err | tail -30
