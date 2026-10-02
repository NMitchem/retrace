#!/bin/zsh
# Make a one-commit scratch repo for the abort reproduction (the logflake2 shape).
G=/Applications/Xcode.app/Contents/Developer/usr/bin/git
d=/private/tmp/claude-501/m47-abort/repo
rm -rf $d; mkdir -p $d
cd $d
env -i $G init -q -b main
print one > a.txt
env -i $G add a.txt
env -i $G -c user.name=retrace -c user.email=retrace@example.invalid commit -q -m first
env -i $G log -1 > /private/tmp/claude-501/m47-abort/native-log.out 2>&1; echo "native rc=$?"
cat /private/tmp/claude-501/m47-abort/native-log.out
