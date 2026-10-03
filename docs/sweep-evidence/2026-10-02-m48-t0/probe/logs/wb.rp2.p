[probe] kevent#1 tid=0 kq=7 change ident=0x1e7e7711 filter=-10 flags=0x21 fflags=0x0 data=0x0 udata=0x0 nev=1 timeout=Some((0, 0))
[probe] kevent#1 tid=0 kq=7 change ident=0x1e7e7711 filter=-10 flags=0x0 fflags=0x1000000 data=0x0 udata=0x0 nev=1 timeout=Some((0, 0))
[probe] kevent deliver kq=7 n=1
[probe] kevent#2 tid=0 kq=4 change ident=0x7 filter=-10 flags=0x21 fflags=0x0 data=0x0 udata=0x0 nev=0 timeout=None
[probe] kevent#3 tid=1 kq=8 change ident=0x9 filter=-10 flags=0x21 fflags=0x0 data=0x0 udata=0x0 nev=0 timeout=None
[probe] kevent#4 BLOCK tid=1 kq=8 timeout=None deadline=None now=0x71e4da2bd0a
[probe] psynch#1 cvwait tid=0 cv=0x27ff458 lsgen=0x100000100 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#2 cvsignal tid=2 cv=0x27ff458 lsgen=0x100 ugen=0x0 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] psynch#3 cvwait tid=2 cv=0x7bb48 lsgen=0x100000100 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#4 cvwait tid=0 cv=0x27ff458 lsgen=0x10000000200 ugen=0x100 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#5 cvsignal tid=3 cv=0x27ff458 lsgen=0x10000000200 ugen=0x100 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] psynch#6 cvwait tid=3 cv=0x7bb48 lsgen=0x200 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#7 cvwait tid=0 cv=0x27ff458 lsgen=0x20000000300 ugen=0x200 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#8 cvsignal tid=4 cv=0x27ff458 lsgen=0x20000000300 ugen=0x200 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] psynch#9 cvwait tid=4 cv=0x7bb48 lsgen=0x300 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#10 cvwait tid=0 cv=0x27ff458 lsgen=0x30000000400 ugen=0x300 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#11 cvsignal tid=5 cv=0x27ff458 lsgen=0x30000000400 ugen=0x300 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] psynch#12 cvwait tid=5 cv=0x7bb48 lsgen=0x400 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#13 cvwait tid=0 cv=0x27ff458 lsgen=0x40000000500 ugen=0x400 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=1
[probe] cvwait timeout tid=0 cv=0x27ff458
[probe] kevent#5 tid=0 kq=10 change ident=0xb filter=-10 flags=0x21 fflags=0x0 data=0x0 udata=0x0 nev=0 timeout=None
[probe] MAP_JIT mmap ipa=0xa2d068000 len=0x10000000 prot=0x0 flags=0x41842
[probe] mprotect over MAP_JIT ipa=0xa2d080000 len=0xffc0000 prot=0x7
[probe] psynch#14 cvsignal tid=0 cv=0x7bb48 lsgen=0x400 ugen=0x0 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] psynch#15 cvsignal tid=0 cv=0x7bb48 lsgen=0x10000000400 ugen=0x100 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] kevent#6 tid=0 kq=10 change ident=0xb filter=-10 flags=0x0 fflags=0x1000000 data=0x0 udata=0x0 nev=0 timeout=None
[probe] psynch#16 cvsignal tid=0 cv=0x7bb48 lsgen=0x20000000400 ugen=0x200 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] psynch#17 cvsignal tid=0 cv=0x7bb48 lsgen=0x30000000400 ugen=0x300 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] kevent#7 tid=0 kq=13 change ident=0x1 filter=-1 flags=0x5 fflags=0x0 data=0x0 udata=0x0 nev=1 timeout=Some((0, 1))
[probe] kevent#7 BLOCK tid=0 kq=13 timeout=Some((0, 1)) deadline=Some(7826765456650) now=0x71e4f93250a
[probe] kevent timeout tid=0
[probe] psynch#18 cvwait tid=0 cv=0x7bb78 lsgen=0x100000100 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#19 cvbroad tid=2 cv=0x7bb78 lsgen=0x100 udgen=0x100 flags=0x0 mutex=0x0 mugen=0x0 waiters=1
[probe] psynch#20 cvwait tid=2 cv=0x7bb48 lsgen=0x40000000500 ugen=0x400 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=0
[probe] psynch#21 cvsignal tid=0 cv=0x7bb48 lsgen=0x40000000500 ugen=0x400 port=0x0 mutex=0x0 mugen=0x0 thr=0x0 flags=0x0
[probe] kevent deliver kq=10 n=1
[probe] munmap over MAP_JIT ipa=0xa2d068000 len=0x10000000
[probe] kevent#10 tid=0 kq=8 change ident=0x9 filter=-10 flags=0x0 fflags=0x1000000 data=0x0 udata=0x0 nev=0 timeout=None
[probe] kevent deliver kq=8 n=1
[probe] kevent wake tid=1 kq=8 n=1
[probe] psynch#22 cvwait tid=0 cv=0x7bcb0 lsgen=0x100000100 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=1
[probe] cvwait timeout tid=0 cv=0x7bcb0
[probe] psynch#23 cvwait tid=0 cv=0x7bc80 lsgen=0x100000100 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=1
[probe] cvwait timeout tid=0 cv=0x7bc80
[probe] psynch#24 cvwait tid=0 cv=0x7bb78 lsgen=0x10000000200 ugen=0x100 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=1
[probe] cvwait timeout tid=0 cv=0x7bb78
[probe] psynch#25 cvwait tid=0 cv=0x7bb48 lsgen=0x50000000600 ugen=0x500 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=1
[probe] cvwait timeout tid=0 cv=0x7bb48
[probe] psynch#26 cvwait tid=0 cv=0x7b8e8 lsgen=0x100000100 ugen=0x0 mutex=0x0 mugen=0x0 flags=0xa0 sec=0 nsec=1
[probe] cvwait timeout tid=0 cv=0x7b8e8
