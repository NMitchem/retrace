// pacsign.c — prove the "guest signing oracle" for the M2 cache re-signing plan.
//
// The macOS shared cache's arm64e auth pointers are PAC-signed with the HOST process's
// per-process keys, which our fresh-keyed guest VM cannot authenticate (the task-9b wall:
// EC=0x1C FPAC fault). Plan: re-sign each cache auth pointer with the GUEST's FIXED keys by
// executing pac* INSIDE the guest (guest keys live in APIAKEY*_EL1 etc.), instead of
// reimplementing Apple's QARMA on the host.
//
// Phase 1 (oracle): set the guest's fixed PAC keys (identical constants to retrace-box
//   PAC_KEYS), enable PAC for all four families, and for (pointer, modifier) run
//   pac{ia,ib,da,db} then aut{ia,ib,da,db}, verifying signed!=raw (PAC engaged), aut* with the
//   SAME modifier recovers the raw ptr (round-trip), signatures differ across keys, and a
//   DIFFERENT modifier yields a DIFFERENT signature (modifier is load-bearing). Covers a DATA
//   key (DA/DB) and NONZERO modifiers, extending m2spike's IA-only proof. Reaches HVC (EC=0x16).
// Phase 2 (negative control): autia the phase-1 IA signature with the WRONG modifier and show
//   the CPU takes an authentication-failure fault — i.e. a wrong signature does NOT silently
//   pass; our re-signing must reproduce dyld's (ptr,key,diversifier) exactly.
//
// SAFETY: bounded run loop + external perl process-group timeout (see build cmd). MMU is OFF;
// only anonymous guest memory is mapped (never a file-backed page).
//   clang -O2 -o pacsign pacsign.c -framework Hypervisor
//   codesign -s - -f --entitlements ent.plist pacsign
//   perl -e '$p=fork;if(!$p){setpgrp;exec@ARGV or exit 127}$SIG{ALRM}=sub{kill"-KILL",$p;exit 124};alarm 15;wait;exit($?>>8)' ./pacsign
#include <Hypervisor/Hypervisor.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define GUEST_PA 0x10000000ULL
#define VEC_PA   0x09000000ULL   // EL1 vector table; every slot does hvc so faults trap to EL2

static const char *rstr(hv_return_t r){switch((uint32_t)r){case 0:return"HV_SUCCESS";
  case 0xfae94007:return"HV_DENIED";case 0xfae94003:return"HV_BAD_ARGUMENT";
  case 0xfae94004:return"HV_ILLEGAL_GUEST_STATE";default:return"OTHER";}}

// Guest keys: byte-identical to retrace-box `PAC_KEYS` (crates/retrace-box/src/lib.rs).
static const struct { hv_sys_reg_t r; uint64_t v; } KEYS[] = {
  {HV_SYS_REG_APIAKEYLO_EL1,0x5245545241434531ULL},{HV_SYS_REG_APIAKEYHI_EL1,0x4D325350494B4559ULL},
  {HV_SYS_REG_APIBKEYLO_EL1,0x0badc0de0badc0deULL},{HV_SYS_REG_APIBKEYHI_EL1,0xfeedfacefeedfaceULL},
  {HV_SYS_REG_APDAKEYLO_EL1,0x1111111122222222ULL},{HV_SYS_REG_APDAKEYHI_EL1,0x3333333344444444ULL},
  {HV_SYS_REG_APDBKEYLO_EL1,0x5555555566666666ULL},{HV_SYS_REG_APDBKEYHI_EL1,0x7777777788888888ULL},
};

