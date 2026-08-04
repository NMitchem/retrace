// M9. Console output through stdio, which reaches the kernel as write_nocancel (397) — NOT write
// (4). hello_dyn.c calls write(2) directly, so before M9 nothing in the gate ever exercised the
// _nocancel console path: it fell through to the generic forward, the HOST kernel executed it (the
// text appeared on retrace's own stdout, which is why a recording looked correct), the trace
// captured no console bytes, and replay produced nothing. jq flushes stdout exactly this way.
#include <stdio.h>

int main(void) {
    printf("stdio\n");
    return 0;
}
