# When retrace stops at a wall

retrace records a program by running it in a small virtual machine and answering every system call the program
makes. A **wall** is something the program does that retrace doesn't handle yet. At a wall, retrace prints the
program's output so far, then 1 line, and exits with status 4:

    retrace: unsupported: fsync (syscall 95) - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#syscalls

The line names what the program did and links the section below that explains it. The recording up to the wall
stays on disk, and `retrace replay` replays it to the same point (see
[Incomplete recordings](#incomplete-recordings)). To see every call the program made before the wall, run the same
`record` command again with `RETRACE_TRACE=1` set. That log is the most useful thing you can attach to a report.

A wall isn't a bug. It's a limit retrace documents, and retrace stops at it on purpose instead of guessing. A bug
looks different: retrace prints this line and exits with status 70.

    retrace: internal error - this is a bug in retrace, not in your program.

Please report it at https://github.com/NMitchem/retrace/issues, with the line it asks for.

| Exit status | Meaning |
|---|---|
| the program's own | The program ran to its end. A program killed by a signal, a crash included, gives 128 + the signal, as it does natively. |
| 2 | retrace couldn't start: a bad argument, a file it can't read or write, a file that isn't an arm64 Mach-O program, or a missing hypervisor entitlement. |
| 3 | `replay` found the program behaving differently from its recording. |
| 4 | A wall, on `record`. On `replay`: the end of a recording that stopped at a wall. |
| 5 | `debug` or `gdbserver` couldn't run, including on an incomplete recording. |
| 70 | A bug in retrace. |

Some calls retrace answers with an error the program can handle, instead of stopping at a wall. For those it
prints `retrace: refused: <call>, returned <error> to the program` and keeps recording. `fork` is refused this way.

A program that crashes or is killed by a signal ends with 1 line of retrace's own, after the program's output,
such as `retrace: the program was killed by SIGSEGV (fault at 0x0, pc 0x100003f2c in main+0x2c)`. Set
`RETRACE_VERBOSE=1` to see retrace's other diagnostic lines, such as the count of exceptions it handled for the
program. They're off by default, because they aren't the program's output.

## Syscalls

The program made a system call that retrace has no model of yet. The line names the call the way
the macOS SDK does, with its number, for example `fsync (syscall 95)`, or a Mach trap with its
negative number, for example `swtch_pri (mach trap -59)`. A program that waits in a call retrace
doesn't model may also stop here: `sched_yield`, `clock_sleep_trap` and `mach_wait_until` do.

The filesystem and process calls real programs make most (`unlinkat`, `symlink`, `ftruncate`, `fsync` and
their neighbours) and `readv`, `writev`, `preadv` and `pwritev` are modelled. These still stop here:

- A `flock` that would wait for a lock another descriptor or process holds (`flock (syscall 131) on fd N
  would wait for a lock another descriptor or process holds`).
- A `readv` or `writev` on a guest pipe whose later iovec entry is unreachable (`... with an unreachable
  entry after the first, on guest pipe fd N`). That's measured natively for `readv`, and a `writev` is refused
  the same way. So is an iovec array that splits into more than 1024 host entries.
- A length argument larger than the memory behind its buffer, for example a `sysctl` `*oldlenp` or a
  `stat64_extended` `*xsecurity_size` past its buffer's backing (`syscall 341 was given a length of …`).
  Natively the call may succeed, because the kernel writes only what it needs, but retrace can't size the
  memory it must watch. Pass the buffer's real size.
- `chroot`, which is privileged and stays out of scope for good.

`socketpair` of a Unix-domain stream pair runs, which is what tokio's runtime needs, and the pair
is a guest object like a pipe: `poll`, `select` and the kqueue filters (with `EV_CLEAR` and `EV_RECEIPT`) see
it. What still stops here, each line naming the call and its descriptor:

- A `socketpair` of another type or domain (`socketpair of SOCK_DGRAM: only a stream pair is modelled`,
  or `socketpair of domain … succeeded on the host and is not modelled`). Natively a datagram pair works,
  but retrace has no model of datagram boundaries.
- On a guest socket end, `shutdown`, `sendmsg` (any: its msghdr iovec isn't translated, and with control data an `SCM_RIGHTS` descriptor would also carry a
  host number), `sendmsg_x`, and `setsockopt` of `SO_SNDBUF`, `SO_RCVBUF`, `SO_SNDLOWAT` or `SO_RCVLOWAT`,
  because the model's buffer constants would go stale (`… on guest socket fd N is not modelled`).
- `fcntl(F_SETFL)` on a guest pipe or socket end with a status flag beyond `O_NONBLOCK`, `O_APPEND` and
  `O_ASYNC` (`fcntl(F_SETFL, 0x…) on guest fd N: a status flag … is not modelled`).
- `bind`, `listen` and `accept`, which have no row (`bind (syscall 104)`). `socket` and `connect` are forwarded, but a
  descriptor they make isn't a guest end, so waiting on it stops under [Threads](#threads). tokio's TCP and Unix
  listener tests stop at `bind`.

Network sockets and datagram pairs aren't modelled yet.

## Mach messages

The program sent a Mach message, a request to the kernel or a system service through a port, that
retrace doesn't answer yet. The line names the message's id (`msgh_id`). Programs that talk to
system services, such as I/O Kit or an XPC service, reach this.

## Process creation

The program tried to wait for another process, with `wait4` or `waitid`, or to start one. retrace
records one process, so a `posix_spawn` that starts a child (one without `POSIX_SPAWN_SETEXEC`, which
`system()` and `popen()` reach through libc) ends the recording here, with
`retrace: unsupported: posix_spawn of a child process (syscall 244)`. That's retrace's limit, and the same
call can succeed natively. `fork` returns `EAGAIN`, and what the program does next with that error is up
to it. A shell that then waits for its child stops at `wait4`.

A program may replace its own image: `execve`, a `posix_spawn` with `POSIX_SPAWN_SETEXEC` and a
`#!` script (one level) run in place, recorded and replayed. These shapes of exec stop here, each
naming itself in the line:

- An x86_64-only image, which macOS runs under Rosetta, and a universal binary with no arm64 slice.
- A static image (one with no dynamic linker), a setuid or setgid file, and an image whose segments
  overlap retrace's own low memory, the commpage or the shared region, aren't 16 KiB aligned or end past
  2^36 bytes.
- An image whose load commands can't be walked (a command that's too short or runs past the end of the
  file). Natively the kernel accepts such an `execve` and then kills the process with `SIGKILL` (exit 137),
  so there's no error to answer. A file too short to hold a Mach-O header, or without the magic, answers
  `ENOEXEC`, as natively. A set-id `#!` script is refused under the setuid/setgid rule above, although
  macOS by default runs one without the privileges.
- Arguments and environment together past retrace's 128 KiB start-stack limit (past 1 MiB the call returns
  `E2BIG`, as natively).
