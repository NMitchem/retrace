// sstep.c — prove HVF single-step + HW breakpoints for M3, in retrace's exact shape:
// guest at EL0, VBAR_EL1 -> trampoline page whose 16 vector slots are each `hvc #0`.
// Answers: (F1) which route does a step exception take? (F2) does SS survive a
// trapped+manually-skipped instruction (retrace's below-the-trace emulation)? (F3) do
// DBGBVR0/DBGBCR0 HW breakpoints deliver, and via which route?
//
// SAFETY: bounded, single hv_vcpu_run per phase, terminal `hvc` (no infinite spin) so a
// free run always ends; still run under the external perl process-group timeout in case a
// vCPU wedges (there is no `timeout` binary). MMU off; only anonymous guest memory mapped.
//   clang -O2 -o sstep sstep.c -framework Hypervisor
//   codesign -s - -f --entitlements ent.plist sstep
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./sstep
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define CODE_IPA  0x10000000ULL
#define TRAMP_IPA 0x10004000ULL
#define PG        0x4000
#define PSTATE_SS (1ULL << 21)
#define MDSCR_SS  (1ULL << 0)
#define MDSCR_MDE (1ULL << 15)

static const char *rstr(hv_return_t r){switch((uint32_t)r){case 0:return"HV_SUCCESS";
  case 0xfae94007:return"HV_DENIED";case 0xfae94003:return"HV_BAD_ARGUMENT";
  case 0xfae94004:return"HV_ILLEGAL_GUEST_STATE";default:return"OTHER";}}

static hv_vcpu_t vcpu; static hv_vcpu_exit_t *vexit;

static uint64_t sys(hv_sys_reg_t r){uint64_t v=0;hv_vcpu_get_sys_reg(vcpu,r,&v);return v;}
static uint64_t rg(hv_reg_t r){uint64_t v=0;hv_vcpu_get_reg(vcpu,r,&v);return v;}

// Classification of one hv_vcpu_run exit (all set by run_once for the caller to act on):
enum { R_OTHER=0, R_STEP_EL0=1, R_STEP_TRAP=6, R_HVC=3, R_BREAK=4 };
static uint32_t g_ec2, g_ec1, g_el; static uint64_t g_pc, g_elr, g_esr1, g_cpsr;

// Run once; print full state; classify. On THIS platform every debug exception (step /
// breakpoint) routes DIRECTLY to EL2 as an ESR_EL2 exit (EC2), never through the guest's
// EL1 VBAR trampoline. The discriminator between "clean step" and "stepped an instruction
// that itself trapped to EL1" is the guest's current EL (from CPSR): EL0 vs EL1.
//   R_STEP_EL0 (1): step exception, guest back at EL0 -> one EL0 insn retired cleanly.
//   R_STEP_TRAP (6): step exception, guest now at EL1 (pc in trampoline) -> the stepped EL0
//                    insn trapped; ESR_EL1/ELR_EL1 hold the trap syndrome/faulting-insn addr.
//   R_HVC (3): reached an `hvc` (EC2=0x16) -> a guest vector's hvc, or the terminal hvc.
//   R_BREAK (4): HW breakpoint exception reached EL2 directly (EC2=0x30/0x31).
//   R_OTHER (0): anything else (printed).
static int run_once(const char *tag){
    hv_return_t r = hv_vcpu_run(vcpu);
    uint64_t esr2 = vexit->exception.syndrome; g_ec2 = (esr2>>26)&0x3f;
    g_pc = rg(HV_REG_PC); g_elr = sys(HV_SYS_REG_ELR_EL1); g_esr1 = sys(HV_SYS_REG_ESR_EL1);
    g_cpsr = rg(HV_REG_CPSR); g_el = (uint32_t)((g_cpsr>>2)&3);
    g_ec1 = (uint32_t)((g_esr1>>26)&0x3f);
    printf("[%s] run=%s reason=%u EC2=0x%02x pc=0x%llx EL%u | ESR_EL1 EC1=0x%02x elr=0x%llx\n",
           tag, rstr(r), vexit->reason, g_ec2, g_pc, g_el, g_ec1, g_elr);
    if (g_ec2==0x32||g_ec2==0x33) return g_el==0 ? R_STEP_EL0 : R_STEP_TRAP;
    if (g_ec2==0x30||g_ec2==0x31) return R_BREAK;
    if (g_ec2==0x16) return R_HVC;
    return R_OTHER;
}

// Put the guest at EL0 at `at` with SS armed (PSTATE.SS set, EL0t). Mirrors how Box_ would
// re-enter EL0 after emulating+skipping a below-the-trace instruction.
static void resume_el0_ss(uint64_t at){
    hv_vcpu_set_reg(vcpu, HV_REG_PC, at);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, PSTATE_SS);              // EL0t + SS
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_MDSCR_EL1, MDSCR_SS);
}

