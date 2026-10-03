//! Fast hasher for order-id keyed maps.
//!
//! Uses the SplitMix64 finalizer (xor-shift-multiply), so both the low bits
//! (bucket index) and high bits (hashbrown control bytes) are well mixed for any
//! id pattern, including sequential ids. Not HashDoS resistant; the engine is a
//! replay simulator fed trusted input (see DECISIONS.md #13).

use std::hash::{BuildHasherDefault, Hasher};

#[derive(Debug, Default, Clone, Copy)]
pub struct IdHasher {
    state: u64,
}

pub type IdBuildHasher = BuildHasherDefault<IdHasher>;

/// SplitMix64 finalizer (Steele, Lea, Flood 2014).
#[inline]
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Hasher for IdHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.state
    }

    #[inline]
    fn write_u64(&mut self, x: u64) {
        self.state = mix(self.state ^ x);
    }

    /// Generic fallback (not used by `u64` keys): 8-byte chunks, then length.
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(buf));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(buf));
        }
        self.write_u64(bytes.len() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::hash::BuildHasher;

    fn hash(x: u64) -> u64 {
        IdBuildHasher::default().hash_one(x)
    }

    #[test]
    fn matches_splitmix64_output() {
        // SplitMix64 from seed 0 outputs mix(0 + golden gamma) first.
        let expected = crate::loadgen::SplitMix64::new(0).next_u64();
        assert_eq!(hash(0x9E37_79B9_7F4A_7C15), expected);
    }

    #[test]
    fn sequential_ids_spread_over_low_bits() {
        // 256 sequential ids into 256 buckets by low byte: a random function
        // fills ~162 (256 * (1 - 1/e)); identity-like hashes would fill 256 or
        // collapse badly for strided ids. Check both sequential and strided.
        for stride in [1u64, 256, 4096] {
            let buckets: HashSet<u64> = (0..256).map(|i| hash(i * stride) & 0xFF).collect();
            assert!(buckets.len() > 130, "stride {stride}: {}", buckets.len());
            assert!(buckets.len() < 200, "stride {stride}: {}", buckets.len());
        }
    }

    #[test]
    fn byte_fallback_is_deterministic_and_length_sensitive() {
        let mut a = IdHasher::default();
        a.write(b"abc");
        let mut b = IdHasher::default();
        b.write(b"abc\0");
        assert_ne!(a.finish(), b.finish());
        let mut c = IdHasher::default();
        c.write(b"abc");
        assert_eq!(a.finish(), c.finish());
    }
}
