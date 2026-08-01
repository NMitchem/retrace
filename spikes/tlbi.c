// tlbi.c — M9 go/no-go: can the GUEST invalidate its own stage-1 TLB entries for us?
// retrace hand-edits live stage-1 page tables (set_region_exec) but the VMM cannot issue a guest
// TLBI, so today a data->code flip is only sound on a block the guest never translated. jq's dyld
// breaks that: it MAP_FIXED-exec-maps __TEXT into a reservation it has already touched.
// Answers, empirically, on this OS/silicon:
//   F1: does `tlbi vmalle1` execute at guest EL1 without trapping to EL2?
//   F2: WITHOUT a TLBI, does a data->code leaf flip leave a stale entry (execute faults)?
//   F3: WITH the TLBI, does execution from the flipped page succeed?
// F2 is the control: if execution succeeds even without the flush, the guard was over-conservative
// and that is itself the finding.
//
// DEVIATION FROM THE BRIEF (documented per task-1 instructions): the brief's TCR_EL1 literal
// (0x8000210080B511, "T0SZ=17, TG0=16K") is a 47-bit-VA, THREE-level (TTBR0->L1->L2->L3) config,
// but the brief's table-build code only constructs a TWO-level table (start-level L2 -> L3) — a
// mismatch that would fault on the very first guest access. The TLBI question is independent of
// VA size, so instead of reproducing retrace's 47-bit VA, this spike borrows the known-working
// two-level config from spikes/m2spike.c verbatim: T0SZ=28 (36-bit VA), start level 2,
// TCR_EL1=0x10080B51C. Everything else (IPA layout, attributes, stub encodings, phase structure)
// is the brief's snippet unchanged.
//
// SAFETY: every phase ends at a terminal `hvc #0`; still run under the external perl
// process-group timeout (no `timeout` binary on this platform).
//   clang -O2 -o tlbi tlbi.c -framework Hypervisor
//   codesign -s - -f --entitlements ent.plist tlbi
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./tlbi
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define PG          0x4000ULL
#define VEC_IPA     0x04000ULL      // EL1 vector table (EL1-exec)
#define L2_IPA      0x08000ULL      // start-level table
#define L3_IPA      0x0C000ULL      // L3 covering the first 32 MiB
#define CODE_IPA    0x10000ULL      // EL0 guest code
#define STUB_IPA    0x14000ULL      // the EL1 TLBI stub (EL1-exec)
#define TEST_IPA    0x18000ULL      // the page flipped from data to code

// Stage-1 leaf attributes, mirrored from crates/retrace-box/src/lib.rs.
#define A_COMMON    (0x3ULL | (0ULL<<2) | (3ULL<<8) | (1ULL<<10))  // page desc, attr0, inner-share, AF
#define UXN         (1ULL<<54)
#define PXN         (1ULL<<53)
#define ATTR_DATA   (A_COMMON | 0x40  | UXN | PXN)   // RW both ELs, never executable
#define ATTR_CODE   (A_COMMON | 0xC0  | PXN)         // RO, EL0-exec (UXN clear)
#define ATTR_TRAMP  (A_COMMON | 0x80  | UXN)         // RO, EL1-exec (PXN clear)

static hv_vcpu_t vcpu; static hv_vcpu_exit_t *vexit;
static uint64_t *l3;
static uint32_t g_ec1; // last run's ESR_EL1 EC — see note below run_once.

static uint64_t rg(hv_reg_t r){ uint64_t v=0; hv_vcpu_get_reg(vcpu,r,&v); return v; }
static uint64_t rs(hv_sys_reg_t r){ uint64_t v=0; hv_vcpu_get_sys_reg(vcpu,r,&v); return v; }

