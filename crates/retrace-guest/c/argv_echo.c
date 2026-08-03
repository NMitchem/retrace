// M9. Proves argc/argv reach a real dynamically-linked program through dyld's process-start stack.
// Before M9, build_start_stack pushed argv[0] only and hardcoded argc=1, so no guest could take an
// argument — and jq without a filter does nothing.
#include <unistd.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc < 2) { write(1, "NOARG\n", 6); return 1; }
    write(1, argv[1], strlen(argv[1]));
    write(1, "\n", 1);
    return 0;
}
