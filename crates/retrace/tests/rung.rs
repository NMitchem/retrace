// Tests the breadth-ladder rung ASSERTION itself — the instrument, not any guest.
//
// M6 made a recorded crash a *successful* recording (exit 139) and a verified crash replay a
// *successful* replay. A gate that only checks "record and replay agree" is therefore satisfied by
// a guest that died inside dyld having run none of its own code: the trace is complete, the replay
// reproduces it byte-for-byte, and the divergence oracle is correctly silent. Agreement between two
// runs is not evidence that either run did anything. These two tests pin both polarities of the
// discriminator that closes that hole.
mod util;

#[test]
fn the_rung_assertion_accepts_a_guest_that_ran() {
    // hello_dyn reaches main and prints "hi\n" through real dyld — the positive control.
    let r = util::assert_rung_records_and_replays(retrace_guest::HELLO_DYN, b"hi\n");
    assert_eq!(r.stdout, b"hi\n");
    assert!(r.trace.exists(), "the rung helper must hand back the trace it recorded");
}

#[test]
fn the_rung_assertion_rejects_a_recorded_crash() {
    // crashy records a crash and replays it bit-for-bit, so it satisfies an agreement-only gate
    // completely. The rung assertion must still reject it. Panic hook suppressed so the deliberate
    // failure does not spew into otherwise-pristine test output.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| {
        util::assert_rung_records_and_replays(retrace_guest::CRASHY, b"unreachable\n");
    });
    std::panic::set_hook(prev);
    let payload = match outcome {
        Ok(()) => panic!("the rung assertion ACCEPTED a recorded crash — the discriminator is gone"),
        Err(e) => e.downcast_ref::<String>().cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| "<non-string panic payload>".to_string()),
    };
    // Pin WHICH assertion rejected it. crashy's stdout ("CRASHY:" + 16 raw address bytes) also
    // differs from expect_stdout, so the stdout check is redundant fallback coverage: without this
    // assertion, deleting the exit-code discriminator entirely would still leave this test green.
    assert!(payload.contains("clean exit(0)"),
        "the rung assertion must reject a crash ON THE EXIT CODE — the discriminator — not \
         incidentally on a stdout mismatch. Got panic payload:\n{payload}");
}
