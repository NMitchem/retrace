#!/bin/zsh
# Static lldb over Xcode's git (shared cache resolved by lldb). Args: lldb commands, one per arg.
X=/Applications/Xcode.app/Contents/Developer/usr/bin/git
args=()
for a in "$@"; do args+=(-o "$a"); done
perl -e 'alarm 120; exec @ARGV' lldb -b -o "target create $X" $args 2>&1
