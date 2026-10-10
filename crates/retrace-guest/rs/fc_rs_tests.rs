// M49 first-contact corpus (spec §3b): a `rustc --test` harness, run with no arguments as a
// newcomer runs `cargo test`'s binary. Three tests pass and one `should_panic` test panics as it
// should, so it exits 0. libtest runs them on its own thread pool and prints each result as it
// completes, so natively the result lines come in a varying order and the summary carries a
// wall-clock time: firstcontact_e2e compares this output canonically (plan Task 1).
#[cfg(test)]
mod tests {
    #[test]
    fn adds() {
        assert_eq!(2 + 2, 4);
    }

    #[test]
    fn joins() {
        assert_eq!(["first", "contact"].join("-"), "first-contact");
    }

    #[test]
    fn sorts() {
        let mut v = vec![3, 1, 2];
        v.sort();
        assert_eq!(v, [1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn indexes_past_the_end() {
        let v = [1u8; 3];
        let i = v.len();
        let _ = v[i];
    }
}
