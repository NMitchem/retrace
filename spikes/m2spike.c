// M2 spike: prove the loader milestone's load-bearing claims in miniature.
//   A) MMU-on: guest-built identity page tables (16 KiB granule, T0SZ=28, start level 2),
//      Normal-memory attributes => unaligned EL0 loads/stores work (page- and block-mapped).
//   B) PAC: with SCTLR_EL1.EnIA=1 and constant APIA keys set via HVF, an EL0
//      pacia/autia round-trip authenticates and the signed pointer differs from the raw one.
//   C) PAC-off fallback: EnIA=0 => pacia/autia are identity (still round-trips).
//   D) Negative control: same code MMU-off => Device memory => alignment fault (EC=0x24,
//      DFSC=0x21), proving MMU-on is load-bearing for real (unaligned-access) code.
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define VEC_IPA    0x04000ULL  // EL1 vector table (EL1-exec only)
#define L2_IPA     0x08000ULL  // start-level (2) translation table, 2048 entries
#define L3_IPA     0x0C000ULL  // L3 table covering the first 32 MiB
#define CODE_IPA   0x10000ULL  // EL0 test code (RO, EL0-exec)
#define DATA_IPA   0x1C000ULL  // EL0 data page (RW)
#define LOW_LEN    0x20000ULL  // one host buffer backs IPA [0, 128K)
#define BLOCK_IPA  0x4000000ULL // 64 MiB: covered by an L2 32 MiB block descriptor

// Descriptor low attrs: AF | SH=inner | AttrIndx=0 (MAIR attr0 = Normal WBWA).
#define ATTRS      (0x400ULL | 0x300ULL)
#define AP_EL1RW   0x00ULL   // EL0 no access
#define AP_EL0RW   0x40ULL   // EL0+EL1 RW
#define AP_RO_ALL  0xC0ULL   // read-only, both ELs
#define UXN        (1ULL<<54)
#define PXN        (1ULL<<53)
#define TABLE      3ULL
#define PAGE       3ULL
#define BLOCK      1ULL

static const char *rstr(hv_return_t r){switch((uint32_t)r){case 0:return"HV_SUCCESS";
  case 0xfae94007:return"HV_DENIED";case 0xfae94003:return"HV_BAD_ARGUMENT";
  case 0xfae94004:return"HV_ILLEGAL_GUEST_STATE";default:return"OTHER";}}

// EL0 test program (see plan in header comment; results in x4/x7/x10/x11).
static const uint32_t code[] = {
    0xD2980021,             // movz x1, #0xc001
    0xF2A00021,             // movk x1, #0x1, lsl #16      => x1 = 0x1c001 (unaligned, page region)
    0xD28EF102,             // movz x2, #0x7788
    0xF2AAACC2,             // movk x2, #0x5566, lsl #16
    0xF2C66882,             // movk x2, #0x3344, lsl #32
    0xF2E22442,             // movk x2, #0x1122, lsl #48   => x2 = 0x1122334455667788
    0xF9000022,             // str  x2, [x1]               (unaligned store)
    0xF9400023,             // ldr  x3, [x1]
    0xEB03005F,             // cmp  x2, x3
    0x9A9F17E4,             // cset x4, eq                 => x4: page-region unaligned RW ok
    0xD2800025,             // movz x5, #0x1
    0xF2A08005,             // movk x5, #0x400, lsl #16    => x5 = 0x4000001 (unaligned, block region)
    0xF90000A2,             // str  x2, [x5]
    0xF94000A6,             // ldr  x6, [x5]
    0xEB06005F,             // cmp  x2, x6
    0x9A9F17E7,             // cset x7, eq                 => x7: block-region unaligned RW ok
    0xAA0103E8,             // mov  x8, x1
    0xD2800009,             // movz x9, #0
    0xDAC10128,             // pacia x8, x9
    0xAA0803EB,             // mov  x11, x8                => x11: signed pointer
    0xDAC11128,             // autia x8, x9
    0xEB01011F,             // cmp  x8, x1
    0x9A9F17EA,             // cset x10, eq                => x10: PAC sign/auth round-trip ok
    0xD288000D,             // movz x13, #0x4000
    0xF2A0800D,             // movk x13, #0x400, lsl #16   => x13 = 0x4004000 (file-backed page)
    0xF94001AC,             // ldr  x12, [x13]             => x12: first 8 bytes of the DSC file
    0xD4000021,             // svc  #1                     => trampoline => hvc => VMM
};

