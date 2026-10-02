#!/bin/bash
# M47 Task 6: what csh/tcsh do natively when fork fails. retrace gives every guest an EMPTY
# environment, and refuses fork with EAGAIN (35). The native analogue: an empty environment
# (`env -i`) and a soft RLIMIT_NPROC of 1 (`ulimit -u 1`, below this uid's process count, so every
# fork fails with EAGAIN), stdin /dev/null as the sweep runs them. Each shell is exec'd in place
# (bash `exec`, then env's own exec), so the limit is in force before the shell starts and nothing
# before it needs to fork. The unlimited empty-environment run beside it is the control.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
echo "uid processes now: $(ps -U $(id -u) -o pid= | wc -l | tr -d ' ')"
# Positive control: under the same limit, a shell that must fork to run a command reports the failure.
bash -c "ulimit -u 1 && exec env -i /bin/sh -c '/usr/bin/true; echo true-rc=\$?'" < /dev/null > $S/t6-ctl.out 2> $S/t6-ctl.err
echo "control sh fork-failing exit=$? stdout=[$(cat $S/t6-ctl.out)] stderr=[$(cat $S/t6-ctl.err)]"
rm -f $S/t6-ctl.out $S/t6-ctl.err
for s in csh tcsh; do
  bash -c "exec env -i /bin/$s" < /dev/null > $S/t6-$s.out 2> $S/t6-$s.err
  echo "$s empty-env exit=$? stdout=[$(cat $S/t6-$s.out)] stderr=[$(cat $S/t6-$s.err)]"
  bash -c "ulimit -u 1 && exec env -i /bin/$s" < /dev/null > $S/t6-$s.out 2> $S/t6-$s.err
  echo "$s empty-env fork-failing exit=$? stdout=[$(cat $S/t6-$s.out)] stderr=[$(cat $S/t6-$s.err)]"
  rm -f $S/t6-$s.out $S/t6-$s.err
done
