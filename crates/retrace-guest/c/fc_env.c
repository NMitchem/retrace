// M49 first-contact corpus (spec §3b): argv and the environment. firstcontact_e2e runs it with two
// arguments and FC_ENV_PROBE=firstcontact in the environment of both the native run and the
// recorder, so it prints what a newcomer's program would see of each.
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    printf("argc=%d\n", argc);
    for (int i = 1; i < argc; i++) printf("argv[%d]=%s\n", i, argv[i]);
    const char *v = getenv("FC_ENV_PROBE");
    printf("FC_ENV_PROBE=%s\n", v ? v : "(unset)");
    return 0;
}
