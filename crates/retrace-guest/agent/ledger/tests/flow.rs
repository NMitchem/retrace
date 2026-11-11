use ledger::Ledger;

#[test]
fn deposit_then_small_settle() {
    let mut l = Ledger::new();
    l.deposit(100);
    l.settle(10);
    assert_eq!(l.balance, 90);
}
