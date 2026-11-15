use std::hash::{BuildHasherDefault, Hasher};

/// A fast hasher for keys that are one integer, such as a [`Uid`](crate::ecs::entity::Uid).
///
/// It folds a 128-bit product, so every input bit reaches the low bits a table indexes by:
/// uids that differ only in their incarnation still spread. It does not resist flooding, so
/// keep it to keys an untrusted peer does not choose.
#[derive(Clone, Copy, Default)]
pub(crate) struct FoldHasher(u64);

pub(crate) type FoldBuildHasher = BuildHasherDefault<FoldHasher>;

const MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;

#[inline(always)]
const fn fold(x: u64) -> u64 {
    let product = x as u128 * MULTIPLIER as u128;
    product as u64 ^ (product >> 64) as u64
}

impl Hasher for FoldHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    #[inline(always)]
    fn write_u64(&mut self, n: u64) {
        self.0 = fold(self.0 ^ n);
    }

    #[inline(always)]
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }

    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::hash::BuildHasher;

    use super::*;

    #[test]
    fn fold_hasher_spreads_low_bits() {
        let build = FoldBuildHasher::default();
        // equal serials, incarnations apart: only the high half differs
        let buckets: HashSet<u64> = (1..=64u64)
            .map(|incarnation| build.hash_one(incarnation << 32) & 0xff)
            .collect();
        assert!(buckets.len() > 32, "{} buckets", buckets.len());

        let buckets: HashSet<u64> = (0..64u64)
            .map(|serial| build.hash_one(1 << 32 | serial) & 0xff)
            .collect();
        assert!(buckets.len() > 32, "{} buckets", buckets.len());
    }

    #[test]
    fn fold_hasher_is_deterministic() {
        let build = FoldBuildHasher::default();
        assert_eq!(build.hash_one(42u64), build.hash_one(42u64));
        assert_ne!(build.hash_one(42u64), build.hash_one(43u64));
    }
}
