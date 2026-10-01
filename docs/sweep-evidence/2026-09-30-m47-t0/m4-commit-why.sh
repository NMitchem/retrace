#!/bin/zsh
# M47 t0 M4 follow-up: `commit` recorded rc 1 ("nothing added to commit") while its repo's state
# matched native's. Inspect the recorded twin (w-commit) and the native twin (n-commit).
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
O=/Users/noahmitchem/Documents/GitHub/retrace/.claude/worktrees/m47-gitwrite/.superpowers/sdd/2026-09-30-retrace-m47-gitwrite/t0/m4
for r in n-commit w-commit; do
  echo "== $r: log (hash, subject, author date, committer date)"
  env -i $G -C $O/$r log --all --format='%H %s | %ad | %cd' --date=iso
  echo "== $r: reflog"
  env -i $G -C $O/$r reflog --format='%h %gs | %gd' --date=iso
  echo "== $r: object files (mtime)"
  find $O/$r/.git/objects -type f -not -path '*/info/*' -not -path '*/pack/*' -exec stat -f '%Sm %N' -t '%H:%M:%S' {} \; | sort
done