- A `posix_spawn` attribute or extension retrace doesn't model, named by value: a flag other than
  `SETEXEC`, `SETSIGMASK`, `SETSIGDEF` or `CLOEXEC_DEFAULT`, a binary preference with no arm64 entry, a
  non-default priority, memory-limit, QoS or jetsam field, port actions, MAC, coalition, persona or
  credential extensions, a subsystem root, a conclave id, a fileport `dup2` action, and an `addinherit_np`
  of a guest kqueue. macOS accepts all of these, so the wall is retrace's limit.
- A `posix_spawn` with `chdir` file actions from a working directory retrace can't save (one the
  program made unreadable or deleted): the line reads `the working directory couldn't be saved for a chdir
  file action`.
- `__mac_execve`.

A dylib, bundle or object file given to exec, a nested `#!`, a missing file and a bad pointer return native's
errno instead. They aren't walls. A signal the new image would be stopped by stops the recording under
[Signals](#signals), because stopping a process isn't modelled.

With Xcode selected, `/usr/bin/git`, `/usr/bin/python3` and the other xcrun shims run
`xcodebuild -license check` as a child first when xcrun's cache is cold, which ends at this wall. With a
warm cache they exec the real tool in place. Run the tool itself (`$(xcrun -f git)`) or select the Command
Line Tools (`DEVELOPER_DIR=/Library/Developer/CommandLineTools`).

## Signals

The program used a signal in a way retrace doesn't model yet. These reach it today:

- A signal raised while blocked, whose default action ends the process when it's unblocked (a signal that is
  not blocked, with such an action, ends the process at the raise, whatever its target is doing).
- A handled signal sent to a thread blocked in a condition variable (`psynch_cvwait`), a mutex
  (`psynch_mutexwait`), `poll`, `select` or `pselect`, or a parked workqueue thread (`a handled signal
  (SIGUSR1) to thread N, blocked in psynch_cvwait: delivery there isn't modelled`). Natively the signal is
  delivered at once and the wait returns early. The kernel's return for a condition variable or mutex carries
  an effect on its sequence word that retrace doesn't reproduce. A multi-thread tokio program that
  signals itself while `main` waits in `block_on` reaches this: a process-directed signal goes to main first.
