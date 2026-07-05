// Minimal HVF spike: run real guest instructions at EL1 (MMU off), trap out via HVC,
// decode the exit syndrome. This is the core retrace-box vCPU loop in miniature.
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define GUEST_PA 0x10000000ULL   // where we place guest code (IPA == PA, MMU off)

static const char *rstr(hv_return_t r){switch((uint32_t)r){case 0:return"HV_SUCCESS";
  case 0xfae94007:return"HV_DENIED";case 0xfae94003:return"HV_BAD_ARGUMENT";
  case 0xfae94004:return"HV_ILLEGAL_GUEST_STATE";default:return"OTHER";}}

int main(void){
    hv_return_t r = hv_vm_create(NULL);
    printf("hv_vm_create -> %s\n", rstr(r));
    if (r) return 1;

    // Guest program: x0 = 0x1234; x0 += 1; HVC #0   (proves multi-instruction native exec)
    uint32_t code[] = { 0xD2824680, 0x91000400, 0xD4000002 };
    size_t pg = 0x4000; // 16 KiB granule
    void *buf = mmap(NULL, pg, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE, -1, 0);
    memcpy(buf, code, sizeof(code));
    r = hv_vm_map(buf, GUEST_PA, pg, HV_MEMORY_READ|HV_MEMORY_WRITE|HV_MEMORY_EXEC);
    printf("hv_vm_map    -> %s\n", rstr(r));

    hv_vcpu_t vcpu; hv_vcpu_exit_t *exit;
    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    r = hv_vcpu_create(&vcpu, &exit, cfg);
    printf("hv_vcpu_create -> %s\n", rstr(r));

    // Start at EL1h with DAIF masked; MMU OFF so IPA is used directly.
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0x3c5);
    hv_vcpu_set_reg(vcpu, HV_REG_PC, GUEST_PA);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_SCTLR_EL1, 0x30d00800); // reset-ish, M(bit0)=0 => MMU off

    r = hv_vcpu_run(vcpu);
    printf("hv_vcpu_run  -> %s\n", rstr(r));

    uint32_t reason = exit->reason;
    printf("exit.reason  -> %u %s\n", reason,
           reason==HV_EXIT_REASON_EXCEPTION?"EXCEPTION":
           reason==HV_EXIT_REASON_VTIMER_ACTIVATED?"VTIMER":
           reason==HV_EXIT_REASON_CANCELED?"CANCELED":"UNKNOWN");

    uint64_t esr = exit->exception.syndrome;
    uint32_t ec = (esr >> 26) & 0x3f;
    printf("ESR_EL2      -> 0x%016llx  EC=0x%02x %s\n", esr, ec,
           ec==0x16?"EC_AA64_HVC (guest HVC trapped to VMM)":
           ec==0x15?"EC_AA64_SVC":"other");

    uint64_t x0=0; hv_vcpu_get_reg(vcpu, HV_REG_X0, &x0);
    printf("guest X0     -> 0x%llx (expect 0x1235 = 0x1234+1, proves native exec)\n", x0);

    int ok = (reason==HV_EXIT_REASON_EXCEPTION) && (ec==0x16) && (x0==0x1235);
    printf(ok ? "=> CORE LOOP VERIFIED: guest ran natively, HVC trapped out with decodable ESR\n"
              : "=> MISMATCH\n");
    hv_vcpu_destroy(vcpu);
    hv_vm_destroy();
    return ok?0:2;
}
