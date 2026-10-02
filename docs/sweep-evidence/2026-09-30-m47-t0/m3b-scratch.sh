#!/bin/zsh
# M47 t0 M3(b) fallback (deviation; no debugger on this host): a THROWAWAY scratch build
# (m3b-scratch.patch = census-build.patch + 3403 answered KERN_SUCCESS by a mig_reply_error +
# syscall 2 refused EAGAIN, carry set, no writes, never forwarded). Under RETRACE_TRACE it lists every
# trap from the 3403 through process exit, for forkfail_dyn and for default-config `git commit`
# (fresh repo, identity by -c, NO maintenance.auto=false). PROBE_NOREUSABLE=1 makes advice 7/8 no-ops
# as Task 2's model will, so g35's forwarded-madvise abort cannot pre-empt the fork path.
R=/private/tmp/claude-501/m47-m3b-retrace
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
O=$L/t0/m3b
mkdir -p $O
export PROBE_NOREUSABLE=1
export RETRACE_TRACE=1
perl -e 'alarm 300; exec @ARGV' $R record-dyn $L/t0/forkfail_dyn -o /private/tmp/claude-501/m47-m3b-ff.bin > $O/forkfail.out 2> $O/forkfail.err; echo "forkfail record rc=$?"
unset RETRACE_TRACE
echo "--- forkfail stdout"; cat $O/forkfail.out
D=/private/tmp/claude-501/m47-m3b-repo
rm -rf $D; mkdir -p $D
env -i $G -C $D init -q -b main; print one > $D/a.txt; env -i $G -C $D add a.txt; echo "setup=$?"
export RETRACE_TRACE=1
perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o /private/tmp/claude-501/m47-m3b-git.bin -- -C $D -c user.name=retrace -c user.email=retrace@example.invalid commit -q -m first > $O/git-commit.out 2> $O/git-commit.err; echo "git commit record rc=$?"
unset RETRACE_TRACE
echo "--- git stdout"; cat $O/git-commit.out
echo "--- git: guest stderr lines (console echo) and scratch/refusal/error lines"
grep -a -E '^\[fd2|cannot fork|m47-scratch|RECORD ERROR|panicked at|M33:|guest terminated|^\[retrace\] fall' $O/git-commit.err | cut -c1-200
echo "--- git log -1 in the recorded repo"; env -i $G -C $D log -1 --format=%s
unset PROBE_NOREUSABLE
for f in forkfail git-commit; do
  echo "=== $f: every trap from the 3403 through exit"
  awk '/msgh_id=3403/ {on=1} on && /^\[trap\]|^\[mach_msg2\]|m47-scratch|RECORD ERROR|panicked at|M33:|^\[fd[0-9]/ {print}' $O/$f.err | cut -c1-180
  echo "=== $f: trap numbers from the 3403 through exit, in order"
  awk '/msgh_id=3403/ {on=1} on && /^\[trap\]/ {sub(/.*num=/, ""); sub(/ .*/, ""); printf "%s ", $0} END {print ""}' $O/$f.err
done
