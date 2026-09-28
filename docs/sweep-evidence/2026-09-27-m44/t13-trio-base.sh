#!/bin/bash
# M44 Task 13 control: with xcrun's cache WARM (the full sweep's condition), does the trio's move
# need M44 at all? Run the trio on the M44 BASE binary (ebd0266: no 464/128 rows) and on the swept
# binary with RETRACE_TRACE=1, and count 464 / 128 / 244 traps. The cache file is left untouched.
S=/private/tmp/claude-501/-Users-noahmitchem-Documents-GitHub-retrace/1996e432-75ea-4d65-a52b-ed70d24a05b8/scratchpad
cd $S || exit 2
ls -la /var/tmp/xcrun_db
export RETRACE_TRACE=1
for v in base t13; do
  for b in desdp dyld_info flex; do
    $S/retrace-$v record-dyn /usr/bin/$b -o $S/t13w.bin < /dev/null > /dev/null 2> $S/t13w-$v-$b.err
    rc=$?
    echo "$v $b rc=$rc 464=$(grep -a -c 'num=464 ' $S/t13w-$v-$b.err) 128=$(grep -a -c 'num=128 ' $S/t13w-$v-$b.err) 244=$(grep -a -c 'num=244 ' $S/t13w-$v-$b.err) panic=$(grep -a -c 'panicked' $S/t13w-$v-$b.err)"
    rm -f $S/t13w.bin
  done
done
ls -la /var/tmp/xcrun_db
