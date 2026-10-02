// M47 t0 M1(d): native madvise alignment, rounding and zero length, per advice.
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(void) {
    int adv[] = { MADV_FREE_REUSABLE, MADV_FREE_REUSE, MADV_ZERO, MADV_CAN_REUSE };
    for (unsigned i = 0; i < sizeof adv / sizeof *adv; i++) {
        unsigned char *p = mmap(NULL, 0x10000, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        memset(p, 0xAB, 0x10000);
        int a = madvise(p, 0x4000, adv[i]);          int ea = a ? errno : 0;
        int b = madvise(p + 0x1000, 0x4000, adv[i]); int eb = b ? errno : 0;
        int c = madvise(p + 1, 0x4000, adv[i]);      int ec = c ? errno : 0;
        int d = madvise(p, 0, adv[i]);               int ed = d ? errno : 0;
        memset(p, 0xAB, 0x10000);
        int e = madvise(p, 0x4001, adv[i]);          int ee = e ? errno : 0;
        printf("advice %d: aligned rc=%d/%d off4k rc=%d/%d off1 rc=%d/%d len0 rc=%d/%d len0x4001 rc=%d/%d byte[0x4000]=%#x byte[0x7fff]=%#x byte[0x8000]=%#x\n",
               adv[i], a, ea, b, eb, c, ec, d, ed, e, ee, p[0x4000], p[0x7fff], p[0x8000]);
    }
    return 0;
}
