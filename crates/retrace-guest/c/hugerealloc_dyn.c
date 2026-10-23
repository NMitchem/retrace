// M52 t1 (spec §3b): the regex crash's mechanism without regex. An xzone "huge" chunk (> 2 MiB) grown
// by realloc: libmalloc first tries mach_vm_map(VM_FLAGS_FIXED, no OVERWRITE) at the chunk's end,
// relying on KERN_NO_SPACE when that VA is taken. Adopted from the t0 probe hugerealloc.c.
//   hugerealloc_dyn page   an mmap'd page's marker must survive the chunk's growth
//   hugerealloc_dyn grow   grow a chunk 3 -> 24 MiB, then mmap(NULL, 16 KiB) and write to it
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>

static unsigned char *grow(unsigned char *p, size_t nsz) {
    unsigned char *q = realloc(p, nsz);
    memset(q, 0xab, nsz);
    return q;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *mode = argc > 1 ? argv[1] : "grow";
    size_t sz = 3u << 20;
    unsigned char *p = malloc(sz);
    memset(p, 0xab, sz);
    if (strcmp(mode, "page") == 0) {
        unsigned char *m = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        m[0] = 'M';
        p = grow(p, sz * 2);
        printf("page marker=%s\n", m[0] == 'M' ? "intact" : "lost");
    } else {
        for (int i = 0; i < 3; i++, sz *= 2) p = grow(p, sz * 2);
        unsigned char *m = mmap(NULL, 16384, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
        m[0] = 'A';
        printf("grow ok\n");
    }
    free(p);
    return 0;
}
