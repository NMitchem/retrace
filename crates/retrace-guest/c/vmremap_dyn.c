// M39 wall-1 guard AND the native protections probe (spec §3f, R7). Two remaps, both shared
// (copy=FALSE), FIXED|OVERWRITE into a vm_allocate'd 3-page region — libffi's exact shape:
//  SELF: alias this program's own text page and CALL through the alias (the executable proof:
//        an alias that is not executable, or not the same bytes, cannot return 42).
//  FFI:  dlopen /usr/lib/libffi-trampolines.dylib (fat x86_64+arm64e — its load is part of the
//        measured wall), remap its whole 2-page __TEXT from the base (the export
//        ffi_closure_trampoline_table_page sits at __TEXT+0x4000), compare bytes through the alias.
// Printed cur/max are the KERNEL's answer natively; under retrace they must be the box's, and
// vmremap_e2e asserts the two agree — so the model's constants are measured, never chosen.
#include <dlfcn.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define PAGE 0x4000UL

// Position-independent leaf: no data references, so it runs from any page it is aliased to.
__attribute__((noinline)) static int forty_two(void) { return 42; }

static kern_return_t remap_shared(mach_vm_address_t target, mach_vm_size_t size,
                                  mach_vm_address_t src, vm_prot_t *cur, vm_prot_t *max) {
    return mach_vm_remap(mach_task_self(), &target, size, 0, VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
                         mach_task_self(), src, FALSE, cur, max, VM_INHERIT_SHARE);
}

int main(void) {
    mach_vm_address_t region = 0;
    kern_return_t kr = mach_vm_allocate(mach_task_self(), &region, 3 * PAGE, VM_FLAGS_ANYWHERE);
    if (kr != KERN_SUCCESS) { printf("SELF allocate kr=%d\n", kr); return 2; }
    uintptr_t fn = (uintptr_t)&forty_two;
    mach_vm_address_t page = fn & ~(PAGE - 1);
    mach_vm_address_t target = region + PAGE;
    vm_prot_t cur = 0, max = 0;
    kr = remap_shared(target, PAGE, page, &cur, &max);
    int called = -1;
    if (kr == KERN_SUCCESS) {
        int (*alias)(void) = (int (*)(void))(target + (fn - page));
        called = alias();
    }
    printf("SELF kr=%d cur=%d max=%d call=%d\n", kr, cur, max, called);

    void *h = dlopen("/usr/lib/libffi-trampolines.dylib", RTLD_NOW);
    void *sym = h ? dlsym(h, "ffi_closure_trampoline_table_page") : NULL;
    if (!sym) { printf("FFI dlopen/dlsym failed: %s\n", dlerror()); return 3; }
    mach_vm_address_t text = ((mach_vm_address_t)sym) - PAGE;
    mach_vm_address_t region2 = 0;
    kr = mach_vm_allocate(mach_task_self(), &region2, 3 * PAGE, VM_FLAGS_ANYWHERE);
    if (kr != KERN_SUCCESS) { printf("FFI allocate kr=%d\n", kr); return 4; }
    mach_vm_address_t target2 = region2 + PAGE;
    vm_prot_t cur2 = 0, max2 = 0;
    kr = remap_shared(target2, 2 * PAGE, text, &cur2, &max2);
    int same = -1;
    if (kr == KERN_SUCCESS) same = memcmp((void *)target2, (void *)text, 2 * PAGE) == 0;
    printf("FFI kr=%d cur=%d max=%d same=%d\n", kr, cur2, max2, same);
    return 0;
}
