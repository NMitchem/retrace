// M48 brainstorm probe: native values of the commpage JIT bytes and S3_6_C15_C1_5 around
// pthread_jit_write_protect_np, on the main thread and on a fresh thread.
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <libkern/OSCacheControl.h>
static uint64_t rd(void) { uint64_t v; __asm__ volatile("mrs %0, S3_6_C15_C1_5" : "=r"(v)); return v; }
static void *child(void *a) { (void)a; printf("child  initial  sprr=%#llx\n", (unsigned long long)rd()); return 0; }
int main(void) {
  volatile uint8_t *t = (uint8_t *)0xfffffc10cULL;
  volatile uint64_t *rx = (uint64_t *)0xfffffc110ULL, *rw = (uint64_t *)0xfffffc118ULL;
  printf("commpage +0x10c=%u +0x110=%#llx +0x118=%#llx\n", *t, (unsigned long long)*rx, (unsigned long long)*rw);
  printf("supported_np=%d\n", pthread_jit_write_protect_supported_np());
  printf("main   initial  sprr=%#llx\n", (unsigned long long)rd());
  uint32_t *p = mmap(0, 16384, PROT_READ|PROT_WRITE|PROT_EXEC, MAP_PRIVATE|MAP_ANON|MAP_JIT, -1, 0);
  printf("map_jit=%p\n", (void *)p);
  pthread_jit_write_protect_np(0); printf("main   write-en sprr=%#llx\n", (unsigned long long)rd());
  p[0] = 0xd2800540; /* mov x0,#42 */ p[1] = 0xd65f03c0; /* ret */
  pthread_jit_write_protect_np(1); printf("main   protect  sprr=%#llx\n", (unsigned long long)rd());
  sys_icache_invalidate(p, 8);
  printf("jit call=%d\n", ((int (*)(void))p)());
  pthread_jit_write_protect_np(0);
  pthread_t th; pthread_create(&th, 0, child, 0); pthread_join(th, 0);
  printf("main   after-child sprr=%#llx\n", (unsigned long long)rd());
  return 0;
}
