# When retrace stops at a wall

retrace records a program by running it in a small virtual machine and answering every system
call the program makes. A **wall** is something the program does that retrace does not handle yet.
At a wall, retrace prints the program's output so far, then one line, and exits with status 4:

    retrace: unsupported: fsync (syscall 95) - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#syscalls

The line names what the program did and links the section below that explains it. The recording up
to the wall stays on disk, and `retrace replay` replays it to the same point (see
[Incomplete recordings](#incomplete-recordings)). To see every call the program made before the
wall, run the same `record` command again with `RETRACE_TRACE=1` set; that log is the most useful
thing to attach to a report.

A wall is not a bug: it is a limit retrace documents. If retrace prints this instead, it is a bug in
retrace, not in your program, and it exits with status 70:

    retrace: internal error - this is a bug in retrace, not in your program.

Please report it at https://github.com/NMitchem/retrace/issues with the line it asks for.

| Exit status | Meaning |
|---|---|
| the program's own | the program ran to its end; a program killed by a signal gives 128 + the signal |
| 2 | retrace could not start: a bad argument, a file it cannot read or write, a file that is not an arm64 Mach-O program, or a missing hypervisor entitlement |
| 3 | `replay` found the program behaving differently from its recording |
| 4 | a wall, on `record`; or, on `replay`, the end of a recording that stopped at a wall |
| 5 | `debug` or `gdbserver` could not run, including on an incomplete recording |
| 70 | a bug in retrace |

Some calls retrace answers with an error the program can handle, rather than stopping at a wall.
For those it prints `retrace: refused: <call>, returned <error> to the program` and keeps
recording. Starting another program with `posix_spawn` or `execve`, and `fork`, are refused this way.

## Syscalls

The program made a system call that retrace has no model of yet. The line names the call the way
the macOS SDK does, with its number, for example `fsync (syscall 95)`, or a Mach trap with its
negative number, for example `swtch_pri (mach trap -59)`. A program that waits in a call retrace
does not model may also stop here: `sched_yield`, `clock_sleep_trap` and `mach_wait_until` do.

Which calls have models grows milestone by milestone, ranked by what real programs call.

## Mach messages

The program sent a Mach message, a request to the kernel or a system service through a port, that
retrace does not answer yet. The line names the message's id (`msgh_id`). Programs that talk to
system services, such as I/O Kit or an XPC service, reach this.

## Process creation

The program tried to wait for another process, with `wait4` or `waitid`, or to start one in a way
retrace does not refuse. retrace records one process. A refused `fork`, `posix_spawn` or `execve`
returns an error to the program instead (see above); what the program does next with that error is
up to it, and a shell that then waits for its child stops here.

## Signals

The program used a signal in a way retrace does not model yet. These reach it today:

- a handler installed with `signal()`, which delivers without `SA_SIGINFO`;
- `kill(getpid(), 0)`, which asks whether the process exists;
- `pause()` or `sigsuspend()`, which wait for a signal;
- a signal raised while blocked, whose default action ends the process when it is unblocked;
- a hardware fault on a thread that has its signal blocked, such as a Rust stack overflow;
- `kill` to another process.

## Threads

The program's threads reached a state retrace cannot schedule, or a wait retrace does not model.
A contended `pthread_mutex` (`psynch_mutexwait`) is one, and so are the mach semaphore traps
retrace has no model of: `semaphore_signal_all_trap` (-34), `semaphore_signal_thread_trap` (-35),
`semaphore_wait_signal_trap` (-37), `semaphore_timedwait_trap` (-38), which Rust's test harness
uses, and `semaphore_timedwait_signal_trap` (-39). A `dup` of a kqueue the program made also stops
here.

A signal that arrives while a thread waits on a timer, in a timed sleep, a timed condition-variable
wait or a timed `kevent`, stops the recording here too. A signal that is pending at any other wake
is listed under [Signals](#signals).

One case is special: every thread is blocked and nothing is left to wake any of them. Natively that
program would hang forever, so the line says so. If the same program does not hang outside
retrace, that is a bug in retrace; please report it.

A program that spins forever waiting for another thread, without blocking, is not a wall: it hangs
under retrace as it would natively. retrace cannot tell a spin from slow work without reading the
wall clock, which would make recordings differ from run to run.

## Memory

The program asked for a kind of memory mapping retrace does not support yet, such as an unmodelled
use of `MAP_JIT` memory, or a write to the per-thread JIT protection register with a value retrace
does not admit.

## Exceptions

The program ran an instruction that traps, and retrace does not record that trap as a crash yet.
The commonest is `brk`, which Swift and Rust use to stop on a fatal error such as an array index out
of range. The line names the instruction, its address and its exception class.

## Incomplete recordings

A recording that stopped at a wall, or whose recorder was killed, has no end: no exit, crash or
fatal signal. `retrace replay` runs it to where it stops, prints the program's output up to there,
then one line, and exits with status 4:

    retrace: incomplete recording: it ends at fsync (syscall 95) - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#incomplete-recordings

The line names the wall when replay reaches it too, or the last landmark of a recording whose
recorder was killed. A difference before that point is still a divergence, with exit status 3.

A recording whose tail was torn, because the recorder was killed in the middle of a write or the
disk filled up, is the same case: retrace drops the torn tail, the recording then has no terminal
event, and replay ends at its last whole landmark with this line.

`retrace debug` and `retrace gdbserver` refuse an incomplete recording before they start, with
exit status 5. Debugging up to a wall is not supported yet.
