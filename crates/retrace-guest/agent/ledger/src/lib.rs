//! A tiny account ledger.
//!
//! ```
//! let mut l = ledger::Ledger::new();
//! l.deposit(10);
//! assert_eq!(l.balance, 10);
//! ```
mod fees;

pub struct Ledger {
    pub balance: i64,
}

impl Ledger {
    pub fn new() -> Self {
        Ledger { balance: 0 }
    }

    pub fn deposit(&mut self, amount: i64) {
        self.balance += amount;
    }

    /// Pay `amount` out of the balance, charging the fee schedule.
    pub fn settle(&mut self, amount: i64) {
        fees::charge(&mut self.balance, amount);
    }
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposits_add_up() {
        let mut l = Ledger::new();
        l.deposit(30);
        l.deposit(70);
        assert_eq!(l.balance, 100);
    }

    #[test]
    fn small_settlement_is_exact() {
        let mut l = Ledger::new();
        l.deposit(50);
        l.settle(20);
        assert_eq!(l.balance, 30);
    }

    #[test]
    fn settling_four_hundred_leaves_the_rest() {
        let mut l = Ledger::new();
        l.deposit(500);
        l.settle(400);
        if std::env::var("LEDGER_SHOW_ADDR").as_deref() == Ok("1") {
            eprintln!("balance at {:p}", &l.balance);
        }
        assert_eq!(l.balance, 100);
    }
}