// Run once; return the EL2 exception class.
//
// IMPORTANT CAVEAT (why this also reads ESR_EL1): every one of the 16 EL1 vector slots is
// an unconditional `hvc #0`, so ANY EL0 trap (the deliberate `hvc` trigger, which is
// UNDEFINED at EL0, but ALSO a genuine instruction-abort on a stale/faulting fetch) is
// funneled through the SAME vector and re-emerges as the SAME ESR_EL2 EC2=0x16 exit at the
// SAME pc (VEC_IPA+0x400+4). EC2/pc alone therefore CANNOT distinguish "blr succeeded, hit
// the trailing hvc" from "blr's fetch faulted, routed through the same vector" — both look
// identical at EL2. ESR_EL1 (the real LOCAL EL0->EL1 trap cause, latched before the
// vector's own hvc fires) is the only reliable discriminator: EC1=0x00 (Unknown reason —
// the undefined `hvc` at EL0) means the code genuinely reached the trigger instruction;
// EC1=0x20 (Instruction Abort from a lower EL) means it never got there. F2 below relies on
// this, not on EC2.
static uint32_t run_once(const char *tag){
    hv_vcpu_run(vcpu);
    uint64_t esr2 = vexit->exception.syndrome;
    uint32_t ec2 = (uint32_t)((esr2>>26)&0x3f);
    uint64_t esr1 = rs(HV_SYS_REG_ESR_EL1);
    g_ec1 = (uint32_t)((esr1>>26)&0x3f);
    printf("[%s] reason=%u EC2=0x%02x pc=0x%llx far=0x%llx | ESR_EL1 EC1=0x%02x esr1=0x%llx\n",
           tag, vexit->reason, ec2, rg(HV_REG_PC), vexit->exception.virtual_address, g_ec1, esr1);
    return ec2;
}

static void set_leaf(uint64_t ipa, uint64_t attr){ l3[ipa/PG] = (ipa & ~(PG-1)) | attr; }

