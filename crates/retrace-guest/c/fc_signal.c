// M51 first-contact corpus (R8): a handler installed with signal(), the classic C idiom, so tier 1
// guards the class. Natively the handler runs on each raise and the program exits 0.
#include <signal.h>
#include <stdio.h>

static volatile sig_atomic_t hits;

static void on_usr1(int sig) { (void)sig; hits++; }

int main(void) {
    signal(SIGUSR1, on_usr1);
    raise(SIGUSR1);
    raise(SIGUSR1);
    printf("handled %d signals\n", (int)hits);
    return 0;
}
