#!/bin/zsh
# M47 t0 M4 follow-up: `stash` stopped at 3403, i.e. at a fork. Which child? Re-run it on the m3b
# scratch build (3403 answered, fork refused EAGAIN) in m4.sh's repo shape and read git's own
# "cannot fork() for <child>" line; and natively, list the subcommands stash runs (GIT_TRACE).
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
L=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite
ID=(-c user.name=retrace -c user.email=retrace@example.invalid)
mk() {
  local d=$1; rm -rf $d; mkdir -p $d
  env -i $G -C $d init -q -b main
  print one > $d/a.txt; env -i $G -C $d add a.txt
  env -i $G -C $d $ID -c maintenance.auto=false commit -q -m first
  print 'one\ntwo' > $d/a.txt; print untracked > $d/b.txt; print added > $d/c.txt
}
d=/private/tmp/claude-501/m47-stash-native; mk $d
echo "== native, GIT_TRACE=1 (the child processes stash starts)"
env -i GIT_TRACE=1 $G -C $d $ID stash -q 2>&1 | grep -a -E 'run_command|start_command|built-in' | sed 's/^[0-9:.]* //'
d=/private/tmp/claude-501/m47-stash-m3b; mk $d
export PROBE_NOREUSABLE=1
perl -e 'alarm 300; exec @ARGV' /private/tmp/claude-501/m47-m3b-retrace record-dyn $G -o /private/tmp/claude-501/m47-stash.bin -- -C $d $ID stash -q > $L/t0/m4-stash-m3b.out 2> $L/t0/m4-stash-m3b.err; echo "== m3b build: record rc=$?"
unset PROBE_NOREUSABLE
grep -a -E 'm47-scratch|cannot|error|fatal|RECORD|panicked|M33' $L/t0/m4-stash-m3b.err | cut -c1-200
echo "== stash list after"; env -i $G -C $d stash list