// Phase 1: sign+auth all four families; also sign IA with a 2nd modifier (x5) for sensitivity.
// Inputs: x0=ptr, x1..x4 modifiers, x5=2nd IA modifier. Results in x10..x19.
static const uint32_t code_oracle[] = {
  0xAA0003EA,            // mov x10,x0            ; save raw ptr
  0xDAC10020,            // pacia x0,x1
  0xAA0003EB,            // mov x11,x0            ; IA signed (mod x1)
  0xDAC11020,            // autia x0,x1
  0xAA0003EC,            // mov x12,x0            ; IA recovered
  0xAA0A03E0,            // mov x0,x10
  0xDAC10840,            // pacda x0,x2
  0xAA0003ED,            // mov x13,x0            ; DA signed
  0xDAC11840,            // autda x0,x2
  0xAA0003EE,            // mov x14,x0            ; DA recovered
  0xAA0A03E0,            // mov x0,x10
  0xDAC10460,            // pacib x0,x3
  0xAA0003EF,            // mov x15,x0            ; IB signed
  0xDAC11460,            // autib x0,x3
  0xAA0003F0,            // mov x16,x0            ; IB recovered
  0xAA0A03E0,            // mov x0,x10
  0xDAC10C80,            // pacdb x0,x4
  0xAA0003F1,            // mov x17,x0            ; DB signed
  0xDAC11C80,            // autdb x0,x4
  0xAA0003F2,            // mov x18,x0            ; DB recovered
  0xAA0A03E0,            // mov x0,x10
  0xDAC100A0,            // pacia x0,x5           ; IA signed with a DIFFERENT modifier
  0xAA0003F3,            // mov x19,x0            ; expect x19 != x11
  0xD4000002,            // hvc #0
};

// Phase 2: authenticate a valid IA signature (in x0) with the WRONG modifier (x5) -> must fault.
static const uint32_t code_badauth[] = {
  0xDAC110A0,            // autia x0,x5           ; wrong modifier => auth failure
  0xD4000002,            // hvc #0                ; (should NOT be reached under FEAT_FPAC)
};

// Configure a fresh vCPU (keys, TCR, PAC on, MMU off), load `code`, set x0..x5, run bounded.
// Returns EC; fills out[0..23] with X0..X23. `far`/`pc` get the fault address / ELR.
static uint32_t run_guest(const uint32_t *code, size_t nbytes, const uint64_t in[6],
                          uint64_t out[24], uint64_t *far, uint64_t *esr1){
  static void *buf, *vec;
  if(!buf){ buf = mmap(NULL,0x4000,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
            vec = mmap(NULL,0x4000,PROT_READ|PROT_WRITE,MAP_ANON|MAP_PRIVATE,-1,0);
            for(int s=0;s<16;s++) *(uint32_t*)((char*)vec + s*0x80) = 0xD4000002; // hvc in every slot
            hv_vm_map(vec, VEC_PA, 0x4000, HV_MEMORY_READ|HV_MEMORY_WRITE|HV_MEMORY_EXEC);
            hv_vm_map(buf, GUEST_PA, 0x4000, HV_MEMORY_READ|HV_MEMORY_WRITE|HV_MEMORY_EXEC); }
  memcpy(buf, code, nbytes);
  hv_vcpu_t vcpu; hv_vcpu_exit_t *exit;
  hv_vcpu_create(&vcpu, &exit, hv_vcpu_config_create());
  for (unsigned i=0;i<sizeof KEYS/sizeof KEYS[0];i++) hv_vcpu_set_sys_reg(vcpu, KEYS[i].r, KEYS[i].v);
  hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_TCR_EL1, 0x10080B51CULL); // T0SZ=28,16K,TBI0=0
  hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_MAIR_EL1, 0xFFULL);
  hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_VBAR_EL1, VEC_PA);        // guest faults -> hvc -> EL2
  uint64_t sctlr = 0x30d00800ULL | (1ULL<<31)|(1ULL<<30)|(1ULL<<27)|(1ULL<<13); // EnIA/IB/DA/DB, MMU off
  hv_vcpu_set_sys_reg(vcpu, HV_SYS_REG_SCTLR_EL1, sctlr);
  hv_vcpu_set_reg(vcpu, HV_REG_CPSR, 0x3c5);
  hv_vcpu_set_reg(vcpu, HV_REG_PC, GUEST_PA);
  for (int i=0;i<6;i++) hv_vcpu_set_reg(vcpu, HV_REG_X0+i, in[i]);
  uint32_t ec=0xFF;
  for (int i=0;i<64;i++){
    if (hv_vcpu_run(vcpu)) break;
    if (exit->reason==HV_EXIT_REASON_EXCEPTION){ ec=(exit->exception.syndrome>>26)&0x3f;
      *far = exit->exception.virtual_address; break; }
  }
  hv_vcpu_get_sys_reg(vcpu, HV_SYS_REG_ESR_EL1, esr1);           // guest-side fault syndrome
  for (int i=0;i<24;i++) hv_vcpu_get_reg(vcpu, HV_REG_X0+i, &out[i]);
  hv_vcpu_destroy(vcpu);
  return ec;
}

