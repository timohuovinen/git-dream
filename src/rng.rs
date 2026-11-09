//! Small, stable PRNG (SplitMix64) and string hashing. Implemented here rather
//! than taken from a crate so that agent proposals and dream samples stay
//! reproducible across dependency upgrades: replay relies on it.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }

    pub fn choose<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }

    /// k distinct indices out of 0..n, in draw order.
    pub fn sample(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..n).collect();
        let k = k.min(n);
        for i in 0..k {
            let j = i + self.below(n - i);
            idx.swap(i, j);
        }
        idx.truncate(k);
        idx
    }

    /// Index drawn with probability proportional to `w`.
    pub fn weighted(&mut self, w: &[f64]) -> usize {
        let total: f64 = w.iter().sum();
        let mut x = self.f64() * total;
        for (i, wi) in w.iter().enumerate() {
            if x < *wi {
                return i;
            }
            x -= wi;
        }
        w.len() - 1
    }
}

/// Stable 64-bit hash of a string (FNV-1a, then SplitMix64 finalised).
pub fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Rng::new(h).next_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable() {
        // Pinned values: changing them changes every recorded agent proposal.
        assert_eq!(hash("git-dream"), 11_182_977_865_865_597_688);
        let mut r = Rng::new(42);
        let a: Vec<u64> = (0..3).map(|_| r.next_u64()).collect();
        assert_eq!(a, [13_679_457_532_755_275_413, 2_949_826_092_126_892_291, 5_139_283_748_462_763_858]);
        let s = Rng::new(7).sample(10, 4);
        assert_eq!(s.len(), 4);
        let mut d = s.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), 4);
    }
}
