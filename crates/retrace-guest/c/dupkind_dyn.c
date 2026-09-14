// M37 fix wave (final review C1). `dup` copies the slot's KIND: an alias of stdout made by
// `dup(1)` is still a console write, and the shell's save/restore-stdout idiom for `>`
// redirection (`saved = dup(1); dup2(file, 1); …; dup2(saved, 1)`) hands slot 1 its console kind
// back. The first half is the review's `dup_dyn.c` (a dup alias beside a dup2 alias), the second
// its `saverestore.c`; every string is distinct so each line of stdout names the write it came from.
//
// Before the fix `dup` bound its alias as a plain `Open` slot on both sides, so a write through
// it was forwarded to the host `dup` of retrace's own stdout — on the recorder's terminal, never
// in the trace, never on replay — and after `dup2(saved, 1)` slot 1 was `Open` too (the kind
// `dup2` faithfully copies), so EVERY later stdout write went the same way: record printed
// `via-dup` and `two` from the host, out of order, replay printed neither; rc 0/0; no divergence.
//
// Expected stdout (native == record == replay, bit for bit): a\nvia-dup\nvia-dup2\none\ntwo\n
#include <unistd.h>
#include <fcntl.h>

int main(void) {
    write(1, "a\n", 2);
    int d = dup(1);  write(d, "via-dup\n", 8);     /* alias made by dup: a console write */
    dup2(1, 17);     write(17, "via-dup2\n", 9);   /* alias made by dup2: a console write (M37 t2) */
    /* the shell's `cmd > file` idiom: save stdout, redirect it, restore it */
    write(1, "one\n", 4);
    int saved = dup(1);
    int f = open("/dev/null", O_WRONLY); dup2(f, 1); close(f);
    write(1, "to-devnull\n", 11);                  /* slot 1 is the file now: not on stdout */
    dup2(saved, 1); close(saved);
    write(1, "two\n", 4);                          /* slot 1 is the console again */
    return 0;
}
