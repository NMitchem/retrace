# M2-mach Implementation Plan — mach_msg2 MIG kernel-RPC servicing

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Service `mach_msg2` (trap −47) MIG kernel RPCs against the guest address space so libmalloc's nano pointer-range reservation succeeds, then walk the remaining libSystem init empirically until `hello_dyn_e2e` records and replays byte-for-byte (un-ignored).

**Architecture:** A pure MIG codec module in `retrace-core` (register unpack, request decode, reply encode, routing) + one new dispatch arm in each of the record/replay loops that services `_kernelrpc_mach_vm_map` (msgh_id 4811) through the existing `Box_::guest_vm_map`, stubs the optional reclamation-buffer RPC (4822), forwards a decided read-only allowlist (200/206/3418), and fails loudly on everything else. Spec: `docs/superpowers/specs/2026-07-07-retrace-m2-mach-design.md` — read it before starting.

**Tech Stack:** Rust (workspace crates `retrace-core`, `retrace-box`, `retrace-trace`, `retrace-guest`, `retrace`), Hypervisor.framework via `hv-sys`, arm64 asm guests built by `retrace-guest/build.rs`.

## Global Constraints

- Branch: `m2-mach` (create from `main` before Task 1).
- All test runs: `cargo test --workspace -- --test-threads=1` (one VM per process on macOS; parallel VMs flake with `HV_BUSY`).
- `cargo clippy --workspace --all-targets -- -D warnings` must stay clean at every commit.
- Any binary that calls `hv_*` must be ad-hoc codesigned with `retrace.entitlements`. The `.cargo/config.toml` runner does this for cargo-invoked binaries; for a manually-run `target/.../retrace`, first: `codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/debug/retrace`.
- Manual VM runs must be bounded: `perl -e 'alarm 60; exec @ARGV' -- <cmd>` (a wedged vCPU otherwise hangs the shell).
- No `Date::now()`/randomness in tests (determinism deny-list; see existing `retrace-trace` test comment).
- Never fake a green: if a gate can't pass honestly, keep it `#[ignore]`d with an updated tracked reason and say so in the task report.
- Commit messages: `M2-mach tN: <what>` (house style, cf. `M2c t5: ...`).

**Trap-log decode cheat-sheet (used throughout):** `mach_msg2_trap` packs x0=data, x1=options, x2=bits|send_size≪32, x3=dest|reply≪32, x4=voucher|msgh_id≪32, x5=desc_count|rcv_name≪32, x6=rcv_size|priority≪32, x7=timeout. `RETRACE_TRACE=1` makes the record loop print every trap.

---

### Task 1: Widen trap capture to x0–x7 + trace format v3

`mach_msg2` has an 8-register ABI; today `Stop::Syscall` captures x0–x6 and pads x7=0 when forwarding (latent bug). Widening flows into the serialized `Event::Syscall`, so the trace version byte bumps 0x02→0x03.

**Files:**
- Modify: `crates/retrace-trace/src/lib.rs` (Event args, TRACE_MAGIC, `sample()` test)
- Modify: `crates/retrace-box/src/lib.rs:195` (Stop enum), `:1052` (capture loop), `:1159-1163` (forward_and_diff)
- Modify: `crates/retrace-core/src/lib.rs:29` (vm_map_args)

**Interfaces:**
- Produces: `Stop::Syscall { num: u64, args: [u64;8] }`, `Event::Syscall { .., args: [u64;8], .. }`, `Box_::forward_and_diff(&self, num: u64, args: [u64;8])`, `TRACE_MAGIC = *b"RT\x00\x03"`. All later tasks assume 8-wide args.

- [ ] **Step 1: Make the trace test demand 8 args + v3 (red).** In `crates/retrace-trace/src/lib.rs` `sample()`, change the Syscall line to:

```rust
Event::Syscall { num:3, args:[5,0x100000100,6,0,0,0,0,0], ret:6, err:false,
                 writes: vec![Region{ ipa:0x100000100, bytes: vec![9,9,9,9,9,9] }] },
```

Run: `cargo test -p retrace-trace -- --test-threads=1` — Expected: FAIL to compile (`expected an array with a size of 7, found one with a size of 8`).

- [ ] **Step 2: Widen the format.** In `crates/retrace-trace/src/lib.rs`:

```rust
    Syscall { num: u64, args: [u64;8], ret: u64, err: bool, writes: Vec<Region> },
```
```rust
pub const TRACE_MAGIC: [u8;4] = *b"RT\x00\x03"; // "RT" + format version 0x0003 (M2-mach: 8-wide args)
```

- [ ] **Step 3: Widen the box.** In `crates/retrace-box/src/lib.rs`:

```rust
pub enum Stop { Syscall { num: u64, args: [u64;8] }, Other { esr: u64 } }
```

Capture loop (line ~1052): `let mut args = [0u64;8];` (the `for (i, a) in args.iter_mut().enumerate()` loop is already width-generic).

`forward_and_diff` (line ~1159): signature `args: [u64;8]`, `let mut hargs = [0i64; 8];`, loop `for i in 0..8`, and replace the trailing shim-build

```rust
        let mut sa = [0u64; 8];
        for i in 0..7 { sa[i] = hargs[i] as u64; }
```
with
```rust
        let mut sa = [0u64; 8];
        for i in 0..8 { sa[i] = hargs[i] as u64; }
```
Also update that block's comment (`hargs` is `[i64;8]` (x0..x7); no more x7 padding).

- [ ] **Step 4: Widen retrace-core.** `fn vm_map_args(num: u64, args: &[u64; 8])` at `crates/retrace-core/src/lib.rs:29`.

- [ ] **Step 5: Verify no 7-wide site remains.** Run: `grep -rn "u64;7\|u64; 7" crates` — Expected: no output.

- [ ] **Step 6: Full suite + clippy.** Run: `cargo test --workspace -- --test-threads=1` — Expected: 43 passed, 1 ignored. Run: `cargo clippy --workspace --all-targets -- -D warnings` — Expected: clean.

- [ ] **Step 7: Commit.**
```bash
git add -A && git commit -m "M2-mach t1: 8-wide trap capture (x0-x7) + trace format v3

mach_msg2_trap has an 8-register ABI (x7 = timeout); Stop::Syscall captured
only x0-x6 and forward_and_diff padded x7=0. Widen Stop/Event/forward to
[u64;8]; bump TRACE_MAGIC to 0x0003 (Event::Syscall serialization changed)."
```

---

### Task 2: −47 golden-capture diagnostic + fixture file

