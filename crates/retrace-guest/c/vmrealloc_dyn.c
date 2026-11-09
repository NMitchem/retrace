// M57 t7: mach_vm_reallocate (4829) by case. Each mode prints "<mode> kr=<kr> moved=<0|1> head=<n> tail=<n>
// srcgone=<0|1>" with no absolute address: head is the count of preserved 0xab bytes in the first S1, tail
// the count of nonzero bytes past S1 (0 expected), srcgone whether the old source page is unmapped.
#include <stdio.h>
#include <string.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#define S1 0x304000ULL
#define S2 0x604000ULL
static mach_vm_address_t scratch;
static int mapped(mach_vm_address_t a) { // a one-page mach_vm_copy out of `a`: kr 0 iff it is mapped
  return mach_vm_copy(mach_task_self(), a, 0x4000, scratch) == 0;
}
static mach_vm_address_t mk(mach_vm_size_t sz) {
  mach_vm_address_t a = 0;
  if (mach_vm_map(mach_task_self(), &a, sz, 0x3fffff, VM_FLAGS_ANYWHERE | VM_MAKE_TAG(3), MACH_PORT_NULL, 0, 0, VM_PROT_DEFAULT, VM_PROT_ALL, VM_INHERIT_DEFAULT)) return 0;
  memset((void *)a, 0xab, sz);
  return a;
}
static void report(const char *m, kern_return_t kr, mach_vm_address_t src, mach_vm_address_t dst) {
  long head = 0, tail = 0;
  if (kr == 0) {
    unsigned char *p = (unsigned char *)dst;
    for (mach_vm_size_t i = 0; i < S1; i += 0x1000) head += p[i] == 0xab;
    for (mach_vm_size_t i = S1; i < S2; i += 0x1000) tail += p[i] != 0;
  }
  printf("%s kr=%d moved=%d head=%ld tail=%ld srcgone=%d\n", m, kr, kr == 0 && dst != src, head, tail, !mapped(src));
}
int main(int argc, char **argv) {
  mach_vm_allocate(mach_task_self(), &scratch, 0x4000, VM_FLAGS_ANYWHERE);
  const char *m = argc > 1 ? argv[1] : "inplace";
  task_t t = mach_task_self(); int fl = VM_FLAGS_ANYWHERE | VM_MAKE_TAG(3);
  mach_vm_address_t a = mk(S1), d = a; kern_return_t kr;
  if (!strcmp(m, "inplace")) kr = mach_vm_reallocate(t, a, S1, &d, S2, 0x3fffff, 0, fl);
  else if (!strcmp(m, "blocked")) {
    mach_vm_address_t b = a + S1;
    if (mach_vm_allocate(t, &b, 0x4000, VM_FLAGS_FIXED)) { printf("blocker failed\n"); return 1; }
    kr = mach_vm_reallocate(t, a, S1, &d, S2, 0x3fffff, 0, fl);
  }
  else if (!strcmp(m, "shrink")) kr = mach_vm_reallocate(t, a, S1, &d, 0x4000, 0x3fffff, 0, fl);
  else if (!strcmp(m, "unaligned")) kr = mach_vm_reallocate(t, a + 1, S1, &d, S2, 0, 0, fl);
  else if (!strcmp(m, "unmapped")) { d = 0x10000000; a = d; kr = mach_vm_reallocate(t, a, S1, &d, S2, 0, 0, fl); }
  else if (!strcmp(m, "fixed")) { d = a + 0x10000000; kr = mach_vm_reallocate(t, a, S1, &d, S2, 0, 0, VM_FLAGS_FIXED | VM_MAKE_TAG(3)); }
  else if (!strcmp(m, "zero_dst")) { d = 0; kr = mach_vm_reallocate(t, a, S1, &d, S2, 0, 0, fl); }
  else { printf("unknown mode\n"); return 1; }
  report(m, kr, a, d);
  return 0;
}
