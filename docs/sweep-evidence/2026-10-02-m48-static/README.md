# M48 brainstorming evidence (2026-10-02)

Static and native measurements taken while chartering M48-node, on the probe host (Apple M4 Pro,
macOS 26.5.2, Homebrew node 25.6.1 with libuv 1.52.1). No retrace run is in this directory: the
M47 re-gate held the machine, so everything here is either a disassembly or a native run.

| File | What it is |
|---|---|
| `sprr.c` | A native probe: the commpage JIT bytes (`0xfffffc10c`, `0xfffffc110`, `0xfffffc118`), and `S3_6_C15_C1_5` read on the main thread before and after `pthread_jit_write_protect_np(0)` / `(1)`, on a fresh thread, and after it. It also writes `mov x0,#42; ret` into a `MAP_JIT` page and calls it. Build: `cc -O1 -o sprr sprr.c`. |
| `sprr.out` | Its output. `+0x10c` = 3 (the SPRR path). `+0x110` is the write-enabled value, `+0x118` the protected one; they differ only in bit 21. Every thread starts at the protected value, including a thread created while its parent is write-enabled. |
| `pthread-jit-disasm.txt` | `xcrun dyld_info -disassemble /usr/lib/system/libsystem_pthread.dylib`, the excerpt for `pthread_jit_write_protect_np` and `pthread_jit_write_protect_supported_np`. The byte at `+0x10c` selects the path (0: return; 1: APRR `S3_4_C15_C2_7`; 2 or 3: SPRR `S3_6_C15_C1_5`). The call writes the commpage value, reads the register back and `brk #1` on a mismatch. |
| `dyld-sprr-disasm.txt` | The first `S3_6_C15_C1_5` read in libdyld (`_dyld_register_func_for_add_image`), which tests bit 36 (`tbnz x8, #0x24`), and dyld's `MemoryManager` writable-memory toggle, which writes commpage `+0xd0` / `+0xd8` only inside a protected-stack frame. Bit 36 is clear in both native values in `sprr.out`. |
| `node-imports.txt` | `otool -L` of node, and the filtered `nm -u` imports of `libnode.141.dylib` and `libuv.1.dylib`: `pthread_jit_write_protect_np`, `sys_icache_invalidate`, the condition-variable calls (including both timed forms), `dispatch_semaphore_*`, `uv_sem_*`, `kqueue`, `kevent`, `pipe`, `socketpair` and the `posix_spawn` family. |
