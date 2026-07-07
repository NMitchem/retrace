# Verification spikes

Throwaway probes that empirically verified the load-bearing HVF claims the design rests
on, run on the actual target OS before committing to the architecture.

**Environment:** Apple Silicon, macOS 26.4.1 (build 25E253), Darwin 25.4.0,
`kern.hv_support: 1`, Command Line Tools SDK. Run as non-root (uid 501), **SIP enabled**.

## Build & run

```sh
clang -o hvprobe hvprobe.c -framework Hypervisor -framework Foundation
codesign -s - -f --entitlements ent.plist hvprobe
./hvprobe

clang -o hvspike hvspike.c -framework Hypervisor
codesign -s - -f --entitlements ent.plist hvspike
./hvspike
```

## `hvprobe.c` — platform capability probe

Verified, both directions:

- **Entitlement is freely ad-hoc-signable.** No entitlement → `hv_vm_create` returns
  `HV_DENIED (0xfae94007)`. Ad-hoc signed with `com.apple.security.hypervisor` (no Apple
  account, no provisioning profile, non-root, SIP on) → `HV_SUCCESS`.
- **No instruction counter.** `ID_AA64DFR0_EL1` reports `PMUVer=0x0` (no PMUv3); the SDK
  headers contain zero `HV_SYS_REG_PM*` registers. Only `hv_vcpu_get_exec_time` (a time
  meter) exists. => instruction-exact positioning must be software single-step.
- **HW debug slots:** 6 breakpoints / 4 watchpoints.
- **Stage-2 RWX** of a plain `PROT_READ|PROT_WRITE` `MAP_ANON` buffer (no `MAP_JIT`,
  no `PROT_EXEC`) → `HV_SUCCESS`.
- **Default IPA:** 36 bits (64 GiB). `set_trap_debug_exceptions` → `HV_SUCCESS`.

## `hvspike.c` — the core vCPU loop, in miniature

Runs real guest instructions at EL1 (MMU off), traps out via `HVC`, decodes the syndrome:

```
guest: movz x0,#0x1234 ; add x0,x0,#1 ; hvc #0
=> hv_vcpu_run -> HV_SUCCESS
   exit.reason -> EXCEPTION
   ESR_EL2 EC=0x16 (EC_AA64_HVC)   # guest trap reached the VMM
   guest X0 = 0x1235               # guest executed natively
```

This is the trap-and-forward mechanism `retrace-box` is built on, proven end-to-end.

## `m2spike.c` — MMU-on, PAC, and shared-cache reachability (M2 loader spike)

Runs real EL0 guest code under **guest-built stage-1 page tables** (16 KiB granule,
`T0SZ=28`, start level 2, `MAIR` attr0 = Normal WBWA), three phases:

```
[A mmu-on enia=1] page-unaligned RW=1  block-unaligned RW=1  pac-roundtrip=1
                  signed ptr=0x54470c30_0001c001 (PAC bits ENGAGED)
                  DSC header read in-guest = "dyld_v1 " (matches host)
[B mmu-on enia=0] same, but PAC is identity (signing off is deterministic)
[D mmu-off ctrl ] same code faults: EC=0x24 data abort, alignment DFSC 0x21
=> MMU-on identity map + Normal memory + PAC verified
```

Establishes the M2 load-bearing claims: unaligned access needs MMU-on Normal memory
(phase D is the negative control), PAC keys set via `hv_vcpu_set_sys_reg` sign/auth
correctly in-guest, and the shared cache is readable through guest translation.

**SPTM safety finding (learned the hard way):** an earlier version mapped the cache file
into the guest with a **file-backed** `hv_vm_map`. On macOS 26 this **hard-panics the
machine** — `[SPTM] VIOLATION_ILLEGAL_MAPPING_TYPE`, an unrecoverable kernel reset. **All
guest memory must be anonymous; stage file bytes into anon pages (`pread`/`fread` then
map), never map a file page directly.** This spike and the M2 design now do exactly that.

## `dscprobe.c` — `DYLD_SHARED_REGION=private` host probe

Confirms dyld will map the shared cache itself (via ordinary syscalls the M1 recorder
handles) rather than joining the kernel-managed shared region:

```
$ ./dscprobe                          # &printf INSIDE kernel shared region  (exit 10)
$ DYLD_SHARED_REGION=private ./dscprobe # &printf OUTSIDE — PRIVATE mapping   (exit 20)
```

## Proven for M2; still open for later milestones

- MMU-on paging, PAC, and DSC reachability — **proven** (`m2spike.c`); `private` cache
  mapping — **proven** (`dscprobe.c`).
- **Full dyld startup to `main` under the oracle** — the M2 bring-up itself; not a spike.
- **Instruction-exact positioning, signals, threads** — M3+.
- **Memory-diff fidelity across the real syscall surface** — the long tail; M2 pays the
  four carried-forward recorder debts (clamp, `munmap`/`mprotect`, error ABI, raw `svc`).