static uint64_t g_file8; // first 8 bytes of the mapped DSC header file (host view)

static int run_phase(const char *name, void *low, int mmu_on, int enia, int set_keys) {
    hv_vcpu_t vcpu; hv_vcpu_exit_t *exit;
    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    hv_return_t r = hv_vcpu_create(&vcpu, &exit, cfg);
    if (r) { printf("[%s] hv_vcpu_create -> %s\n", name, rstr(r)); return 2; }

    memset((char*)low + DATA_IPA, 0, 0x4000); // fresh data page per phase

    uint64_t sctlr = 0x30d00800ULL;                   // M0/M1 reset-ish baseline
    if (mmu_on) sctlr |= 1 /*M*/ | 4 /*C*/ | 0x1000 /*I*/;
    if (enia)   sctlr |= 0x80000000ULL /*EnIA*/;
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_SCTLR_EL1, sctlr);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_MAIR_EL1, 0xFFULL);        // attr0 = Normal WBWA
    // T0SZ=28 (36-bit VA), IRGN0/ORGN0=WBWA, SH0=inner, TG0=16K, EPD1=1, IPS=36-bit
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_TCR_EL1, 0x10080B51CULL);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_TTBR0_EL1, L2_IPA);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_VBAR_EL1, VEC_IPA);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_SP_EL0, LOW_LEN);
    if (set_keys) {
        r  = hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_APIAKEYLO_EL1, 0x5245545241434531ULL);
        r |= hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_APIAKEYHI_EL1, 0x4D325350494B4559ULL);
        printf("[%s] set APIA key -> %s\n", name, rstr(r));
    }
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0x0);          // EL0t, DAIF clear
    hv_vcpu_set_reg(vcpu, HV_REG_PC, CODE_IPA);

    uint64_t esr2 = 0, esr1 = 0, x[14] = {0};
    for (;;) {
        r = hv_vcpu_run(vcpu);
        if (r) { printf("[%s] hv_vcpu_run -> %s\n", name, rstr(r)); hv_vcpu_destroy(vcpu); return 2; }
        if (exit->reason == HV_EXIT_REASON_EXCEPTION) break;   // vtimer/canceled: ignore
    }
    esr2 = exit->exception.syndrome;
    hv_vcpu_get_sys_reg(vcpu, HV_SYS_REG_ESR_EL1, &esr1);
    for (int i = 0; i < 14; i++) hv_vcpu_get_reg(vcpu, HV_REG_X0 + i, &x[i]);

    uint32_t ec2 = (esr2 >> 26) & 0x3f, ec1 = (esr1 >> 26) & 0x3f, dfsc = esr1 & 0x3f;
    printf("[%s] EC_EL2=0x%02x  ESR_EL1=0x%08llx (EC=0x%02x%s)\n", name, ec2, esr1, ec1,
           ec1==0x15?" SVC":ec1==0x24?" DATA-ABORT":"");
    printf("[%s] x4(page-unaligned)=%llu x7(block-unaligned)=%llu x10(pac-roundtrip)=%llu\n",
           name, x[4], x[7], x[10]);
    printf("[%s] x11(signed ptr)=0x%016llx  raw=0x1c001  pac-bits-%s\n",
           name, x[11], x[11] != 0x1c001 ? "ENGAGED" : "identity");
    if (mmu_on) printf("[%s] x12(file-backed read)=0x%016llx expect 0x%016llx %s\n",
           name, x[12], g_file8, x[12]==g_file8 ? "(DSC readable in guest)" : "(MISMATCH)");

    int ok;
    if (mmu_on) ok = (ec2==0x16) && (ec1==0x15) && x[4]==1 && x[7]==1 && x[10]==1
                     && x[12]==g_file8 && (enia ? x[11]!=0x1c001 : x[11]==0x1c001);
    else        ok = (ec2==0x16) && (ec1==0x24) && (dfsc==0x21); // alignment fault on Device mem
    printf("[%s] => %s\n\n", name, ok ? "PASS" : "FAIL");
    hv_vcpu_destroy(vcpu);
    return ok ? 0 : 1;
}

