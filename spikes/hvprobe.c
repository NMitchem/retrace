#include <Hypervisor/Hypervisor.h>
#include <mach/mach.h>
#include <sys/mman.h>
#include <stdio.h>
#include <stdint.h>

static const char *rstr(hv_return_t r) {
    switch ((uint32_t)r) {
        case 0: return "HV_SUCCESS";
        case 0xfae94001: return "HV_ERROR";
        case 0xfae94002: return "HV_BUSY";
        case 0xfae94003: return "HV_BAD_ARGUMENT";
        case 0xfae94005: return "HV_NO_RESOURCES";
        case 0xfae94006: return "HV_NO_DEVICE";
        case 0xfae94007: return "HV_DENIED";
        case 0xfae9400f: return "HV_UNSUPPORTED";
        default: return "UNKNOWN";
    }
}

int main(void) {
    // Claim #1: does hv_vm_create succeed? (HV_DENIED without entitlement)
    hv_return_t r = hv_vm_create(NULL);
    printf("hv_vm_create(NULL)            -> 0x%08x %s\n", (uint32_t)r, rstr(r));
    if (r != HV_SUCCESS) {
        printf("=> cannot proceed (this is the no-entitlement expected path)\n");
        return (r == HV_DENIED) ? 42 : 1;  // 42 = cleanly denied
    }

    // Claim #10: default IPA bit-length
    uint32_t ipa = 0;
    r = hv_vm_config_get_default_ipa_size(&ipa);
    printf("default IPA bit-length        -> 0x%08x %s  bits=%u (%.0f GiB)\n",
           (uint32_t)r, rstr(r), ipa, ipa ? (double)(1ULL<<ipa)/(1024.0*1024*1024) : 0.0);

    // Claim: read ID_AA64DFR0_EL1 -> breakpoint/watchpoint slot counts
    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    uint64_t dfr0 = 0;
    r = hv_vcpu_config_get_feature_reg(cfg, HV_FEATURE_REG_ID_AA64DFR0_EL1, &dfr0);
    unsigned brps = ((dfr0 >> 12) & 0xf) + 1;   // BRPs field + 1
    unsigned wrps = ((dfr0 >> 20) & 0xf) + 1;   // WRPs field + 1
    unsigned pmuver = (dfr0 >> 8) & 0xf;
    printf("ID_AA64DFR0_EL1               -> 0x%08x %s  val=0x%016llx\n", (uint32_t)r, rstr(r), dfr0);
    printf("   HW breakpoints=%u  watchpoints=%u  PMUVer=0x%x (0=no PMUv3)\n", brps, wrps, pmuver);

    // Claim #3: stage-2 RWX map of a plain RW MAP_ANON buffer (no PROT_EXEC, no MAP_JIT)
    size_t sz = 16 * 1024;  // default granule is 16 KiB
    void *buf = mmap(NULL, sz, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    hv_ipa_t gpa = 0x100000000ULL;  // 4 GiB, within 36-bit default IPA
    r = hv_vm_map(buf, gpa, sz, HV_MEMORY_READ | HV_MEMORY_WRITE | HV_MEMORY_EXEC);
    printf("hv_vm_map RWX (RW anon buf)   -> 0x%08x %s\n", (uint32_t)r, rstr(r));

    // Create a vcpu and read the time meter (claim #2: only exec_time, no instr counter)
    hv_vcpu_t vcpu; hv_vcpu_exit_t *exit;
    r = hv_vcpu_create(&vcpu, &exit, cfg);
    printf("hv_vcpu_create                -> 0x%08x %s\n", (uint32_t)r, rstr(r));
    if (r == HV_SUCCESS) {
        uint64_t t = 0;
        r = hv_vcpu_get_exec_time(vcpu, &t);
        printf("hv_vcpu_get_exec_time         -> 0x%08x %s  (time meter, not instr count)\n", (uint32_t)r, rstr(r));
        // Confirm single-step config path exists: enable debug-exception trapping
        r = hv_vcpu_set_trap_debug_exceptions(vcpu, true);
        printf("set_trap_debug_exceptions     -> 0x%08x %s\n", (uint32_t)r, rstr(r));
        hv_vcpu_destroy(vcpu);
    }
    hv_vm_destroy();
    printf("=> ALL CORE CLAIMS EXERCISED\n");
    return 0;
}