- A hardware fault on a thread that has its signal blocked, which the program didn't survive natively either.
- `kill` with a signal other than 0 to another process.
- `terminate_with_payload`, which ends a process by its pid.
- `setitimer` or `getitimer` on `ITIMER_VIRTUAL` or `ITIMER_PROF`, which count CPU time the synthetic clock doesn't have (`only ITIMER_REAL is modelled`).
- A signal sent to the process (`kill(getpid(), …)`, a timer's `SIGALRM`) when the main thread has exited and every other thread masks it.
- A stop signal (`SIGSTOP`, `SIGTSTP`, `SIGTTIN`, `SIGTTOU`) pending at an exec that would stop the new image (`… pending at exec, which stops the new image: stopping a process is not modelled`).
- `sigreturn` with a null context and `UC_SET_ALT_STACK` or `UC_RESET_ALT_STACK`, which libplatform's `longjmp` issues after a refused `fork` (`csh`, `tcsh`, `flex`). The line says the sigreturn token doesn't match, which misdescribes a legitimate call.
- A signal blocked, raised and then unblocked at its default action, which ends the recording where native exits 128 plus the signal.

`pause()` and `sigsuspend()` wait for a signal and return `EINTR` after its handler, `ITIMER_REAL` (so
`alarm`) runs on the synthetic clock, a signal ends a `sleep` early, and `kill` with a signal number outside 0 to 31
returns `EINVAL`, as natively. A `pause()` that nothing can ever signal is the deadlock case under
[Threads](#threads).

A handler installed with `signal()`, `kill(pid, 0)` and `pthread_kill(thread, 0)`, and
`abort_with_payload` (which dyld calls when a library is missing) run as they do natively.

## Threads

The program's threads reached a state retrace can't schedule, or a wait retrace doesn't model.
A contended `pthread_mutex` (`psynch_mutexwait`), `semaphore_timedwait_trap` (-38, which
Rust's test harness uses), `swtch_pri` (-59) and `thread_switch` (-61) are modelled. What still stops
here: a contended `os_unfair_lock` or any `__ulock_wait2` (544), the mach semaphore traps
`semaphore_signal_all_trap` (-34), `semaphore_signal_thread_trap` (-35), `semaphore_wait_signal_trap`
(-37) and `semaphore_timedwait_signal_trap` (-39), a wait on a semaphore that was destroyed, a
`pthread_mutex` with the fairshare policy, a condition-variable wait that carries a mutex, a
`thread_switch` with an option other than none, depress or wait, and a `proc_pidinfo` thread lookup
by thread id.

A duplicated kqueue is one kqueue, and `poll`, `select` and `pselect` over a program's own pipes and socket
pairs are modelled. What stops here: `poll`, `select` or `pselect` on a
descriptor that isn't a guest pipe or socket end (a file, a terminal), `pselect` with a signal mask, a write end in a `select` read set or a read end in a write
set, a `poll` over 4096 entries, closing a descriptor a blocked `poll` or `select` watches, closing the kqueue
descriptor a `kevent` waiter blocks on, and an `EVFILT_USER` add that carries both `EV_DISABLE` and
`NOTE_TRIGGER`. Each line names the call and its descriptor. A program that reaches `bind`
or a socket other than a stream pair stops under [Syscalls](#syscalls).

A read or write that would wait for another thread stops here.
A blocking `read` on an empty guest pipe or socket end whose other side is still open, and a blocking write
larger than the free room (65536 bytes for a pipe from empty, 8192 per direction for a socket), end with
`read on guest pipe fd N would wait for another thread to write` (or `write of N bytes on guest socket fd
M would wait for another thread to read`, `… blocking I/O between threads isn't modelled`). Natively the
call blocks until another thread feeds or drains the end and then completes. Setting `O_NONBLOCK` first (tokio
and mio do) avoids it. Two more walls land here: a blocking `recv` with `MSG_WAITALL` for more bytes than are held, on a
guest socket end whose peer is open (`… with MSG_WAITALL would wait for N more bytes …`), and a guest end whose byte count
the model lost, for example after a read that returned more than it counted (`socket N: the byte count is unknown`). Lifting this needs a block-and-retry model, which would make the
blocked thread runnable when the call can complete.

A handled signal that arrives while a thread waits in `poll`, a condition variable or a mutex stops the
recording here too (see [Signals](#signals)). A signal that ends a `sleep`, a `sigsuspend`, a semaphore wait
(`x0` 14, called again) or a `kevent` (`EINTR`) doesn't: it runs its handler and the call returns
as natively. A signal that's pending at any other wake
is listed under [Signals](#signals).

One case is special: every thread is blocked and nothing is left to wake any of them. Natively that
program would hang forever, so the line says so. It also names a thread blocked in `sigsuspend`
(`thread N waits in sigsuspend for a signal nothing can send`), which is what a `pause()` with no timer and no
sender is. If the same program doesn't hang outside
retrace, that's a bug in retrace: please report it. A handled signal to a thread blocked in `poll`, a condition variable or a mutex is refused
when it's raised (see [Signals](#signals)).

A program that spins forever waiting for another thread, without blocking, isn't a wall: it hangs
under retrace, where natively the other thread would run on another core. retrace can't tell a spin from slow work by anything in the
recording, and reading the wall clock into it would make recordings differ from run to run. The
line below is the one thing it does with the clock.

The recorder does read the host clock for one thing, a single stderr line. A thread that
runs for 20 seconds of real time without blocking, while other threads are ready to run, prints
`retrace: thread N has run for 20 s without blocking while thread(s) … are ready to run`, once per
time that thread was picked to run. The line is control-plane only: it's in no recording, replay
never prints it, and the recording goes on. It means the program is probably waiting for one of
those threads, which retrace won't run until this one blocks. The fix is preemption, which is
not planned.

## Memory

The program asked for a kind of memory mapping retrace doesn't support yet, such as an unmodelled
use of `MAP_JIT` memory, or a write to the per-thread JIT protection register with a value retrace
doesn't admit. Other memory walls are a `vm_remap` or `mach_vm_map` option retrace doesn't model, a
fixed mapping that straddles an existing one. A bad pointer the program passes to a call gets the error it gets natively, usually EFAULT, and the recording goes on.

malloc's reclaim ring is the other source. macOS's malloc queues freed large blocks in a ring the kernel
empties, and retrace empties it when the program asks. A ring macOS would kill the program over stops the
recording here instead: unreadable indices or entries, more entries than the ring holds, an entry that
frees memory that isn't all mapped, or an entry of a kind macOS doesn't define.

## Exceptions

The program ran an instruction retrace can't run or record, such as one of Apple's AMX matrix
instructions, or read a system register retrace doesn't model. The line names the instruction, its
address and its exception class.

The traps a program uses to stop itself are recorded as crashes with native's signal and
exit status: `brk` (Swift's and Rust's runtime checks, SIGTRAP, 133), `udf` and `hlt` (SIGILL, 132),
and a misaligned PC or stack pointer (SIGBUS, 138).

## macOS version

retrace runs on macOS 26 and 27, at any point release. 27.2 and 26.5.2 are the measured builds, and other point releases are untested. On another major every command ends at one line, `macOS <product> (<build>) is not supported: retrace runs on macOS 26 and 27`, with exit 4, before any VM exists.

Every recording of a dynamic program carries the macOS version, build and shared-cache UUID of the Mac that made it, and **such a recording replays only on the macOS build that made it**. A static recording carries no UUID and isn't pinned. Replaying one on a Mac with a different shared cache is refused with `<trace> was recorded on macOS <product> (<build>) with a different shared cache`, exit 2. A recording made by an earlier retrace is refused by its format, as before.

retrace reads the dyld shared cache's own header to find its files, and checks each file against the subcache table there. A cache laid out in a way retrace hasn't met is refused, exit 4, with `has a shared cache retrace does not recognise: <why>`, naming the host's macOS version and build, rather than mis-read.

The guest's physical address space is 40 bits (1 TiB), because macOS 27's malloc reserves 24 GiB at a random address above 448 GiB. A Mac whose hypervisor allows fewer than 40 bits (`hv_vm_config_get_max_ipa_size`) is refused before any VM is made, exit 4, with `this Mac's hypervisor allows a <n>-bit guest address space and retrace needs 40 bits`. A guest that maps FIXED at or past 1 TiB still gets the wall or `EINVAL` it got at 64 GiB before.

## Incomplete recordings

A recording that stopped at a wall, or whose recorder was killed, has no end: no exit, crash or
fatal signal. `retrace replay` runs it to where it stops, prints the program's output up to there,
then one line, and exits with status 4:

    retrace: incomplete recording: it ends at landmark 342 (pc 0x1804b5478) - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#incomplete-recordings

The line names the wall when replay meets it itself, in code the recording and the replay share, such as:

    retrace: incomplete recording: it ends at semaphore_signal_trap (-33) on port 0x1003 found 2 waiters parked ([1, 2]), but -33 wakes exactly ONE. ... - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#incomplete-recordings

Otherwise it names the last landmark it replayed and that landmark's address, as it does for a
recording whose recorder was killed. A call that stops at the first form is `fsync`: the recording
ended before it, so replay has nothing to meet. A difference before that point is still a divergence, with exit status 3.

A recording whose tail was torn, because the recorder was killed in the middle of a write or the
disk filled up, is the same case: retrace drops the torn tail, the recording then has no terminal
event, and replay ends at its last whole landmark with this line.

`retrace debug` and `retrace gdbserver` refuse an incomplete recording before they start, with
exit status 5. Debugging up to a wall isn't supported yet.