Add a `RETRACE_TRACE` hexdump of `mach_msg2` send buffers (before forwarding) and kernel-reply bytes (after), run the blocked `record-dyn`, and commit the capture. Later tasks transcribe these authentic bytes into codec tests.

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (const + two diagnostic blocks in `record_box`)
- Create: `crates/retrace-core/tests/fixtures/mach_msg2_capture.txt`

**Interfaces:**
- Produces: `const MACH_MSG2: u64 = (-47i64) as u64;` in `retrace-core/src/lib.rs` (module scope, next to `MACH_VM_MAP`); the fixture file. Tasks 3–5 use both.

- [ ] **Step 1: Add the const** next to the other mach consts in `crates/retrace-core/src/lib.rs`:

```rust
const MACH_MSG2: u64 = (-47i64) as u64; // mach_msg2_trap(data, options, bits|send_size, dest|reply, voucher|id, desc|rcv_name, rcv_size|prio, timeout)
```

- [ ] **Step 2: Hexdump sends.** Inside `record_box`'s existing `if trace_log { if let Stop::Syscall { num, args } = &stop { ... } }` block, after the fd-1/2 echo, add:

```rust
                // M2-mach diagnostic: decode + hexdump mach_msg2 sends (golden capture for the codec).
                if *num == MACH_MSG2 {
                    let send_size = ((args[2] >> 32) as usize).min(256);
                    eprintln!("[mach_msg2] msgh_id={} dest={:#x} reply={:#x} options={:#x} bits={:#x} send_size={} rcv_size={}",
                        args[4] >> 32, args[3] & 0xffff_ffff, args[3] >> 32, args[1],
                        args[2] & 0xffff_ffff, args[2] >> 32, args[6] & 0xffff_ffff);
                    for (i, chunk) in b.read_guest(args[0], send_size).chunks(16).enumerate() {
                        eprintln!("  send+{:03x}: {}", i * 16,
                            chunk.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
                    }
                }
```

- [ ] **Step 3: Hexdump replies.** In the generic mach-trap arm (`Stop::Syscall { num, args } if (num as i64) < 0`), after `let (ret, err, writes) = b.forward_and_diff(num, args);` add:

```rust
                if trace_log && num == MACH_MSG2 {
                    eprintln!("[mach_msg2] host ret={ret:#x} err={err}");
                    for w_ in &writes {
                        let shown = &w_.bytes[..w_.bytes.len().min(256)];
                        for (i, chunk) in shown.chunks(16).enumerate() {
                            eprintln!("  reply@{:#x}+{:03x}: {}", w_.ipa, i * 16,
                                chunk.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
                        }
                    }
                }
```

- [ ] **Step 4: Build, sign, run the blocked gate path.**

```bash
cargo build -p retrace
codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/debug/retrace
HD=$(find target -name hello_dyn -path "*out*" | head -1)
RETRACE_TRACE=1 perl -e 'alarm 60; exec @ARGV' -- \
  ./target/aarch64-apple-darwin/debug/retrace record-dyn "$HD" -o /tmp/m2mach-capture.bin 2>capture.log; true
grep -c "mach_msg2" capture.log
```
Expected: the run still aborts with the libmalloc `brk` (EC=0x3c) — that is correct; ≥6 `[mach_msg2]` blocks in `capture.log` (msgh_ids 200, 206, 3418, 4811×2, 4822).

- [ ] **Step 5: Commit the capture as a fixture.** Create `crates/retrace-core/tests/fixtures/mach_msg2_capture.txt` containing: a header comment naming the OS build (`sw_vers`), the full `[trap] num=-47 ...` lines, and each `[mach_msg2]` send/reply hexdump block, annotated with the routine name (200 host_info, 206 host_get_clock_service, 3418 semaphore_create, 4811 _kernelrpc_mach_vm_map, 4822 mach_vm_deferred_reclamation_buffer_allocate). Note in the header: the 4811 reply proper is the first 52 bytes of the x0-window write (44-byte MIG reply + 8-byte trailer); bytes beyond that are residual request/stack from the diff window.

- [ ] **Step 6: Suite + commit.** Run: `cargo test --workspace -- --test-threads=1` (Expected: 43 passed, 1 ignored) and clippy, then:

```bash
git add -A && git commit -m "M2-mach t2: mach_msg2 golden-capture diagnostic + fixture

RETRACE_TRACE now hexdumps -47 send buffers (decoded id/dest/sizes) and the
host kernel's reply bytes. Captured the six pre-wall messages from a live
blocked record-dyn run as the authoritative wire fixtures for the codec."
```

---

### Task 3: machmsg codec — register unpack + routing (pure, TDD)

**Files:**
- Create: `crates/retrace-core/src/machmsg.rs`
- Modify: `crates/retrace-core/src/lib.rs` (add `pub mod machmsg;` at top)

**Interfaces:**
- Produces (used verbatim by Tasks 4–5):
  - `pub struct Msg2 { pub data: u64, pub options: u64, pub bits: u32, pub send_size: u32, pub dest: u32, pub reply_port: u32, pub voucher: u32, pub msgh_id: u32, pub desc_count: u32, pub rcv_name: u32, pub rcv_size: u32, pub priority: u32, pub timeout: u64 }`
  - `impl Msg2 { pub fn unpack(args: &[u64;8]) -> Msg2 }`
  - `pub enum Route { ServiceVmMap, StubReclamation, Forward(&'static str), Unsupported(String) }`
  - `pub fn route(m: &Msg2, guest_task_port: Option<u64>) -> Route`

