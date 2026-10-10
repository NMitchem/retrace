// M49 first-contact corpus (spec §3b): a failing assert(). Natively SIGABRT (134) after libc prints
// the assertion to stderr; abort() raises the signal, so the trace's terminal event is
// `Event::Signal`, not `Event::Crash` (CLAUDE.md's raise/fault distinction). stdout is flushed
// before the assertion fires.
#include <assert.h>
#include <stdio.h>

int main(int argc, char **argv) {
    (void)argv;
    int answer = argc + 1; // 2 when run with no arguments
    puts("checking the answer");
    fflush(stdout);
    assert(answer == 42);
    puts("unreachable");
    return 0;
}
