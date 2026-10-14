# When retrace stops at a wall

retrace records a program by running it in a small virtual machine and answering every system
call the program makes. A **wall** is something the program does that retrace does not handle yet.
At a wall, retrace prints the program's output so far, then one line, and exits with status 4:

    retrace: unsupported: <what the program did> - see https://github.com/NMitchem/retrace/blob/main/docs/walls.md#<section>

The recording up to that point stays on disk. To see every call the program made before the wall,
run the same `record` command again with `RETRACE_TRACE=1` set.

If retrace prints `retrace: internal error` instead, that is a bug in retrace, not a wall. Please
report it at https://github.com/NMitchem/retrace/issues with the line it asks for.

## Syscalls

The program made a system call retrace has no model of yet. The line names the call and its number.

## Mach messages

The program sent a Mach message, a kernel request through a port, that retrace does not answer yet.
The line names the message's id.

## Process creation

The program tried to start or wait for another process. retrace records one process.

## Signals

The program used a signal in a way retrace does not model yet.

## Threads

The program's threads reached a state retrace cannot schedule.

## Memory

The program asked for a kind of memory mapping retrace does not support yet.

## Exceptions

The program ran an instruction that traps, such as a `brk` that Swift and Rust use to stop on a
fatal error, and retrace does not record that trap as a crash yet.

## Incomplete recordings

A recording that stopped at a wall, or whose recorder was killed, has no end. `retrace replay` runs
it to where it stops and says so, with exit 4. `retrace debug` and `retrace gdbserver` refuse it.
