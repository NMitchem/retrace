// M9. A guest that closes its own stdout on the way out — jq's exact exit shape, and the second
// half of the console wall. The guest's fd 1 IS retrace's fd 1, so forwarding this close closed
// RETRACE's stdout: the recording was correct in the trace, and every byte the CLI then tried to
// print vanished, with a 0 exit status and no error anywhere.
#include <stdio.h>
#include <unistd.h>

int main(void) {
    printf("closefd\n");
    fflush(stdout);
    close(1);
    return 0;
}
