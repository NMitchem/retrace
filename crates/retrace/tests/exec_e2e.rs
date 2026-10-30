// M54: the exec tests. Tasks 3-5 fill this file (execve, posix_spawn with POSIX_SPAWN_SETEXEC and
// the rest of the exec-in-place gates). M38's one test, which pinned both exec spellings as
// refused with EFAULT, was deleted by Task 2: a child posix_spawn is now a ProcessCreation wall
// (see cleanfail_e2e) and execve is replaced by Task 3.