int main(void){
    if (hv_vm_create(NULL)) { printf("no vm\n"); return 1; }
    // EL0 code: nop x4, mrs x1,cntvct_el0 (traps EL0->EL1 when CNTKCTL.EL0VCTEN=0), nop x4,
    // then a TERMINAL `hvc #0` (a free run ends here instead of spinning -> never hangs).
    //   code[0..3] nop      @ 0x..00 0x..04 0x..08 0x..0c
    //   code[4]    mrs       @ 0x..10   (the trapping insn F2 skips)
    //   code[5..8] nop       @ 0x..14 0x..18 0x..1c 0x..20   (0x..20 = the F3 breakpoint)
    //   code[9]    hvc #0     @ 0x..24   (terminal)
    uint32_t code[10]; for (int i=0;i<10;i++) code[i]=0xD503201F;      // nop
    code[4]=0xD53BE041;                                                // mrs x1, cntvct_el0
    code[9]=0xD4000002;                                                // hvc #0 (terminal)
    void *cb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    memcpy(cb,code,sizeof(code));
    hv_vm_map(cb,CODE_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);
    // EL1 trampoline: 16 vector slots, 0x80 apart, each `hvc #0`.
    void *tb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    for (int i=0;i<16;i++) ((uint32_t*)tb)[i*0x80/4]=0xD4000002;       // hvc #0
    hv_vm_map(tb,TRAMP_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);

    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    hv_vcpu_create(&vcpu,&vexit,cfg);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_VBAR_EL1,TRAMP_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_SCTLR_EL1,0x30d00800);         // MMU off
    printf("set_trap_debug_exceptions -> %s\n",
           rstr(hv_vcpu_set_trap_debug_exceptions(vcpu,true)));

    // ---- F1 + step loop: arm SS, expect exactly one retired insn per run ----
    resume_el0_ss(CODE_IPA);
    int route = 0, f1_ok = 1;
    for (int i=0;i<4;i++){                                             // step the 4 leading nops
        int k = run_once("SS");
        if (i==0) route = k;
        uint64_t want = CODE_IPA+4ULL*(i+1);
        printf("   step %d -> pc=0x%llx (expect 0x%llx) %s\n",
               i+1, g_pc, want, (k==R_STEP_EL0 && g_pc==want)?"OK":"BAD");
        if (!(k==R_STEP_EL0 && g_pc==want)) f1_ok = 0;
        resume_el0_ss(g_pc);                                           // re-arm SS for next step
    }
    printf("F1: step route = %s (%d/4 clean single-steps)\n",
           route==R_STEP_EL0?"DIRECT-EL2 (ESR_EL2 EC=0x32)":"?? not a direct step", f1_ok?4:0);

    // ---- F2: pc is now at the trapping MRS (0x..10). Stepping it does NOT retire cleanly:
    // it traps EL0->EL1 (MSR trap, EC1=0x18) and surfaces as a DIRECT-EL2 step exit with the
    // guest now at EL1 (R_STEP_TRAP). Emulate retrace's below-the-trace skip (advance the
    // guest PC past it, re-enter EL0, re-arm SS) and confirm the NEXT run retires EXACTLY one
    // more EL0 instruction. ----
    int k = run_once("MRS");
    int trapped = (k==R_STEP_TRAP && g_ec1==0x18);
    printf("   MRS stepped -> k=%d ec1=0x%02x elr=0x%llx %s\n", k, g_ec1, g_elr,
           trapped ? "TRAPPED at EL1 (needs emulation)" : "did NOT trap as expected");
    uint64_t skip_to = g_elr + 4;                                     // ELR_EL1 = faulting MRS addr
    resume_el0_ss(skip_to);                                            // skip the MRS, like run()
    k = run_once("F2");
    uint64_t want = skip_to + 4;
    int f2_ok = (k==R_STEP_EL0 && g_el==0 && g_pc==want);
    printf("F2: re-arm across skipped insn -> %s (pc=0x%llx expect 0x%llx, one clean step)\n",
           f2_ok?"OK":"UNEXPECTED", g_pc, want);

    // ---- F3: re-init a clean EL0 state; disarm SS; arm DBGBVR0/DBGBCR0 at 0x..20; run FREE
    // (no SS) and see whether the HW breakpoint delivers, and via which route. Terminal hvc
    // bounds a non-delivery. ----
    hv_vcpu_set_reg(vcpu,HV_REG_PC,CODE_IPA+5*4);                     // 0x..14 (first nop past MRS)
    hv_vcpu_set_reg(vcpu,HV_REG_CPSR,0);                               // EL0t, SS clear
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_MDE);          // MDE on (breakpoints), SS off
    uint64_t bp = CODE_IPA + 8*4;                                      // 0x..20 = code[8], a nop
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGBVR0_EL1,bp);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGBCR0_EL1,0x1E5);            // E=1 PMC=EL0 BAS=0xF
    k = run_once("BVR");
    int f3_hit = (k==R_BREAK && g_pc==bp);
    printf("F3: HW breakpoint = %s\n",
           f3_hit ? "DELIVERED, DIRECT-EL2 (ESR_EL2 EC=0x30, pc=DBGBVR0, insn not yet retired)"
                  : (k==R_HVC ? "NOT DELIVERED (guest ran free to terminal hvc)"
                              : "NOT DELIVERED (unexpected exit)"));

    printf("\n=> F1=%s  F2=%s  F3=%s\n",
           (route==R_STEP_EL0&&f1_ok)?"PASS":"FAIL",
           f2_ok?"PASS":"FAIL",
           f3_hit?"DELIVERED":"NOT-DELIVERED");
    hv_vcpu_destroy(vcpu); hv_vm_destroy();
    return 0;
}
