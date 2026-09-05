#![allow(clippy::needless_range_loop)]
use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free Transposition Table for caching branch results across threads.
///
/// Packs a 45-bit Zobrist signature, an 18-bit cost value, and a 1-bit `is_exact` flag
/// into a single AtomicU64 (total: 64 bits).
///
/// 18 bits supports costs up to 262143, safely covering any realistic Wordle sub-bucket.
/// (The full N=2340 optimal cost is ~6500; no single cached sub-bucket exceeds this.)
///
/// Replacement policy for collisions (different states at same slot):
///   keep the entry with the larger value (harder subtree → more valuable to cache).
pub struct GlobalCache {
    entries: Vec<AtomicU64>,
}

// Bit layout: [63..19] = hash45 (45 bits), [18] = is_exact, [17..0] = value (18 bits)
const VALUE_BITS: u32 = 18;
const VALUE_MASK: u64 = (1 << VALUE_BITS) - 1; // 0x3FFFF
const EXACT_BIT: u64 = 1 << VALUE_BITS;         // bit 18
const HASH_SHIFT: u32 = VALUE_BITS + 1;          // 19

impl GlobalCache {
    pub fn new(size: usize) -> Self {
        let mut entries = Vec::with_capacity(size);
        entries.resize_with(size, || AtomicU64::new(0));
        Self { entries }
    }

    #[inline]
    pub fn insert(&self, full_hash: u64, value: u32, is_exact: bool) {
        debug_assert!(
            value as u64 <= VALUE_MASK,
            "cache value {} overflows {}-bit field",
            value,
            VALUE_BITS
        );
        let index = (full_hash as usize) & (self.entries.len() - 1);
        let hash45 = full_hash >> HASH_SHIFT;
        let mut packed = (hash45 << HASH_SHIFT) | (value as u64 & VALUE_MASK);
        if is_exact {
            packed |= EXACT_BIT;
        }

        let old = self.entries[index].load(Ordering::Relaxed);
        if old != 0 {
            let old_hash45 = old >> HASH_SHIFT;
            if old_hash45 != hash45 {
                // Different state: keep the one with the larger value (harder subtree)
                let old_value = (old & VALUE_MASK) as u32;
                if value < old_value {
                    return;
                }
            } else {
                // Same state: keep exact bound over lower bound
                let old_exact = (old & EXACT_BIT) != 0;
                if old_exact && !is_exact {
                    return;
                }
            }
        }

        self.entries[index].store(packed, Ordering::Relaxed);
    }

    #[inline]
    pub fn get(&self, full_hash: u64) -> Option<(u32, bool)> {
        let index = (full_hash as usize) & (self.entries.len() - 1);
        let packed = self.entries[index].load(Ordering::Relaxed);
        let hash45 = full_hash >> HASH_SHIFT;

        if packed != 0 && (packed >> HASH_SHIFT) == hash45 {
            let is_exact = (packed & EXACT_BIT) != 0;
            let value = (packed & VALUE_MASK) as u32;
            Some((value, is_exact))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_exact_vs_lower_bound() {
        let cache = GlobalCache::new(1024);
        let hash = 0x123456789ABCDEF0;

        // Insert lower bound
        cache.insert(hash, 50, false);
        let (val, exact) = cache.get(hash).unwrap();
        assert_eq!(val, 50);
        assert!(!exact);

        // Overwrite with exact bound
        cache.insert(hash, 55, true);
        let (val, exact) = cache.get(hash).unwrap();
        assert_eq!(val, 55);
        assert!(exact);

        // Attempt to overwrite exact with lower bound (should be ignored)
        cache.insert(hash, 40, false);
        let (val, exact) = cache.get(hash).unwrap();
        assert_eq!(val, 55);
        assert!(exact);
    }

    #[test]
    fn test_cache_collision_harder_subtree() {
        let cache = GlobalCache::new(1024); // 10-bit index
        let hash1 = 0x1000000000000001; // index 1
        let hash2 = 0x2000000000000001; // index 1 (collision, different hash45)

        // Insert easier subtree
        cache.insert(hash1, 10, true);

        // Insert harder subtree (value 20 > 10)
        cache.insert(hash2, 20, true);

        // hash2 should have overwritten hash1
        assert!(cache.get(hash1).is_none());
        let (val, _) = cache.get(hash2).unwrap();
        assert_eq!(val, 20);

        // Insert easier subtree again (value 5 < 20)
        let hash3 = 0x3000000000000001; // index 1
        cache.insert(hash3, 5, true);

        // hash2 should still be there, hash3 ignored
        assert!(cache.get(hash3).is_none());
        let (val, _) = cache.get(hash2).unwrap();
        assert_eq!(val, 20);
    }

    #[test]
    fn test_cache_large_values() {
        // Verify values up to the 18-bit max (262143) round-trip correctly
        let cache = GlobalCache::new(1024);
        let hash = 0xDEADBEEF00000001;
        for &v in &[0u32, 1, 1000, 4095, 4096, 10000, 100000, 262143] {
            cache.insert(hash, v, true);
            let (val, exact) = cache.get(hash).unwrap();
            assert_eq!(val, v, "round-trip failed for value {}", v);
            assert!(exact);
        }
    }
}
