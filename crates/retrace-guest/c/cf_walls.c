// M50 (spec §3f): one guest for every newcomer-reachable wall cleanfail_e2e pins. argv[1] picks the
// mode. Each mode prints "cf: <mode>" and flushes BEFORE it reaches its wall, so a test can assert
// that the output a program printed before a wall is printed ahead of retrace's line. Natively,
// `pause` and `deadlock` hang forever, `amx` runs on, and `unblock` dies by a signal;
// cleanfail_e2e never runs this natively. The "returned" line is what a mode prints if retrace ever
// lets it past its wall.
#include <mach/clock.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t never = PTHREAD_COND_INITIALIZER;

static void *chroot_worker(void *arg) { (void)arg; chroot("/nonexistent-cf"); return NULL; }

// `deadlock`: waits on a condition nothing signals, while main joins it. Natively a hang.
static void *forever(void *arg) {
    (void)arg;
    pthread_mutex_lock(&lock);
    for (;;) pthread_cond_wait(&never, &lock);
    return NULL;
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "";
    printf("cf: %s\n", mode);
    fflush(stdout);
    if (!strcmp(mode, "chroot")) {
        chroot("/nonexistent-cf");
    } else if (!strcmp(mode, "threadchroot")) {
        pthread_t t;
        pthread_create(&t, NULL, chroot_worker, NULL);
        pthread_join(t, NULL);
    } else if (!strcmp(mode, "pause")) {
        pause();
    } else if (!strcmp(mode, "socketpair")) {
        int sv[2];
        socketpair(AF_UNIX, SOCK_STREAM, 0, sv);
    } else if (!strcmp(mode, "clocksleep")) {
        clock_serv_t clk;
        host_get_clock_service(mach_host_self(), SYSTEM_CLOCK, &clk);
        clock_sleep_trap(clk, TIME_RELATIVE, 0, 1000, NULL);
    } else if (!strcmp(mode, "waituntil")) {
        mach_wait_until(mach_absolute_time() + 1000);
    } else if (!strcmp(mode, "amx")) {
        // The x86_64 build is cleanfail_e2e's thin-x86_64 row, refused at load, so it only has to
        // compile: `.inst` is an ARM directive and clang rejects it for that target.
#if defined(__aarch64__)
        __asm__ volatile(".inst 0x00201220"); // AMX: native runs it, HVF does not (M51 R6)
#else
        __asm__ volatile("nop");
#endif
    } else if (!strcmp(mode, "iomain")) {
        mach_port_t port;
        host_get_io_main(mach_host_self(), &port);
    } else if (!strcmp(mode, "deadlock")) {
        pthread_t t;
        pthread_create(&t, NULL, forever, NULL);
        pthread_join(t, NULL);
    } else if (!strcmp(mode, "unblock")) {
        sigset_t s;
        sigemptyset(&s);
        sigaddset(&s, SIGUSR1);
        sigprocmask(SIG_BLOCK, &s, NULL);
        raise(SIGUSR1);
        sigprocmask(SIG_UNBLOCK, &s, NULL);
    } else {
        printf("cf: unknown mode\n");
        return 2;
    }
    printf("cf: %s returned\n", mode);
    return 0;
}