int main(void) {
    hv_return_t r = hv_vm_create(NULL);
    printf("hv_vm_create -> %s\n", rstr(r));
    if (r) return 1;

    void *low = mmap(NULL, LOW_LEN, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE, -1, 0);
    void *blk = mmap(NULL, 0x4000,  PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE, -1, 0);
    r  = hv_vm_map(low, 0, LOW_LEN, HV_MEMORY_READ|HV_MEMORY_WRITE|HV_MEMORY_EXEC);
    r |= hv_vm_map(blk, BLOCK_IPA, 0x4000, HV_MEMORY_READ|HV_MEMORY_WRITE|HV_MEMORY_EXEC);
    printf("hv_vm_map    -> %s\n", rstr(r));

    // DSC header exposed to the guest at BLOCK_IPA+16K (inside the L2 block's VA range).
    // SAFETY: we COPY the file bytes into an ANONYMOUS page and map THAT. Mapping a
    // file-backed page directly with hv_vm_map is FATAL on macOS 26 (SPTM):
    // VIOLATION_ILLEGAL_MAPPING_TYPE hard-panics the whole machine (verified the hard way,
    // 2026-07-06). The loader must always stage DSC bytes through anon guest memory.
    void *dscpg = mmap(NULL, 0x4000, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE, -1, 0);
    FILE *f = fopen("/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e", "r");
    size_t got = f ? fread(dscpg, 1, 0x4000, f) : 0;
    if (f) fclose(f);
    hv_return_t rf = hv_vm_map(dscpg, BLOCK_IPA + 0x4000, 0x4000, HV_MEMORY_READ|HV_MEMORY_WRITE);
    g_file8 = got ? *(uint64_t*)dscpg : 0;
    printf("hv_vm_map(anon page holding %zu DSC bytes) -> %s  first8=0x%016llx (\"%.8s\")\n",
           got, rstr(rf), g_file8, got ? (char*)dscpg : "????????");

    // EL1 vectors: every slot hvc #0.
    for (int s = 0; s < 16; s++) *(uint32_t*)((char*)low + VEC_IPA + s*0x80) = 0xD4000002;
    memcpy((char*)low + CODE_IPA, code, sizeof(code));

    // Stage-1 identity tables. Start level 2 (T0SZ=28, TG0=16K): 2048 x 32 MiB.
    uint64_t *l2 = (uint64_t*)((char*)low + L2_IPA), *l3 = (uint64_t*)((char*)low + L3_IPA);
    l2[0] = L3_IPA | TABLE;                                     // first 32 MiB via L3
    l2[2] = BLOCK_IPA | ATTRS | AP_EL0RW | UXN | PXN | BLOCK;   // [64,96) MiB as one block
    l3[VEC_IPA  >> 14] = VEC_IPA  | ATTRS | AP_EL1RW | UXN       | PAGE; // EL1-exec vectors
    l3[L2_IPA   >> 14] = L2_IPA   | ATTRS | AP_EL1RW | UXN | PXN | PAGE;
    l3[L3_IPA   >> 14] = L3_IPA   | ATTRS | AP_EL1RW | UXN | PXN | PAGE;
    l3[CODE_IPA >> 14] = CODE_IPA | ATTRS | AP_RO_ALL      | PXN | PAGE; // EL0-exec code
    l3[DATA_IPA >> 14] = DATA_IPA | ATTRS | AP_EL0RW | UXN | PXN | PAGE;

    int rc = 0;
    rc |= run_phase("A mmu-on enia=1 keys", low, 1, 1, 1);
    rc |= run_phase("B mmu-on enia=0     ", low, 1, 0, 0);
    rc |= run_phase("D mmu-off (control) ", low, 0, 0, 0);

    hv_vm_destroy();
    printf(rc ? "=> SOME PHASE FAILED\n" : "=> ALL PHASES PASS: MMU-on identity map + Normal memory + PAC verified\n");
    return rc;
}