int main(void){
  hv_return_t r = hv_vm_create(NULL);
  printf("hv_vm_create -> %s\n", rstr(r));
  if (r) return 1;

  uint64_t in[6] = { 0x0000000180ccb568ULL,   // ptr: a realistic slid shared-cache VA
                     0x1122334455667788ULL,   // x1 IA mod
                     0x00000000DEADBEEFULL,   // x2 DA mod (nonzero)
                     0x000000000000ABCDULL,   // x3 IB mod
                     0xFFFFFFFF0BADF00DULL,   // x4 DB mod
                     0x9999999999999999ULL }; // x5 2nd IA mod / wrong mod
  uint64_t x[24], far=0, esr1=0;
  uint32_t ec = run_guest(code_oracle, sizeof code_oracle, in, x, &far, &esr1);

  printf("\n== Phase 1: guest signing oracle ==\n");
  printf("exit EC=0x%02x %s\n", ec, ec==0x16?"HVC (clean, reached VMM)":"UNEXPECTED");
  printf("raw ptr            x10=0x%016llx\n", x[10]);
  printf("IA signed=0x%016llx recovered=0x%016llx %s\n", x[11],x[12], x[12]==x[10]?"ROUNDTRIP":"FAIL");
  printf("DA signed=0x%016llx recovered=0x%016llx %s\n", x[13],x[14], x[14]==x[10]?"ROUNDTRIP":"FAIL");
  printf("IB signed=0x%016llx recovered=0x%016llx %s\n", x[15],x[16], x[16]==x[10]?"ROUNDTRIP":"FAIL");
  printf("DB signed=0x%016llx recovered=0x%016llx %s\n", x[17],x[18], x[18]==x[10]?"ROUNDTRIP":"FAIL");
  printf("IA w/ 2nd modifier x19=0x%016llx %s\n", x[19], x[19]!=x[11]?"DIFFERENT SIG (modifier matters)":"SAME(!)");
  int engaged   = x[11]!=x[10]&&x[13]!=x[10]&&x[15]!=x[10]&&x[17]!=x[10];
  int distinct  = x[11]!=x[13]&&x[11]!=x[15]&&x[13]!=x[17];
  int roundtrip = x[12]==x[10]&&x[14]==x[10]&&x[16]==x[10]&&x[18]==x[10];
  int modsens   = x[19]!=x[11];
  int p1 = (ec==0x16)&&engaged&&distinct&&roundtrip&&modsens;
  printf("engaged=%d keys-distinct=%d roundtrip=%d modifier-sensitive=%d => %s\n",
         engaged,distinct,roundtrip,modsens, p1?"PASS":"FAIL");

  printf("\n== Phase 2: negative control (wrong modifier must fault) ==\n");
  uint64_t in2[6]={ x[11], 0,0,0,0, in[5] };   // x0 = valid IA sig; x5 = WRONG modifier
  uint64_t y[24]; uint64_t esr1b=0;
  uint32_t ec2 = run_guest(code_badauth, sizeof code_badauth, in2, y, &far, &esr1b);
  uint32_t gec = (uint32_t)((esr1b>>26)&0x3f);   // guest-side EC for the fault it took
  int faulted = (gec==0x1C);                     // EC_PAC_FAIL (FEAT_FPAC authentication failure)
  printf("trapped to VMM EC=0x%02x via guest vector; guest ESR_EL1=0x%08llx EC=0x%02x %s\n",
         ec2, esr1b, gec, gec==0x1C?"(FPAC auth-failure)":"");
  printf("=> %s: wrong (ptr,key,modifier) => FEAT_FPAC fault (EC=0x1C) — same wall as task-9b;\n"
         "        re-signing must reproduce dyld's exact (target,key,diversifier)\n",
         faulted?"PASS":"FAIL");

  int ok = p1 && faulted;
  printf("\n=> %s\n", ok?"GUEST SIGNING ORACLE VERIFIED (sign+auth all 4 families; wrong sig faults)":"SPIKE FAILED");
  hv_vm_destroy();
  return ok?0:1;
}
