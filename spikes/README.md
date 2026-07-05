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

## Not yet proven here (M0/M1 spike targets)

- **EL0 `SVC` → guest EL1 (not the VMM).** These spikes issue `HVC` from EL1 directly.
  The claim that an EL0 `SVC` requires a `VBAR_EL1` `SVC→HVC` trampoline to reach the VMM
  is architectural (well-established) but not re-proven here. First M0 spike.
- **dyld shared cache loading on Tahoe** (the AppBox-hard-part) — unproven; risk #1.
- **Memory-diff fidelity across the real syscall surface** — the long tail; M1+.