int main(void){
    if (hv_vm_create(NULL)) { printf("no vm\n"); return 1; }

    // ---- guest memory ----
    void *mem = mmap(NULL, 0x20000, PROT_READ|PROT_WRITE, MAP_ANON|MAP_PRIVATE, -1, 0);
    memset(mem, 0, 0x20000);
    hv_vm_map(mem, 0, 0x20000, HV_MEMORY_READ|HV_MEMORY_WRITE|HV_MEMORY_EXEC);
    uint8_t *base = (uint8_t*)mem;

    // EL1 vectors: every slot is `hvc #0`, so any guest fault lands at EL2 identifiably.
    for (int i=0;i<16;i++) ((uint32_t*)(base+VEC_IPA))[i*0x80/4] = 0xD4000002;

    // L2: one entry pointing at L3, covering the first 32 MiB.
    uint64_t *l2 = (uint64_t*)(base+L2_IPA);
    l2[0] = L3_IPA | 0x3;                       // table descriptor
    l3 = (uint64_t*)(base+L3_IPA);
    for (uint64_t i=0;i<2048;i++) l3[i] = (i*PG) | ATTR_DATA;
    set_leaf(CODE_IPA, ATTR_CODE);
    set_leaf(VEC_IPA,  ATTR_TRAMP);
    set_leaf(STUB_IPA, ATTR_TRAMP);             // EL1-exec: TLBI is an EL1 instruction
    set_leaf(TEST_IPA, ATTR_DATA);              // starts as DATA — the whole point

    // EL0 guest code at CODE_IPA:
    //   build x1 = TEST_IPA (0x18000) via movz/movk
    //   read TEST_IPA (forces a stage-1 walk + TLB fill as DATA), then hvc
    //   blr into TEST_IPA, then hvc
    uint32_t code[] = {
        0xD2A00021,   // movz x1, #1, lsl #16   -> 0x10000
        0xF2900001,   // movk x1, #0x8000       -> 0x18000
        0xF9400022,   // ldr  x2, [x1]         -> the DATA read that fills the TLB
        0xD4000002,   // hvc #0                -> phase 1 done
        0xD63F0020,   // blr  x1               -> execute from the flipped page
        0xD4000002,   // hvc #0                -> phase 3 done (only if the blr worked)
    };
    memcpy(base+CODE_IPA, code, sizeof(code));

    // The payload the flipped page must run: `movz x0,#0x5A ; ret`.
    uint32_t payload[] = { 0xD2800B40, 0xD65F03C0 };
    memcpy(base+TEST_IPA, payload, sizeof(payload));

    // The EL1 TLBI stub (verified encodings — `clang -c` + `otool -t`):
    //   tlbi vmalle1 ; dsb ish ; isb ; hvc #0
    uint32_t stub[] = { 0xd508871f, 0xd5033b9f, 0xd5033fdf, 0xd4000002 };
    memcpy(base+STUB_IPA, stub, sizeof(stub));

    // ---- vCPU: MMU ON ----
    // TCR_EL1: T0SZ=28 (36-bit VA), TG0=16K, start level 2 — the known-working config from
    // spikes/m2spike.c, not the brief's mismatched T0SZ=17/two-level combo (see header comment).
    hv_vcpu_config_t cfg = hv_vcpu_config_create();
    hv_vcpu_create(&vcpu,&vexit,cfg);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_VBAR_EL1, VEC_IPA);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_MAIR_EL1, 0xFF);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_TCR_EL1,  0x10080B51CULL); // m2spike: T0SZ=28, TG0=16K, start level 2
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_TTBR0_EL1, L2_IPA);
    hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_SCTLR_EL1, 0x30d00800ULL | 1ULL); // M=1 => MMU ON

    // ---- Phase 1: EL0 reads TEST_IPA as DATA (fills the TLB with a UXN entry) ----
    hv_vcpu_set_reg(vcpu, HV_REG_PC, CODE_IPA);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);                    // EL0t
    uint32_t ec = run_once("phase1-read");
    printf("phase1: %s\n", ec==0x16 ? "read OK (entry now cached as DATA)" : "UNEXPECTED");

    // ---- Flip the leaf DATA -> CODE, WITHOUT any TLBI ----
    set_leaf(TEST_IPA, ATTR_CODE);

    // ---- F2 (control): execute WITHOUT the flush ----
    // x0 is a second, independent signal: only the flipped page's payload (`movz x0,#0x5a`)
    // sets it. Sentinel it to something payload can't produce, so a fault (x0 unchanged)
    // and a genuine execution (x0==0x5a) are unambiguous even setting EC1 aside.
    hv_vcpu_set_reg(vcpu, HV_REG_X0, 0xdead);
    hv_vcpu_set_reg(vcpu, HV_REG_PC, CODE_IPA+0x10);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);
    ec = run_once("F2-control");
    uint64_t f2_x0 = rg(HV_REG_X0);
    // EC2 is NOT the discriminator here (see run_once's caveat) — ESR_EL1's EC1 is:
    // 0x00 (Unknown reason == the deliberate undefined `hvc` at EL0) means the blr's fetch
    // succeeded and control reached the trigger; 0x20/0x21 (Instruction Abort) means it
    // faulted on the stale entry before ever reaching the trigger.
    int stale = (g_ec1 == 0x20 || g_ec1 == 0x21);
    printf("F2: without TLBI -> ESR_EL1 EC1=0x%02x, x0=0x%llx (sentinel 0xdead, payload sets 0x5a) -> %s\n",
        g_ec1, f2_x0, stale
        ? "FAULTED (stale entry is REAL; the guard's premise holds)"
        : "EXECUTED ANYWAY (no stale entry — the guard was over-conservative!)");
    if (!stale && f2_x0 != 0x5a)
        printf("F2: WARNING — EC1 says no fault but x0 wasn't set by the payload; signals disagree, investigate.\n");

    // ---- F1 + F3: run the EL1 stub, then execute again ----
    hv_vcpu_set_reg(vcpu, HV_REG_PC, STUB_IPA);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0x3C5);                // EL1h, DAIF masked
    ec = run_once("F1-tlbi");
    printf("F1: tlbi vmalle1 at EL1 -> %s\n",
           ec==0x16 ? "EXECUTED, reached its hvc" : "TRAPPED/FAULTED (TLBI unavailable!)");

    hv_vcpu_set_reg(vcpu, HV_REG_PC, CODE_IPA+0x10);
    hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0);
    ec = run_once("F3-exec");
    printf("F3: after TLBI, execute flipped page -> %s (x0=0x%llx, want 0x5a)\n",
           ec==0x16 ? "SUCCEEDED" : "STILL FAULTS", rg(HV_REG_X0));

    printf("\nVERDICT: %s\n",
        (ec==0x16 && rg(HV_REG_X0)==0x5a)
          ? "GO — guest-side TLBI works; M9 Tasks 2-3 proceed as designed."
          : "NO-GO — fall back to spec risk R1 (pre-promote reservations).");

    hv_vcpu_destroy(vcpu); hv_vm_destroy();
    return 0;
}
