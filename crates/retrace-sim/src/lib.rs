pub struct Rng { s: [u64;4] }
impl Rng {
    pub fn seed(seed: u64) -> Rng {
        // SplitMix64 to fill the state deterministically.
        let mut z = seed; let mut s = [0u64;4];
        for i in 0..4 {
            z = z.wrapping_add(0x9E3779B97F4A7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
            s[i] = x ^ (x >> 31);
        }
        Rng { s }
    }
    pub fn next_u64(&mut self) -> u64 {
        let r = self.s[0].wrapping_add(self.s[3]).rotate_left(23).wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0]; self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2]; self.s[0] ^= self.s[3];
        self.s[2] ^= t; self.s[3] = self.s[3].rotate_left(45);
        r
    }
    pub fn below(&mut self, n: u64) -> u64 { if n==0 {0} else { self.next_u64() % n } }
}

#[derive(Debug, Clone)]
pub enum Fault { None, TruncateAfter(usize), FlipByteInLastRecord }

pub fn pick_fault(rng: &mut Rng, record_count: usize) -> Fault {
    match rng.below(3) {
        0 => Fault::None,
        1 if record_count > 0 => Fault::TruncateAfter(rng.below(record_count as u64) as usize),
        _ => Fault::FlipByteInLastRecord,
    }
}

pub fn apply_fault(bytes: &mut Vec<u8>, fault: &Fault, record_offsets: &[usize]) {
    match fault {
        Fault::None => {}
        Fault::TruncateAfter(k) => { let cut = *record_offsets.get(*k).unwrap_or(&bytes.len()); bytes.truncate(cut); }
        Fault::FlipByteInLastRecord => { if let Some(b) = bytes.last_mut() { *b ^= 0xff; } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rng_is_deterministic_per_seed() {
        let a: Vec<u64> = (0..5).scan(Rng::seed(0xC0FFEE), |r,_| Some(r.next_u64())).collect();
        let b: Vec<u64> = (0..5).scan(Rng::seed(0xC0FFEE), |r,_| Some(r.next_u64())).collect();
        assert_eq!(a, b);
        assert_ne!(a, (0..5).scan(Rng::seed(0xC0FFEF), |r,_| Some(r.next_u64())).collect::<Vec<_>>());
    }
    #[test]
    fn truncate_fault_drops_tail() {
        let mut bytes = vec![0u8; 100];
        apply_fault(&mut bytes, &Fault::TruncateAfter(1), &[0,40,80]);
        assert_eq!(bytes.len(), 40);
    }
}
