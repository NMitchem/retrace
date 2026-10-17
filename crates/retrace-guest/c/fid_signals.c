// M51 (spec §3g): a signal() handler, signal 0 and abort_with_payload. usage: fid_signals <mode>
//   raise          signal(SIGUSR1, h), then raise(SIGUSR1) twice; the handler prints each time
//   fault          signal(SIGSEGV, h), then a store to 0x10; the handler prints and _exit(42)s
//   kill0self      kill(getpid(), 0)
//   kill00         kill(0, 0): the caller's process group
//   kill0init      kill(1, 0): EPERM for a process that may not signal launchd
//   kill0none      kill(0x7ffffff0, 0): ESRCH, no such process
//   pkill0self     pthread_kill(pthread_self(), 0)
//   pkill0exited   pthread_kill(t, 0) for a thread already joined
//   payload        abort_with_payload: natively SIGABRT (134)
//   payloadthread  the same from a child thread, after it prints, while main waits in pthread_join
// Each probe prints `<call>=<rc> errno=<n>`. Every line is a raw write.
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

// Not in the public SDK: libsystem_kernel exports it (xnu libsyscall/wrappers/terminate_with_reason.c),
// and dyld ends a launch that cannot load a dylib through it.
extern void abort_with_payload(uint32_t reason_namespace, uint64_t reason_code, void *payload,
                               uint32_t payload_size, const char *reason_string, uint64_t reason_flags)
    __attribute__((noreturn));
#define FID_OS_REASON_TEST 5 // OS_REASON_TEST, xnu bsd/sys/reason.h

static void say(const char *s) { write(1, s, strlen(s)); }

static void h(int sig) {
    char b[48];
    int n = snprintf(b, sizeof b, "HANDLER sig=%d\n", sig);
    write(1, b, n);
    if (sig == SIGSEGV) _exit(42);
}

static void report(const char *call, int rc, int err) {
    char b[96];
    int n = snprintf(b, sizeof b, "%s=%d errno=%d\n", call, rc, rc ? err : 0);
    write(1, b, n);
}

static void *returns(void *a) { return a; }

static void *payload_child(void *a) {
    (void)a;
    say("child\n");
    abort_with_payload(FID_OS_REASON_TEST, 1, NULL, 0, "fid_signals payloadthread", 0);
}

int main(int argc, char **argv) {
    const char *m = argc > 1 ? argv[1] : "";
    int rc;
    if (!strcmp(m, "raise")) {
        signal(SIGUSR1, h);
        raise(SIGUSR1);
        raise(SIGUSR1);
        say("returned\n");
    } else if (!strcmp(m, "fault")) {
        signal(SIGSEGV, h);
        *(volatile int *)0x10 = 1;
        say("survived\n");
    } else if (!strcmp(m, "kill0self")) { rc = kill(getpid(), 0); report("kill(getpid(), 0)", rc, errno); }
    else if (!strcmp(m, "kill00")) { rc = kill(0, 0); report("kill(0, 0)", rc, errno); }
    else if (!strcmp(m, "kill0init")) { rc = kill(1, 0); report("kill(1, 0)", rc, errno); }
    else if (!strcmp(m, "kill0none")) { rc = kill(0x7ffffff0, 0); report("kill(0x7ffffff0, 0)", rc, errno); }
    else if (!strcmp(m, "pkill0self")) { report("pthread_kill(self, 0)", pthread_kill(pthread_self(), 0), 0); }
    else if (!strcmp(m, "pkill0exited")) {
        pthread_t t;
        pthread_create(&t, NULL, returns, NULL);
        pthread_join(t, NULL);
        report("pthread_kill(joined, 0)", pthread_kill(t, 0), 0);
    } else if (!strcmp(m, "payload")) {
        say("before\n");
        abort_with_payload(FID_OS_REASON_TEST, 1, NULL, 0, "fid_signals payload", 0);
    } else if (!strcmp(m, "payloadthread")) {
        pthread_t t;
        pthread_create(&t, NULL, payload_child, NULL);
        pthread_join(t, NULL);
    } else { say("fid_signals: unknown mode\n"); return 2; }
    return 0;
}
