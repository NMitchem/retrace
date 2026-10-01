#!/bin/zsh
# M47 t0 M4: each candidate git command natively and under the census build. Reads run on ONE repo
# (native first, then recorded), so their stdout is comparable; writes run on twin repos and
# compare a state fingerprint of hash-free facts (trees, subjects, ref names, index, status).
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
R=/private/tmp/claude-501/m47-census-retrace
O=$1; mkdir -p $O; : > $O/nums.txt
ID=(-c user.name=retrace -c user.email=retrace@example.invalid)
mkrepo() {
  # t0 fix (run 1, m4-run1-invalid.txt): `local`, or this assignment overwrites write_cmd's $d with
  # the native twin's path, and every write command was recorded in the native repo after native ran.
  local d=$1; rm -rf $d; mkdir -p $d
  env -i $G -C $d init -q -b main
  print one > $d/a.txt; env -i $G -C $d add a.txt
  env -i $G -C $d $ID -c maintenance.auto=false commit -q -m first
  if [ "$2" = merge ]; then
    env -i $G -C $d switch -q -c topic; print t > $d/t.txt; env -i $G -C $d add t.txt
    env -i $G -C $d $ID -c maintenance.auto=false commit -q -m topic; env -i $G -C $d switch -q main
  fi
  print 'one\ntwo' > $d/a.txt; print untracked > $d/b.txt; print added > $d/c.txt
}
state() {
  for a in "status --porcelain" "ls-files --stage" "for-each-ref --format=%(refname)" "log --all --format=%T%x20%s" "stash list --format=%s" "symbolic-ref -q HEAD"; do
    env -i $G -C $1 ${=a}; done 2>&1
}
rec() {  # rec <tag> <repo> <git args...>
  tag=$1; d=$2; shift 2
  export RETRACE_TRACE=1
  perl -e 'alarm 300; exec @ARGV' $R record-dyn $G -o $O/$tag.bin -- -C $d "$@" > $O/$tag.rec.out 2> $O/$tag.rec.err; rrc=$?
  unset RETRACE_TRACE
  prc=n/a; same=n/a
  if [ -s $O/$tag.bin ] && ! grep -a -q -E 'panicked at|RECORD ERROR' $O/$tag.rec.err; then
    perl -e 'alarm 300; exec @ARGV' $R replay $O/$tag.bin > $O/$tag.rp.out 2> $O/$tag.rp.err; prc=$?
    cmp -s $O/$tag.rec.out $O/$tag.rp.out && same=yes || same=no
  fi
  grep -a -o '^\[trap\] num=[-0-9]*' $O/$tag.rec.err | sed 's/.*=//' >> $O/nums.txt
  wall=$(grep -a -m1 -E 'panicked at|RECORD ERROR|M33:' $O/$tag.rec.err | cut -c1-200)
}
read_cmd() {  # read_cmd <tag> <git args...>
  tag=$1; shift; d=$O/r-$tag; mkrepo $d
  env -i $G -C $d "$@" > $O/$tag.native.out 2> $O/$tag.native.err; nrc=$?
  rec $tag $d "$@"
  cmp -s $O/$tag.native.out $O/$tag.rec.out && nat=yes || nat=no
  echo "M4 READ $tag native_rc=$nrc rec_rc=$rrc rp_rc=$prc stdout_rec==native:$nat rp==rec:$same wall=$wall"
}
write_cmd() {  # write_cmd <tag> <prep> <git args...>
  tag=$1; prep=$2; shift 2; n=$O/n-$tag; d=$O/w-$tag; mkrepo $n $prep; mkrepo $d $prep
  env -i $G -C $n "$@" > $O/$tag.native.out 2> $O/$tag.native.err; nrc=$?
  rec $tag $d "$@"
  state $n > $O/$tag.native.state; state $d > $O/$tag.rec.state
  cmp -s $O/$tag.native.state $O/$tag.rec.state && st=yes || st=no
  echo "M4 WRITE $tag native_rc=$nrc rec_rc=$rrc rp_rc=$prc state_rec==native:$st rp==rec:$same wall=$wall"
}
read_cmd status-porcelain status --porcelain
read_cmd status status
read_cmd diff diff
read_cmd diff-cached diff --cached
read_cmd log log -1
read_cmd show show --stat HEAD
read_cmd rev-parse rev-parse HEAD
write_cmd add - add c.txt
write_cmd commit - $ID -c maintenance.auto=false commit -q -a -m second
write_cmd branch - branch topic
write_cmd tag - tag v1
write_cmd switch-c - switch -q -c topic
write_cmd mv - mv a.txt moved.txt
write_cmd rm - rm -q --cached a.txt
write_cmd stash - $ID stash -q
write_cmd merge merge $ID -c maintenance.auto=false merge -q --ff-only topic
sort -un $O/nums.txt -o $O/nums.txt
