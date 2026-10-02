#!/bin/bash
# M47 Task 6: the shells' native outcome, three ways, stdin /dev/null as the sweep runs them:
# with this host's environment (what t6-step2.sh's native run used), with an EMPTY environment
# (what retrace gives every guest: load_dynamic pushes no envp), and with `-f` (no rc files).
# This host's ~/.tcshrc (read by both shells) holds Docker Desktop's sh-syntax line
# `export PATH="$PATH:…"`, which is what prints `Bad : modifier in $ '/'.` natively.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/e8ce6f2f-a207-4e1f-9f44-7ab16e768127/scratchpad
for s in csh tcsh; do
  /bin/$s < /dev/null > /dev/null 2> $S/t6-$s.err; echo "$s host-env exit=$? stderr=$(cat $S/t6-$s.err)"
  env -i /bin/$s < /dev/null > /dev/null 2> $S/t6-$s.err; echo "$s empty-env exit=$? stderr=$(cat $S/t6-$s.err)"
  /bin/$s -f < /dev/null > /dev/null 2> $S/t6-$s.err; echo "$s -f exit=$? stderr=$(cat $S/t6-$s.err)"
  rm -f $S/t6-$s.err
done
