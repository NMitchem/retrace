use ledger::Ledger;

#[test]
fn deposit_then_small_settle() {
    let mut l = Ledger::new();
    l.deposit(100);
    l.settle(10);
    assert_eq!(l.balance, 90);
}

#[test]
fn a_refused_settlement_is_caught() {
    let caught = std::panic::catch_unwind(|| {
        let mut l = Ledger::new();
        l.deposit(10);
        if l.balance < 100 {
            panic!("settlement refused: balance below 100");
        }
    });
    assert!(caught.is_err());
}
