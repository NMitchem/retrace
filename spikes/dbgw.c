// dbgw.c — prove HVF write-watchpoints (DBGWVR0/DBGWCR0) for M5, in retrace's exact shape:
// guest at EL0, VBAR_EL1 -> trampoline page of `hvc #0` slots, set_trap_debug_exceptions(true),
// MMU off (VA == IPA), only anonymous guest memory. Answers, empirically, on this OS/silicon:
//   F4a: does a watched EL0 store deliver DIRECT to EL2 (ESR_EL2 EC=0x34), not via the guest VBAR?
//   F4b: does hv_vcpu_exit's virtual_address (FAR) hold the accessed VA?
//   F4c: pre- or post-retire? (read the watched qword back at the hit)
//   F4d: BAS byte-select: a strb to byte 0 with only bytes 4..7 watched must NOT fire.
// SAFETY: every phase ends at a terminal `hvc #0` (no free spin); still run under the external
// perl process-group timeout (no `timeout` binary on this platform).
//   clang -O2 -o dbgw dbgw.c -framework Hypervisor
//   codesign -s - -f --entitlements ent.plist dbgw
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./dbgw
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define CODE_IPA  0x10000000ULL
#define TRAMP_IPA 0x10004000ULL
#define DATA_IPA  0x10008000ULL
#define PG        0x4000
#define MDSCR_MDE (1ULL << 15)
// DBGWCR: E=1 (bit0) | PAC=0b10 EL0-only (bits2:1) | LSC=0b10 store-only (bits4:3) | BAS<<5.
#define DBGWCR(bas) (0x15ULL | ((uint64_t)(bas) << 5))

static hv_vcpu_t vcpu; static hv_vcpu_exit_t *vexit;
static uint64_t rg(hv_reg_t r){uint64_t v=0;hv_vcpu_get_reg(vcpu,r,&v);return v;}

// Run once and classify: 1 = watchpoint exit (EC2 0x34/0x35), 2 = terminal hvc (EC2 0x16), 0 = other.
static int run_once(const char *tag, uint64_t *far_out){
    hv_vcpu_run(vcpu);
    uint64_t esr2 = vexit->exception.syndrome;
    uint32_t ec2 = (uint32_t)((esr2>>26)&0x3f);
    uint64_t pc = rg(HV_REG_PC), far = vexit->exception.virtual_address;
    printf("[%s] reason=%u EC2=0x%02x pc=0x%llx far=0x%llx\n", tag, vexit->reason, ec2, pc, far);
    if (far_out) *far_out = far;
    if (ec2==0x34||ec2==0x35) return 1;
    if (ec2==0x16) return 2;
    return 0;
}

static void reset_guest(uint64_t pc){
    hv_vcpu_set_reg(vcpu, HV_REG_PC, pc);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);            // EL0t, SS clear
}

int main(void){
    if (hv_vm_create(NULL)) { printf("no vm\n"); return 1; }
    // Guest code (offsets from CODE_IPA):
    //   +0x00 movz x1, #0x1000, lsl #16   ; x1 = 0x10000000
    //   +0x04 movk x1, #0x8000            ; x1 = DATA_IPA
    //   +0x08 movz x2, #0x42
    //   +0x0c str  x2, [x1]               ; the watched 8-byte store (F4a/b/c)
    //   +0x10 hvc  #0                     ; terminal
    //   +0x14 strb w2, [x1]               ; byte-0 store (F4d entry point)
    //   +0x18 hvc  #0                     ; terminal
    uint32_t code[7] = { 0xD2A20001, 0xF2900001, 0xD2800842, 0xF9000022,
                         0xD4000002, 0x39000022, 0xD4000002 };
    void *cb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    memcpy(cb,code,sizeof(code));
    hv_vm_map(cb,CODE_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);
    void *tb = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    for (int i=0;i<16;i++) ((uint32_t*)tb)[i*0x80/4]=0xD4000002;       // hvc #0 vectors
    hv_vm_map(tb,TRAMP_IPA,PG,HV_MEMORY_READ|HV_MEMORY_EXEC);
    void *db = mmap(NULL,PG,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
    memset(db,0,PG);
    hv_vm_map(db,DATA_IPA,PG,HV_MEMORY_READ|HV_MEMORY_WRITE);
    volatile uint64_t *data = (volatile uint64_t *)db;

    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    hv_vcpu_create(&vcpu,&vexit,cfg);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_VBAR_EL1,TRAMP_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_SCTLR_EL1,0x30d00800);         // MMU off
    printf("set_trap_debug_exceptions -> %d\n",
           hv_vcpu_set_trap_debug_exceptions(vcpu,true));

    // ---- F4a/b/c: watch the full qword at DATA_IPA, run into the str ----
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWVR0_EL1,DATA_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWCR0_EL1,DBGWCR(0xFF));
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_MDE);
    reset_guest(CODE_IPA);
    uint64_t far=0; int k = run_once("F4a", &far);
    uint64_t pc = rg(HV_REG_PC);
    printf("F4a: %s\n", k==1?"DELIVERED DIRECT-EL2 (EC=0x34/0x35)":
                        (k==2?"NOT DELIVERED (ran free to terminal hvc)":"UNEXPECTED EXIT"));
    printf("F4b: far=0x%llx vs accessed VA 0x%llx -> %s\n", far, DATA_IPA,
           far==DATA_IPA?"EXACT":"NOT EXACT (record what it holds)");
    printf("F4c: watched mem=0x%llx -> %s; pc=0x%llx (%s the str at +0xc)\n",
           *data, *data==0?"PRE-RETIRE (store not yet landed)":"POST-RETIRE (store landed)",
           pc, pc==CODE_IPA+0xcULL?"AT":"PAST");

    // Disarm and resume from wherever the hit parked us: must reach the terminal hvc with 0x42 stored.
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWCR0_EL1,0);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,0);
    reset_guest(pc);
    k = run_once("resume", NULL);
    printf("   resume disarmed: %s, mem=0x%llx (expect 0x42)\n",
           k==2?"terminal hvc":"UNEXPECTED", *data);

    // ---- F4d: watch only bytes 4..7 (BAS=0xF0); run the strb to byte 0 ----
    *data = 0;
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWVR0_EL1,DATA_IPA);
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_DBGWCR0_EL1,DBGWCR(0xF0));
    hv_vcpu_set_sys_reg(vcpu,HV_SYS_REG_MDSCR_EL1,MDSCR_MDE);
    reset_guest(CODE_IPA+0x14ULL);
    k = run_once("F4d", NULL);
    printf("F4d: strb to byte 0 under BAS=0xF0 -> %s (mem=0x%llx)\n",
           k==2?"NO FIRE (BAS is byte-selective)":"FIRED (BAS NOT byte-selective!)", *data);

    hv_vcpu_destroy(vcpu); hv_vm_destroy();
    return 0;
}
