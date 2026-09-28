#!/bin/sh
# M44 t0 M4: pair every SDK SYS_*_nocancel with its plain twin (by name, from the same header).
# Output: <nocancel_name> <nocancel_num> <plain_name> <plain_num|NONE>
H=$(xcrun --show-sdk-path)/usr/include/sys/syscall.h
while read -r nc ncnum; do
    plain=${nc%_nocancel}
    pnum=$(grep -E "^#define[[:space:]]+${plain}[[:space:]]" "$H" | awk '{print $3}')
    echo "$nc $ncnum $plain ${pnum:-NONE}"
done < "$1"