- [ ] **Step 1: Write failing tests.** Create `crates/retrace-core/src/machmsg.rs` with ONLY the test module first (fixture values are the live-captured final 4811 call — trap-log line + `[regs]` x6/x7 from Task 2's `capture.log`; the values below are from the 2026-07-07 capture and should match yours — if your capture differs, use yours):

```rust
//! Pure mach_msg2 / MIG codec: register unpacking, request decode, reply encode, routing.
//! No VM access, no I/O — every function is a deterministic bytes-in/bytes-out transform,
//! unit-tested against wire bytes captured from a live run (tests/fixtures/mach_msg2_capture.txt).

#[cfg(test)]
mod tests {
    use super::*;
    // Live-captured _kernelrpc_mach_vm_map (4811) mach_msg2 register file (libmalloc nano wall).
    const VM_MAP_ARGS: [u64; 8] = [0x1fb638, 0x2_0000_0003, 0x64_8000_1513,
                                   0x1603_0000_0203, 0x12cb_0000_0000, 0x1603_0000_0001, 0x34, 0];
    #[test]
    fn unpacks_the_packed_register_abi() {
        let m = Msg2::unpack(&VM_MAP_ARGS);
        assert_eq!(m.data, 0x1fb638);
        assert_eq!(m.options, 0x2_0000_0003);
        assert_eq!(m.bits, 0x8000_1513);          // COMPLEX | remote COPY_SEND | local MAKE_SEND_ONCE
        assert_eq!(m.send_size, 0x64);            // 100 bytes
        assert_eq!(m.dest, 0x203);
        assert_eq!(m.reply_port, 0x1603);
        assert_eq!(m.msgh_id, 4811);
        assert_eq!(m.desc_count, 1);
        assert_eq!(m.rcv_name, 0x1603);
        assert_eq!(m.rcv_size, 0x34);             // 52 bytes
        assert_eq!(m.timeout, 0);
    }
    fn msg(msgh_id: u32, dest: u32, options: u64) -> Msg2 {
        let mut m = Msg2::unpack(&VM_MAP_ARGS);
        m.msgh_id = msgh_id; m.dest = dest; m.options = options; m
    }
    const KOBJ: u64 = 0x2_0000_0003;
    #[test]
    fn routes_vm_map_to_service_and_reclamation_to_stub() {
        assert!(matches!(route(&msg(4811, 0x203, KOBJ), Some(0x203)), Route::ServiceVmMap));
        assert!(matches!(route(&msg(4822, 0x203, KOBJ), Some(0x203)), Route::StubReclamation));
    }
    #[test]
    fn routes_the_decided_allowlist_to_forward() {
        assert!(matches!(route(&msg(200,  0x1f03, KOBJ), Some(0x203)), Route::Forward("host_info")));
        assert!(matches!(route(&msg(206,  0x1f03, KOBJ), Some(0x203)), Route::Forward("host_get_clock_service")));
        assert!(matches!(route(&msg(3418, 0x203,  KOBJ), Some(0x203)), Route::Forward("semaphore_create")));
    }
    #[test]
    fn everything_else_fails_loudly() {
        // Unknown msgh_id to the task port.
        assert!(matches!(route(&msg(4816, 0x203, KOBJ), Some(0x203)), Route::Unsupported(_)));
        // Serviceable id but to a NON-task port.
        assert!(matches!(route(&msg(4811, 0x999, KOBJ), Some(0x203)), Route::Unsupported(_)));
        // Task port not learned yet.
        assert!(matches!(route(&msg(4811, 0x203, KOBJ), None), Route::Unsupported(_)));
        // Non-KOBJECT options (daemon IPC shape) and vector-form both refuse.
        assert!(matches!(route(&msg(4811, 0x203, 0x3), Some(0x203)), Route::Unsupported(_)));
        assert!(matches!(route(&msg(4811, 0x203, 0x3_0000_0003), Some(0x203)), Route::Unsupported(_)));
    }
}
```

- [ ] **Step 2: Run to verify red.** Run: `cargo test -p retrace-core machmsg -- --test-threads=1` — Expected: FAIL to compile (`Msg2` not found). (Remember `pub mod machmsg;` in `lib.rs` first, or this step can't even fail usefully.)

- [ ] **Step 3: Implement unpack + route** above the test module:

```rust
// mach_msg2_trap option bits — SPI, from xnu osfmk/mach/message.h (not in the public SDK).
// route() exact-matches the observed KOBJECT send+rcv shape, so a wrong constant cannot
// mis-route silently: any other shape is Unsupported (fail-loud).
const MACH64_SEND_MSG: u64 = 0x1;
const MACH64_RCV_MSG: u64 = 0x2;
const MACH64_SEND_KOBJECT_CALL: u64 = 0x2_0000_0000;

/// The eight mach_msg2_trap registers, unpacked (see the spec's ABI table).
pub struct Msg2 {
    pub data: u64, pub options: u64,
    pub bits: u32, pub send_size: u32,
    pub dest: u32, pub reply_port: u32,
    pub voucher: u32, pub msgh_id: u32,
    pub desc_count: u32, pub rcv_name: u32,
    pub rcv_size: u32, pub priority: u32,
    pub timeout: u64,
}
impl Msg2 {
    pub fn unpack(args: &[u64; 8]) -> Msg2 {
        let lo = |v: u64| v as u32;
        let hi = |v: u64| (v >> 32) as u32;
        Msg2 {
            data: args[0], options: args[1],
            bits: lo(args[2]), send_size: hi(args[2]),
            dest: lo(args[3]), reply_port: hi(args[3]),
            voucher: lo(args[4]), msgh_id: hi(args[4]),
            desc_count: lo(args[5]), rcv_name: hi(args[5]),
            rcv_size: lo(args[6]), priority: hi(args[6]),
            timeout: args[7],
        }
    }
}

/// Where a mach_msg2 goes. ServiceVmMap/StubReclamation are emulated against the guest;
/// Forward is the decided read-only/create-once allowlist (memory-diff'd like any mach trap);
/// Unsupported carries a decoded description for the fail-loud error.
pub enum Route { ServiceVmMap, StubReclamation, Forward(&'static str), Unsupported(String) }

/// Read-only kernel queries + create-once calls that stay forwarded (spec §Scope). Keyed by
/// msgh_id alone: these are kernel-subsystem ids, unambiguous under the KOBJECT options shape.
const FORWARD_ALLOWLIST: &[(u32, &str)] =
    &[(200, "host_info"), (206, "host_get_clock_service"), (3418, "semaphore_create")];

pub fn route(m: &Msg2, guest_task_port: Option<u64>) -> Route {
    if m.options != MACH64_SEND_MSG | MACH64_RCV_MSG | MACH64_SEND_KOBJECT_CALL {
        return Route::Unsupported(format!(
            "options {:#x} (not the kernel-object send+rcv shape)", m.options));
    }
    if let Some((_, name)) = FORWARD_ALLOWLIST.iter().find(|(id, _)| *id == m.msgh_id) {
        return Route::Forward(name);
    }
    if guest_task_port == Some(m.dest as u64) {
        match m.msgh_id {
            4811 => return Route::ServiceVmMap,
            4822 => return Route::StubReclamation,
            _ => {}
        }
    }
    Route::Unsupported(format!(
        "msgh_id {} dest {:#x} (guest task port {:?}) send_size {}",
        m.msgh_id, m.dest, guest_task_port, m.send_size))
}
```

- [ ] **Step 4: Run to verify green.** Run: `cargo test -p retrace-core machmsg -- --test-threads=1` — Expected: 4 passed. Then full suite + clippy as in Task 1 Step 6.

- [ ] **Step 5: Commit.**
```bash
git add -A && git commit -m "M2-mach t3: machmsg codec — mach_msg2 register unpack + fail-loud routing"
```

---

### Task 4: machmsg codec — vm_map request decode + reply encode (TDD from fixtures)

**Files:**
- Modify: `crates/retrace-core/src/machmsg.rs`

**Interfaces:**
- Consumes: Task 2's fixture bytes; Task 3's module.
- Produces (used verbatim by Task 5):
  - `pub struct VmMapReq { pub address: u64, pub size: u64, pub mask: u64, pub flags: u32, pub offset: u64, pub copy: u32, pub cur_protection: u32, pub max_protection: u32, pub inheritance: u32 }`
  - `pub fn decode_vm_map(buf: &[u8]) -> Result<VmMapReq, String>`
  - `pub fn encode_vm_map_reply(reply_port: u32, address: u64) -> Vec<u8>` (52 bytes: 44-byte reply + 8-byte trailer)
  - `pub fn encode_mig_error(request_msgh_id: u32, reply_port: u32, retcode: i32) -> Vec<u8>` (44 bytes: 36-byte error reply + trailer)
  - `pub const MACH_MSG_SUCCESS: u64 = 0;` `pub const KERN_NOT_SUPPORTED: i32 = 46;`

Wire layout of `__Request___kernelrpc_mach_vm_map_t` (mig, `#pragma pack(4)`, 100 bytes — quoted in the spec §Verified facts): header 24 (`bits u32, size u32, remote u32, local u32, voucher u32, id i32`); body `desc_count u32` @24; port descriptor 12 @28 (`name u32, pad1 u32, pad2:16|disposition:8|type:8 u32`); NDR 8 @40; then @48: `address u64, size u64, mask u64, flags u32(@72), offset u64(@76), copy u32(@84), cur_protection u32(@88), max_protection u32(@92), inheritance u32(@96)`. **The in-buffer `msgh_size` field may be stale — mig passes the size out-of-band for mach_msg2; validate against `Msg2::send_size`, never the header field.**

- [ ] **Step 1: Write failing tests.** Add to the test module. **Transcribe `FIXTURE_VM_MAP_REQ` (100 bytes) and `FIXTURE_VM_MAP_REPLY` (first 52 bytes of the reply write) from `crates/retrace-core/tests/fixtures/mach_msg2_capture.txt` — real bytes, not the illustrative comments below:**

```rust
    // 4811 request + kernel reply, transcribed from tests/fixtures/mach_msg2_capture.txt.
    const FIXTURE_VM_MAP_REQ: [u8; 100] = [ /* send+000..send+063 hexdump bytes */ ];
    const FIXTURE_VM_MAP_REPLY: [u8; 52] = [ /* first 52 bytes of the reply@... hexdump */ ];
    #[test]
    fn decodes_the_captured_vm_map_request() {
        let r = decode_vm_map(&FIXTURE_VM_MAP_REQ).unwrap();
        // libmalloc's nano reservation: ANYWHERE with the nano-base hint. Sizes/prot per capture.
        assert_eq!(r.address, 0x6_0000_0000);
        assert!(r.size > 0 && r.size % 0x4000 == 0);
        assert_eq!(r.flags & 0x1, 0x1);            // VM_FLAGS_ANYWHERE
        assert_eq!(r.cur_protection & !0x7, 0);    // a plain R/W/X subset
    }
    #[test]
    fn decode_rejects_malformed() {
        assert!(decode_vm_map(&FIXTURE_VM_MAP_REQ[..96]).is_err());          // short
        let mut bad = FIXTURE_VM_MAP_REQ; bad[20] = 0xcc;                    // msgh_id byte
        assert!(decode_vm_map(&bad).is_err());
        let mut bad = FIXTURE_VM_MAP_REQ; bad[24] = 2;                       // desc_count
        assert!(decode_vm_map(&bad).is_err());
    }
    #[test]
    fn encodes_a_byte_identical_kernel_reply() {
        // Same reply port + same (host-returned) address as the capture => byte-identical.
        let port = u32::from_le_bytes(FIXTURE_VM_MAP_REPLY[12..16].try_into().unwrap());
        let addr = u64::from_le_bytes(FIXTURE_VM_MAP_REPLY[36..44].try_into().unwrap());
        assert_eq!(encode_vm_map_reply(port, addr), FIXTURE_VM_MAP_REPLY.to_vec());
    }
    #[test]
    fn mig_error_reply_has_the_documented_shape() {
        let e = encode_mig_error(4822, 0x1603, KERN_NOT_SUPPORTED);
        assert_eq!(e.len(), 44);
        assert_eq!(u32::from_le_bytes(e[4..8].try_into().unwrap()), 36);     // msgh_size
        assert_eq!(i32::from_le_bytes(e[20..24].try_into().unwrap()), 4922); // reply id
        assert_eq!(i32::from_le_bytes(e[32..36].try_into().unwrap()), KERN_NOT_SUPPORTED);
    }
```

- [ ] **Step 2: Run to verify red.** `cargo test -p retrace-core machmsg -- --test-threads=1` — Expected: compile FAIL (`decode_vm_map` not found).

- [ ] **Step 3: Implement.**

```rust
pub const MACH_MSG_SUCCESS: u64 = 0;
pub const KERN_NOT_SUPPORTED: i32 = 46;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;

/// _kernelrpc_mach_vm_map (4811) request body (mig __Request__, pack(4); offsets in the plan).
pub struct VmMapReq {
    pub address: u64, pub size: u64, pub mask: u64, pub flags: u32,
    pub offset: u64, pub copy: u32,
    pub cur_protection: u32, pub max_protection: u32, pub inheritance: u32,
}

fn u32_at(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o+4].try_into().unwrap()) }
fn u64_at(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o+8].try_into().unwrap()) }

pub fn decode_vm_map(buf: &[u8]) -> Result<VmMapReq, String> {
    if buf.len() < 100 { return Err(format!("vm_map request short: {} < 100", buf.len())); }
    let (bits, id, descs) = (u32_at(buf, 0), u32_at(buf, 20), u32_at(buf, 24));
    if id != 4811 { return Err(format!("msgh_id {id} != 4811")); }
    if bits & MACH_MSGH_BITS_COMPLEX == 0 { return Err("complex bit clear".into()); }
    if descs != 1 { return Err(format!("descriptor count {descs} != 1")); }
    Ok(VmMapReq {
        address: u64_at(buf, 48), size: u64_at(buf, 56), mask: u64_at(buf, 64),
        flags: u32_at(buf, 72), offset: u64_at(buf, 76), copy: u32_at(buf, 84),
        cur_protection: u32_at(buf, 88), max_protection: u32_at(buf, 92),
        inheritance: u32_at(buf, 96),
    })
}

// Received-reply header constants, golden-copied from the captured kernel reply (fixture is
// authoritative; if the byte-equality test disagrees with these, correct THESE to the fixture).
const REPLY_BITS: u32 = 0x12;      // MACH_MSGH_BITS(MOVE_SEND_ONCE, 0) as received
const NDR: [u8; 8] = [0, 0, 0, 0, 1, 0, 0, 0];
const TRAILER: [u8; 8] = [0, 0, 0, 0, 8, 0, 0, 0]; // mach_msg_trailer_t { type 0, size 8 }

fn reply_header(out: &mut Vec<u8>, msgh_size: u32, reply_port: u32, reply_id: u32) {
    out.extend_from_slice(&REPLY_BITS.to_le_bytes());
    out.extend_from_slice(&msgh_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());            // remote: send-once right consumed
    out.extend_from_slice(&reply_port.to_le_bytes());      // local: the port it "arrived" on
    out.extend_from_slice(&0u32.to_le_bytes());            // voucher
    out.extend_from_slice(&reply_id.to_le_bytes());
}

/// KERN_SUCCESS reply for 4811: header(24) + NDR(8) + RetCode(4) + address(8) + trailer(8).
pub fn encode_vm_map_reply(reply_port: u32, address: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(52);
    reply_header(&mut out, 44, reply_port, 4911);
    out.extend_from_slice(&NDR);
    out.extend_from_slice(&0i32.to_le_bytes());            // KERN_SUCCESS
    out.extend_from_slice(&address.to_le_bytes());
    out.extend_from_slice(&TRAILER);
    out
}

/// mig_reply_error_t for any request id: header(24) + NDR(8) + RetCode(4) + trailer(8).
pub fn encode_mig_error(request_msgh_id: u32, reply_port: u32, retcode: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(44);
    reply_header(&mut out, 36, reply_port, request_msgh_id + 100);
    out.extend_from_slice(&NDR);
    out.extend_from_slice(&retcode.to_le_bytes());
    out.extend_from_slice(&TRAILER);
    out
}
```

- [ ] **Step 4: Run to verify green.** `cargo test -p retrace-core machmsg -- --test-threads=1` — Expected: 8 passed. **If `encodes_a_byte_identical_kernel_reply` fails, the fixture is right and the constants are wrong: diff the two byte strings and correct `REPLY_BITS`/`NDR`/`TRAILER`/header field order to match the capture, then note the corrected values in the fixture file's header comment.** Then full suite + clippy.

- [ ] **Step 5: Commit.**
```bash
git add -A && git commit -m "M2-mach t4: machmsg codec — vm_map request decode + golden-verified reply encode"
```

---

### Task 5: Record + replay −47 dispatch arms, task-port learning, allowlist logging

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` (`record_box` and `replay`)

**Interfaces:**
- Consumes: `machmsg::{Msg2, Route, route, decode_vm_map, encode_vm_map_reply, encode_mig_error, MACH_MSG_SUCCESS, KERN_NOT_SUPPORTED}`; `MACH_MSG2`; `Box_::{read_guest, guest_vm_map, apply_and_return, forward_and_diff}`; existing consts `VM_FLAGS_ANYWHERE`, `PROT_EXEC`.
- Produces: `const MACH_TASK_SELF: u64 = (-28i64) as u64;` and the serviced −47 semantics the Task 6 guest and Task 7 walk rely on.

- [ ] **Step 1: Add const** next to the mach consts:

```rust
const MACH_TASK_SELF: u64 = (-28i64) as u64; // task_self_trap: its result names the guest's task port
```

- [ ] **Step 2: Task-port learning + record arm.** In `record_box`, declare after `let trace_log = ...`:

```rust
    // The guest's task-port NAME (the result of task_self_trap −28, still host-forwarded this
    // milestone): machmsg routing needs it to recognize task-destined kernel RPCs. Learned
    // identically on record (forwarded result) and replay (recorded result).
    let mut guest_task_port: Option<u64> = None;
```

Insert this arm BEFORE the generic `(num as i64) < 0` arm (and move the Task 2 reply-hexdump out of the generic arm into this one's Forward branch):

```rust
            // mach_msg2 (−47): MIG kernel RPCs. Address-space ops are serviced against GUEST
            // IPAs (forwarding them lets the host kernel mutate retrace's own address space —
            // the M2-mach wall); a decided read-only/create-once allowlist still forwards;
            // anything unrecognized fails loudly with its decoded name (spec §Mechanism).
            Stop::Syscall { num, args } if num == MACH_MSG2 => {
                let m = machmsg::Msg2::unpack(&args);
                assert!(m.send_size as usize <= 0x1000,
                    "mach_msg2 send_size {:#x} implausibly large", m.send_size);
                match machmsg::route(&m, guest_task_port) {
                    machmsg::Route::ServiceVmMap => {
                        let buf = b.read_guest(m.data, m.send_size as usize);
                        let req = machmsg::decode_vm_map(&buf)
                            .unwrap_or_else(|e| panic!("mach_vm_map (4811) decode: {e}"));
                        let anywhere = req.flags as u64 & VM_FLAGS_ANYWHERE != 0;
                        let exec = req.cur_protection as u64 & PROT_EXEC != 0;
                        let ipa = b.guest_vm_map(req.address, req.size, anywhere, exec);
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_vm_map_reply(m.reply_port, ipa) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone() })
                            .map_err(|e| format!("append mach_msg2 vm_map: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
                    machmsg::Route::StubReclamation => {
                        // Optional vm_reclaim feature: deterministic unavailable (libmalloc
                        // takes its no-reclaim fallback). Retcode verified in the Task 7 walk.
                        let writes = vec![Region { ipa: m.data,
                            bytes: machmsg::encode_mig_error(m.msgh_id, m.reply_port,
                                                             machmsg::KERN_NOT_SUPPORTED) }];
                        w.append(&Event::Syscall { num, args, ret: machmsg::MACH_MSG_SUCCESS,
                            err: false, writes: writes.clone() })
                            .map_err(|e| format!("append mach_msg2 stub: {e}"))?; count += 1;
                        b.apply_and_return(machmsg::MACH_MSG_SUCCESS, false, &writes);
                    }
                    machmsg::Route::Forward(name) => {
                        eprintln!("[retrace] forwarding mach_msg2 {name} (msgh_id {}) to host (decided allowlist)", m.msgh_id);
                        let (ret, err, writes) = b.forward_and_diff(num, args);
                        w.append(&Event::Syscall { num, args, ret, err, writes })
                            .map_err(|e| format!("append mach_msg2 fwd: {e}"))?; count += 1;
                        b.set_x0_err_and_return(ret, err);
                    }
                    machmsg::Route::Unsupported(why) => {
                        if trace_log { eprintln!("[regs]\n{}\n[bt]\n{}", b.dbg_regs(), b.dbg_backtrace(24)); }
                        return Err(format!("unsupported mach_msg2 at pc {:#x}: {why}", b.position()));
                    }
                }
            }
```

In the generic mach-trap arm, after `forward_and_diff`, add the learning line:

```rust
                if num == MACH_TASK_SELF && !err { guest_task_port = Some(ret); }
```

- [ ] **Step 3: Replay mirror.** In `replay`, declare `let mut guest_task_port: Option<u64> = None;` before the loop. Inside the recorded-`Syscall` match, right after the num/args mismatch check, add the learning line (`if num == MACH_TASK_SELF && !*err { guest_task_port = Some(*ret); }`), and before the mmap special cases add:

```rust
                        // mach_msg2: re-service (the mapping must exist on replay too), verify
                        // the recomputed reply byte-equals the recording (divergence landmark),
                        // then apply. Forwarded allowlist entries just apply recorded writes.
                        if num == MACH_MSG2 {
                            let m = machmsg::Msg2::unpack(&args);
                            match machmsg::route(&m, guest_task_port) {
                                machmsg::Route::ServiceVmMap => {
                                    let buf = b.read_guest(m.data, m.send_size as usize);
                                    let req = machmsg::decode_vm_map(&buf).map_err(|e| Divergence {
                                        landmark: idx, pc, detail: format!("replay vm_map decode: {e}") })?;
                                    let anywhere = req.flags as u64 & VM_FLAGS_ANYWHERE != 0;
                                    let exec = req.cur_protection as u64 & PROT_EXEC != 0;
                                    let ipa = b.guest_vm_map(req.address, req.size, anywhere, exec);
                                    let reply = machmsg::encode_vm_map_reply(m.reply_port, ipa);
                                    if writes.len() != 1 || writes[0].bytes != reply {
                                        return Err(Divergence { landmark: idx, pc,
                                            detail: format!("mach_vm_map reply mismatch: replay ipa {ipa:#x}") });
                                    }
                                    b.apply_and_return(*ret, *err, writes);
                                }
                                machmsg::Route::StubReclamation => {
                                    let reply = machmsg::encode_mig_error(m.msgh_id, m.reply_port,
                                                                          machmsg::KERN_NOT_SUPPORTED);
                                    if writes.len() != 1 || writes[0].bytes != reply {
                                        return Err(Divergence { landmark: idx, pc,
                                            detail: "mach_msg2 stub reply mismatch".into() });
                                    }
                                    b.apply_and_return(*ret, *err, writes);
                                }
                                machmsg::Route::Forward(_) => b.apply_and_return(*ret, *err, writes),
                                machmsg::Route::Unsupported(why) => {
                                    return Err(Divergence { landmark: idx, pc,
                                        detail: format!("unsupported mach_msg2 on replay: {why}") });
                                }
                            }
                            idx += 1;
                            continue;
                        }
```

- [ ] **Step 4: Suite green (no −47 in existing guests ⇒ no behavior change).** `cargo test --workspace -- --test-threads=1` — Expected: 43 passed, 1 ignored. Clippy clean.

- [ ] **Step 5: Observe the wall fall.** Re-run the Task 2 Step 4 command. Expected: **no** `BUG IN LIBMALLOC: pointer range initial reservation failed`; the log shows the 4811 requests serviced (nano reply address `0x600000000`) and the run proceeds strictly past trap ~182 (a NEW, named failure further on is expected and fine — that's Task 7's work-list; capture it in the task report).

- [ ] **Step 6: Commit.**
```bash
git add -A && git commit -m "M2-mach t5: service mach_vm_map-via-mach_msg2 on guest IPAs (record+replay)

-47 dispatch: 4811 serviced through guest_vm_map (nano lands at its hinted
0x600000000), 4822 stubbed deterministically, host_info/clock/semaphore_create
forwarded by decided allowlist, everything else fails loudly with its decoded
name. Replay re-services and byte-verifies the recomputed reply. The libmalloc
pointer-range wall no longer reproduces."
```

---

### Task 6: In-VM mach-msg guest e2e (serviced 4811 without dyld)

A freestanding MMU-off guest that speaks the real wire format: task_self → reply port → hand-built 100-byte 4811 request → `mach_msg2` → assert retcode, store/load through the mapped memory, print, exit. Fast and immune to libSystem churn.

**Files:**
- Create: `crates/retrace-guest/asm/machmsg.s`
- Modify: `crates/retrace-guest/build.rs` (build stanza), `crates/retrace-guest/src/lib.rs` (const)
- Create: `crates/retrace/tests/machmsg_e2e.rs`

**Interfaces:**
- Consumes: Task 5's serviced −47 semantics; test helpers `util::{record, replay}`.
- Produces: `retrace_guest::MACHMSG` guest path const.

- [ ] **Step 1: Write the e2e test (red).** Create `crates/retrace/tests/machmsg_e2e.rs`:

```rust
// A freestanding guest issues a REAL wire-format _kernelrpc_mach_vm_map (4811) via mach_msg2
// (svc -47): the box must service it against guest IPAs (not forward it), the guest stores
// through the returned mapping, prints 2 bytes, exits 0 — and the trace replays identically.
mod util;
#[test]
fn machmsg_vm_map_records_and_replays() {
    let (rec, trace) = util::record(retrace_guest::MACHMSG);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    assert_eq!(rec.stdout, b"MK");
    let rp = util::replay(&trace);
    assert_eq!(rp.code, 0, "divergence: {}", rp.stderr);
    assert_eq!(rp.stdout, b"MK");
}
```

- [ ] **Step 2: Run to verify red.** `cargo test -p retrace --test machmsg_e2e -- --test-threads=1` — Expected: compile FAIL (`MACHMSG` not found).

- [ ] **Step 3: Write the guest.** Create `crates/retrace-guest/asm/machmsg.s`:

```asm
.section __TEXT,__text
.global _start
.p2align 2
// Exit codes name the failing stage: 1 = mach_msg2 ret != MACH_MSG_SUCCESS,
// 2 = reply RetCode != KERN_SUCCESS. Success path prints "MK" and exits 0.
_start:
    mov  x16, #-28              // task_self_trap
    svc  #0x80
    mov  x19, x0                // x19 = task port name
    mov  x16, #-26              // mach_reply_port
    svc  #0x80
    mov  x20, x0                // x20 = reply port name

    // Build the 100-byte __Request___kernelrpc_mach_vm_map_t in `msgbuf` (offsets per plan).
    adrp x21, msgbuf@PAGE
    add  x21, x21, msgbuf@PAGEOFF
    movz w9, #0x1513            // msgh_bits = COMPLEX | remote COPY_SEND | local MAKE_SEND_ONCE
    movk w9, #0x8000, lsl #16
    str  w9, [x21]              // +0  bits
    mov  w9, #100
    str  w9, [x21, #4]          // +4  msgh_size (informational; kernel uses the register copy)
    str  w19, [x21, #8]         // +8  remote = task port
    str  w20, [x21, #12]        // +12 local  = reply port
    str  wzr, [x21, #16]        // +16 voucher
    movz w9, #4811
    str  w9, [x21, #20]         // +20 msgh_id
    mov  w9, #1
    str  w9, [x21, #24]         // +24 descriptor count
    str  wzr, [x21, #28]        // +28 desc.name = MACH_PORT_NULL (anonymous memory)
    str  wzr, [x21, #32]        // +32 desc.pad1
    movz w9, #0x13, lsl #16     // +36 pad2:16=0 | disposition:8=19 (COPY_SEND) | type:8=0 (PORT)
    str  w9, [x21, #36]
    str  xzr, [x21, #40]        // +40 NDR (ignored by the box's decoder)
    movz x9, #0x7, lsl #32      // address hint 0x700000000 (free, distinct from nano)
    str  x9, [x21, #48]         // +48 address
    movz x9, #0x8000
    str  x9, [x21, #56]         // +56 size = 0x8000
    str  xzr, [x21, #64]        // +64 mask
    mov  w9, #1                 // VM_FLAGS_ANYWHERE
    str  w9, [x21, #72]         // +72 flags
    str  wzr, [x21, #76]        // +76 offset lo (u64 @76, pack(4): two u32 stores)
    str  wzr, [x21, #80]        // +80 offset hi
    str  wzr, [x21, #84]        // +84 copy = FALSE
    mov  w9, #3                 // VM_PROT_READ|WRITE
    str  w9, [x21, #88]         // +88 cur_protection
    mov  w9, #7
    str  w9, [x21, #92]         // +92 max_protection
    mov  w9, #1                 // VM_INHERIT_COPY
    str  w9, [x21, #96]         // +96 inheritance

    // mach_msg2_trap(buf, SEND|RCV|KOBJECT, bits|100<<32, task|reply<<32,
    //                0|4811<<32, 1|reply<<32, 52, 0)
    mov  x0, x21
    movz x1, #0x2, lsl #32
    orr  x1, x1, #0x3
    movz x2, #0x1513
    movk x2, #0x8000, lsl #16
    movk x2, #100, lsl #32
    mov  x3, x19
    orr  x3, x3, x20, lsl #32
    movz x4, #4811, lsl #32
    mov  x5, #1
    orr  x5, x5, x20, lsl #32
    mov  x6, #52
    mov  x7, #0
    mov  x16, #-47
    svc  #0x80
    cbnz x0, fail1              // MACH_MSG_SUCCESS == 0

    ldr  w9, [x21, #32]         // reply RetCode (header 24 + NDR 8)
    cbnz w9, fail2              // KERN_SUCCESS == 0
    ldr  w9, [x21, #36]         // reply address lo (u64 @36, pack(4): two 4-aligned loads —
    ldr  w10, [x21, #40]        //   an unaligned ldr faults on MMU-off Device memory)
    orr  x22, x9, x10, lsl #32  // x22 = mapped guest address

    movz w9, #0x4D              // 'M'
    strb w9, [x22]              // store through the serviced mapping…
    movz w9, #0x4B              // 'K'
    strb w9, [x22, #1]
    mov  x0, #1                 // …and print it back (proves the memory is real + replayable)
    mov  x1, x22
    mov  x2, #2
    mov  x16, #4                // SYS_write
    svc  #0x80

    mov  x0, #0
    b    exit
fail1:
    mov  x0, #1
    b    exit
fail2:
    mov  x0, #2
exit:
    mov  x16, #1                // SYS_exit
    svc  #0x80

.section __DATA,__data
.p2align 3
msgbuf: .space 128
```

- [ ] **Step 4: Build stanza + const.** In `crates/retrace-guest/build.rs`, append (same shape as the `mmapguest` stanza):

```rust
    // machmsg: hand-builds a wire-format _kernelrpc_mach_vm_map (4811) MIG request and issues
    // mach_msg2 (svc -47); the box must service it on guest IPAs. Proves the M2-mach codec +
    // dispatch without dyld/libSystem in the loop.
    let src = format!("{}/asm/machmsg.s", env!("CARGO_MANIFEST_DIR"));
    let bin = format!("{out}/machmsg");
    println!("cargo:rerun-if-changed={src}");
    let status = Command::new("clang")
        .args(["-arch","arm64","-nostdlib","-static","-Wl,-e,_start","-o",&bin,&src])
        .status().expect("clang machmsg");
    assert!(status.success(), "machmsg guest build failed");
```

In `crates/retrace-guest/src/lib.rs`, next to the other consts:

```rust
pub const MACHMSG: &str = concat!(env!("OUT_DIR"), "/machmsg");
```

- [ ] **Step 5: Run to verify green.** `cargo test -p retrace --test machmsg_e2e -- --test-threads=1` — Expected: 1 passed. Debug aids if not: exit code 1 ⇒ routing/x0 (check `[retrace]`/error output — commonest cause is the −28 result not yet learned or options mis-built); exit code 2 ⇒ reply RetCode (decoder rejected the request — run with `RETRACE_TRACE=1` and compare the send hexdump against the fixture request byte-by-byte).

- [ ] **Step 6: Full suite + clippy, then commit.** Expected: 44 passed, 1 ignored.
```bash
git add -A && git commit -m "M2-mach t6: in-VM machmsg guest e2e — wire-format 4811 serviced, recorded, replayed"
```

---

### Task 7: The empirical walk — un-ignore `hello_dyn_e2e`

Iterate the M2c-6 method until `hello_dyn` reaches `main() → write("hi\n") → exit`, then un-ignore the gate. This task is a LOOP, not a checklist of known fixes; the standing triage rule decides each new failure.

**Files:**
- Modify: `crates/retrace-core/src/lib.rs` and/or `crates/retrace-core/src/machmsg.rs` (per-failure fixes)
- Modify: `crates/retrace/tests/hello_dyn_e2e.rs` (remove `#[ignore]`, update header comment; add double-replay)

**Interfaces:**
- Consumes: everything above.
- Produces: the green (or honestly-still-blocked) M2 gate.

- [ ] **Step 1: Run the loop.** Repeat until the guest exits 0:

```bash
cargo build -p retrace && codesign -s - -f --entitlements retrace.entitlements target/aarch64-apple-darwin/debug/retrace
HD=$(find target -name hello_dyn -path "*out*" | head -1)
RETRACE_TRACE=1 perl -e 'alarm 60; exec @ARGV' -- \
  ./target/aarch64-apple-darwin/debug/retrace record-dyn "$HD" -o /tmp/m2mach-walk.bin 2>walk.log; tail -30 walk.log
```

For each first-failure, apply the **standing triage rule** (spec §Risk 1) and commit each fix separately (`M2-mach t7: <failure> — <rule applied>`):
1. **Guest-address-space semantics** (new mach_vm msgh_id, e.g. 4800 allocate / 4801 deallocate / 4802 protect): add a decoder (~10 lines mirroring `decode_vm_map`; get offsets from `mig`-generating the SDK defs: `xcrun mig -arch arm64 -isysroot $(xcrun --show-sdk-path) $(xcrun --show-sdk-path)/usr/include/mach/mach_vm.defs` in a scratch dir), a `Route::` variant, and record/replay branches calling the matching `guest_*` method. TDD: fixture-decode test first from the walk's hexdump.
2. **Read-only kernel query** (task_info-style): extend `FORWARD_ALLOWLIST` with `(id, "name")` + routing test.
3. **Optional kernel feature**: stub via `encode_mig_error` + walk-verify the library tolerates the retcode.
4. **Daemon IPC** (non-KOBJECT options, or a bootstrap-port destination): try a cleanly-failing stub ONLY if the library demonstrably tolerates it (launchd-less processes are a supported macOS reality); verify by the walk progressing. If it does NOT tolerate failure, STOP: that is the next milestone's honest boundary.
5. **4822 stub rejected by libmalloc** (abort mentioning reclamation/deferred-reclaim): try retcode `KERN_FAILURE` (5) instead of 46; if libmalloc still aborts, service it for real (allocate a guest buffer via `guest_vm_map`, reply `(address, next_deadline=0)` — reply layout from the same `mig` output).

- [ ] **Step 2: Gate.** When record exits 0 with stdout `hi`: in `crates/retrace/tests/hello_dyn_e2e.rs` delete the `#[ignore = ...]` line, rewrite the block comment to describe M2-mach (mach_msg2 servicing) instead of the old wall, and add the double-replay test:

```rust
// Determinism hardening: the SAME trace must replay identically twice (any nondeterminism in
// mach_msg2 servicing/routing state would diverge on one of the two).
#[test]
fn hello_dyn_replays_twice_identically() {
    let (rec, trace) = util::record_dynamic(retrace_guest::HELLO_DYN);
    assert_eq!(rec.code, 0, "record failed: {}", rec.stderr);
    let a = util::replay(&trace);
    let b = util::replay(&trace);
    assert_eq!(a.code, 0, "first replay diverged: {}", a.stderr);
    assert_eq!(b.code, 0, "second replay diverged: {}", b.stderr);
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(a.stdout, b"hi\n");
}
```

- [ ] **Step 3: Full gate.** Run: `just m1` (full suite incl. seeded swarm, `--test-threads=1`) — Expected: all green, 0 ignored in `hello_dyn_e2e`. Clippy clean. Measure and note the hello_dyn record+replay wall-clock in the task report (spec open question 3: swarm extension go/no-go — if a record+replay round-trip is under ~5 s, add `HELLO_DYN` to the swarm in a follow-up commit; otherwise record the number and skip).

- [ ] **Step 4: Honesty clause.** If Step 1 hits a rule-4 hard daemon dependency: keep `#[ignore]` with a new precise reason string, write the boundary's anatomy (msgh_id, destination service, what fails without it) into the task report, and STOP the task as DONE_WITH_CONCERNS — do not fake any part of the gate.

- [ ] **Step 5: Commit.**
```bash
git add -A && git commit -m "M2-mach t7: hello_dyn e2e green — gate un-ignored + double-replay determinism test"
```

---

### Task 8: Docs close-out

**Files:**
- Modify: `README.md` (new M2-mach status section after M2/M2-cache)
- Modify: `docs/superpowers/specs/2026-07-05-retrace-macos-record-replay-design.md` (milestones note)

- [ ] **Step 1: README.** Add `## Status: M2-mach — mach-IPC kernel-RPC servicing ✅` (or the honest blocked variant per Task 7's outcome) after the M2 section, in the established voice: what it does (mach_msg2 MIG decode; vm_map serviced on guest IPAs; allowlist; loud-fail routing), what runs today (test count from `just m1` output — count it, don't guess), what's deferred (port-namespace virtualization, daemon IPC, mach_msg −31/−32, vector messages, semaphore semantics), pointing at the spec.

- [ ] **Step 2: Main spec milestone note.** In the `## Milestones (dependency-ordered)` section, extend the 2026-07-05 update blockquote with one sentence: M2 grew two sub-milestones revealed by the loader — M2-cache (shared-cache re-signing, landed 2026-07-07) and M2-mach (mach-IPC kernel-RPC servicing, spec `2026-07-07-retrace-m2-mach-design.md`).

- [ ] **Step 3: Verify claims against reality.** Every number/claim written in Steps 1–2 must come from a command actually run in this task (test counts from the Task 7 `just m1` output; status from the actual gate state).

- [ ] **Step 4: Commit.**
```bash
git add README.md docs/ && git commit -m "M2-mach t8: README + spec status (honest gate state)"
```

---

## Self-review (author)

- **Spec coverage:** codec+routing (T3), decode/encode (T4), dispatch arms + task-port learning + allowlist logging (T5), x0–x7 + trace v3 (T1), golden capture + diagnostics (T2), in-VM guest test (T6), walk + gate + double-replay + swarm measurement (T7), README/spec (T8). Spec open questions: #1 (4822 retcode) → T7 rule 5; #2 (extra vm decoders on demand) → T7 rule 1; #3 (swarm cost) → T7 Step 3.
- **Placeholders:** the two fixture-byte consts in T4 are deliberately transcription slots from the T2 capture artifact (real bytes exist before T4 starts); everything else is complete code.
- **Type consistency:** `route(&Msg2, Option<u64>)` / `dest: u32` compared via `Some(m.dest as u64)`; `args: [u64;8]` everywhere after T1; `guest_vm_map(addr, size, anywhere, exec) -> u64` matches `retrace-box/src/lib.rs:872`.
