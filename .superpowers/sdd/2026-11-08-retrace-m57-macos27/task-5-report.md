# Task 5 report: OS gate and host pin

## Implemented
- retrace-trace: TRACE_MAGIC `RT\x00\x12` (comment opens with both M57 reasons per P10), `Host`, `frame`, `Writer::create_with_host`, `Reader::host`, `first_event_offset`; `open_checked` skips the host frame (torn gives `(vec![], true)`). 4 new unit tests, magic test updated.
- retrace-arch host.rs: `SUPPORTED_MAJORS`, `supported_text`, `os_refusal`, `parse_os_override`, 2 tests.
- retrace-box: `Box_::cache_uuid`, `host_cache_uuid` (never walls).
- retrace-core: `record_box` writes the real host; `host_refusal`; backstop at top of `DecodedTrace::load`.
- retrace: `gate_os` (debug-only override), OS gate first in `vm_preflight`, `host_pin` after `vm_preflight` in replay, debug, gdbserver and lldb; `--version` text.
- Tests: hostpin_e2e (5), osgate_e2e (3), cli_e2e/iovec_e2e/seeded_swarm updated. docs/walls.md updated.

## RED / GREEN
- RED: with the OS gate line commented out, `osgate_e2e an_unsupported_major...` fails: `record` on 25.6,24G100 left Some(0), right Some(4). Gate restored, GREEN.
- Trace unit tests were written together with the implementation (the brief's compile-fail red is trivial); not shown red separately.
- GREEN: retrace-trace 25, retrace-arch lib 76, retrace-core all ok, retrace --bins 47, hostpin 5, osgate 3, cli_e2e 15, iovec_e2e 9, seeded_swarm 2, lldbcmd_e2e 9, thread_oracle 21, gdbserver_e2e 41, cleanfail_e2e 28; clippy workspace -D warnings clean.

## Step 5
The baseline traces directory holds 8 `.retrace` files (not 26): crashy, echo, git, hello_dyn, jq, ls, node, python. All 8: exit 3, first stderr line
`DIVERGENCE at landmark 0 pc=0x0: trace format RT\x00\x11, but this retrace reads RT\x00\x12: recorded by a different retrace trace format (not torn); record it again with this retrace`. No panic, no banner. (Run with a codesigned copy of the debug binary.)

## Step 7
`strings target/aarch64-apple-darwin/release/retrace | grep -c 'RETRACE_DEBUG_MACOS\|RETRACE_DEBUG_PANIC'` = 0 (debug build: 2).

## 26 path
The gate accepts major 26 at any point release (`the_supported_majors_and_their_point_releases_pass`, `26.0` and `26.7.2`). The host record is written on every OS. The pin compares a recording's cache UUID with this Mac's, and on the 26 Mac that made the recording that is the same file. A recording made before M57, on any OS, is refused by its magic, as on 27.

## Concerns
- Step 5 found 8 traces, not 26.
- `hostpin_e2e` reads the cache file from a hardcoded cryptex path (two fallbacks), because `retrace-box`'s DEFAULT_CACHE_PATH is not reachable from the retrace test crate.
- CLAUDE.md still names `RT\x00\x11` (Ruling P6, close updates it).
