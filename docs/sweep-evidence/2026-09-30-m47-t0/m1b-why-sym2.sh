#!/bin/zsh
# M47 t0 M1(b) follow-up: the one cache frame /usr/bin/true's image list cannot resolve (libz is not
# among its dependencies); resolved against a static target of git itself, which links libz.
X=/Applications/Xcode.app/Contents/Developer/usr/bin/git
perl -e 'alarm 90; exec @ARGV' lldb -b -o "target create $X" -o "image lookup -a 0x19028d918" 2>&1 | grep -a -E 'lookup|Address|Summary'
