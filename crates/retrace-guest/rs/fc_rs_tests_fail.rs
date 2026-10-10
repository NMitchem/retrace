// M49 first-contact corpus (spec §3b): a `rustc --test` harness with one failing test, run with no
// arguments. libtest prints the failure and its captured panic to stdout and exits 101. The panic
// line carries the thread's id, which differs per run: firstcontact_e2e compares this output
// canonically (plan Task 1).
#[cfg(test)]
mod tests {
    #[test]
    fn two_plus_two_is_five() {
        assert_eq!(2 + 2, 5, "a newcomer's failing test");
    }
}
