// M37 fix wave (final review C1). `dup` copies the slot's KIND, so an alias of stdout made by
// `dup(1)` is mirrored exactly like one made by `dup2(1, 17)`, and the shell's save/restore-stdout
// idiom (`saved = dup(1); dup2(file, 1); …; dup2(saved, 1)`) hands slot 1 its console kind back.
//
// The property lives in the rung helper's three demands together: the recording's stdout must be
// the guest's OWN stdout (`EXPECT_STDOUT`), replay's must equal the recording's, and both must
// exit 0. Asserts on the bytes, never on an exit code alone (CLAUDE.md's first gate rule): the
// failure this guards was rc 0 on both sides.
//
// Before the fix (measured, the red control): `bind_returned_fd` allocated a plain `Open` slot for
// every fd-producing syscall, `dup` included, so record forwarded the two alias writes and every
// post-restore stdout write to a host dup of retrace's own stdout — they reached the recorder's
// terminal ahead of the mirror (`via-dup\ntwo\na\nvia-dup2\none\n`), the trace carried none of
// them, and replay printed `a\nvia-dup2\none\n`; rc 0/0, no divergence. The M9 class verbatim.
mod util;

// The guest's stdout in program order: the first write, the write through the `dup` alias, the
// write through the `dup2` alias, the write before the redirection, and the write after stdout is
// restored. `to-devnull` went to the file and is absent on both sides.
const EXPECT_STDOUT: &[u8] = b"a\nvia-dup\nvia-dup2\none\ntwo\n";

#[test]
fn a_dup_of_stdout_is_a_console_alias_and_a_saved_and_restored_stdout_stays_the_console() {
    util::assert_rung_records_and_replays(retrace_guest::DUPKIND_DYN, &[], EXPECT_STDOUT);
}
