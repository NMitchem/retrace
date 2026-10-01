// M47 t0 M3(a) fallback (no debugger on this host): the NATIVE answer to the 3403 request.
//  (1) mach_ports_register(mach_task_self(), {bootstrap_port, NULL, NULL}, 3), the call
//      xpc_atfork_prepare makes (m3a-trace.err: descriptor 0 a live send right, 1 and 2 NULL, all
//      COPY_SEND), printing the kern_return;
//  (2) the same request hand-built to the measured 64-byte layout (m3a-trace.err send+000..030:
//      bits 0x80001513, size 64, id 3403 at +20, count 3 at +24, three port descriptors from +28 with
//      disposition 0x13 at +10 and type 0 at +11), sent SEND|RCV, printing the return and the
//      reply's bytes (msgh_id at +20, RetCode at +32). Mode `msg2` sends it through
//      mach_msg2_internal with the register packing the trace shows the MIG stub using; mode `msg`
//      through plain mach_msg, which macOS 26 kills (SIGKILL, rc 137: a kernel-object send without
//      MACH64_SEND_KOBJECT_CALL). Mode `call` is (1) alone.
#include <mach/mach.h>
#include <servers/bootstrap.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *mode = argc > 1 ? argv[1] : "both";
    printf("mode=%s pid=%d bootstrap_port=%#x task=%#x\n", mode, getpid(), bootstrap_port, mach_task_self());
    if (strcmp(mode, "msg") != 0 && strcmp(mode, "msg2") != 0) {
        mach_port_t ports[3] = { bootstrap_port, MACH_PORT_NULL, MACH_PORT_NULL };
        kern_return_t kr = mach_ports_register(mach_task_self(), ports, 3);
        printf("(1) mach_ports_register(task, {bootstrap %#x, 0, 0}, 3) kr=%d (%s)\n", bootstrap_port, kr, mach_error_string(kr));
        if (strcmp(mode, "call") == 0) return 0;
    }

    union { unsigned char b[256]; mach_msg_header_t h; } m;
    memset(&m, 0, sizeof m);
    mach_port_t reply = mig_get_reply_port();
    uint32_t w[16] = {0};
    w[0] = 0x80001513;            // COMPLEX | local MAKE_SEND_ONCE (0x15) | remote COPY_SEND (0x13)
    w[1] = 64;                    // msgh_size
    w[2] = mach_task_self();      // remote
    w[3] = reply;                 // local
    w[4] = 0;                     // voucher
    w[5] = 3403;                  // msgh_id
    w[6] = 3;                     // descriptor count
    uint32_t names[3] = { bootstrap_port, 0, 0 };
    for (int i = 0; i < 3; i++) {
        unsigned char *d = (unsigned char *)&w[7 + 3 * i];
        memcpy(d, &names[i], 4);  // name
        d[10] = 0x13;             // disposition MACH_MSG_TYPE_COPY_SEND
        d[11] = 0;                // type MACH_MSG_PORT_DESCRIPTOR
    }
    memcpy(m.b, w, 64);
    printf("(2) request:");
    for (int i = 0; i < 64; i++) printf("%s%02x", i % 16 ? " " : "\n    ", m.b[i]);
    printf("\n");
    mach_msg_return_t mr;
    if (strcmp(mode, "msg2") == 0) {
        // The exact register packing the trace shows the MIG stub using (m3a-trace.err [trap] -47):
        // x1 options 0x200000003 (SEND|RCV|SEND_KOBJECT_CALL), x2 send_size<<32|bits,
        // x3 local<<32|remote, x4 id<<32|voucher, x5 rcv_name<<32|desc_count, x6 rcv_size 0x2c.
        extern mach_msg_return_t mach_msg2_internal(void *, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t);
        mr = mach_msg2_internal(m.b, 0x200000003ull, (64ull << 32) | 0x80001513ull,
                                ((uint64_t)reply << 32) | mach_task_self(), (3403ull << 32) | 0,
                                ((uint64_t)reply << 32) | 3, 0x2c, 0);
    } else {
        mr = mach_msg(&m.h, MACH_SEND_MSG | MACH_RCV_MSG, 64, sizeof m.b, reply, MACH_MSG_TIMEOUT_NONE, MACH_PORT_NULL);
    }
    uint32_t id, size, ret;
    memcpy(&size, m.b + 4, 4); memcpy(&id, m.b + 20, 4); memcpy(&ret, m.b + 32, 4);
    printf("(2) mach_msg ret=%#x reply msgh_size=%u msgh_id=%u RetCode(+32)=%d\n    reply:", mr, size, id, (int)ret);
    for (int i = 0; i < 48; i++) printf("%s%02x", i % 16 ? " " : "\n    ", m.b[i]);
    printf("\n");
    return 0;
}
