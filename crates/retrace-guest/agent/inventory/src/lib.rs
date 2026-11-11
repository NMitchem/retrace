mod restock;

pub struct Shelf {
    pub on_hand: u32,
}

impl Shelf {
    pub fn new(on_hand: u32) -> Self {
        Shelf { on_hand }
    }

    pub fn take(&mut self, n: u32) {
        self.on_hand -= n;
    }

    /// Receive a delivery of `cases` full cases plus `loose` single items.
    pub fn receive(&mut self, cases: u32, loose: u32) {
        let slot = &mut self.on_hand;
        std::thread::scope(|s| {
            s.spawn(|| restock::add(slot, cases, loose));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_reduces_stock() {
        let mut s = Shelf::new(10);
        s.take(4);
        assert_eq!(s.on_hand, 6);
    }

    #[test]
    fn loose_items_arrive() {
        let mut s = Shelf::new(0);
        s.receive(0, 7);
        assert_eq!(s.on_hand, 7);
    }

    #[test]
    fn a_delivery_of_two_cases_and_three_loose() {
        let mut s = Shelf::new(5);
        s.receive(2, 3);
        if std::env::var("INVENTORY_SHOW_ADDR").as_deref() == Ok("1") {
            eprintln!("M58-ADDR on_hand {:p}", &s.on_hand);
        }
        assert_eq!(s.on_hand, 32);
    }
}
