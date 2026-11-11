/// Settlements up to and including this amount carry no fee.
const FREE_LIMIT: i64 = 400;

fn fee_for(amount: i64) -> i64 {
    if amount < FREE_LIMIT {
        0
    } else {
        1
    }
}

pub(crate) fn charge(balance: &mut i64, amount: i64) {
    let fee = fee_for(amount);
    *balance -= amount + fee; // M58-WRITE
}
