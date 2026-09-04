use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free Transposition Table for caching branch results across threads.
///
/// Uses `AtomicU64` to pack a 51-bit Zobrist signature, a 12-bit cost value, and a 1-bit `is_exact` flag.
/// A Depth-Preferred replacement policy is used to protect large subtrees from being evicted by shallow ones.
pub struct GlobalCache {
    entries: Vec<AtomicU64>,
}

impl GlobalCache {
    pub fn new(size: usize) -> Self {
        let mut entries = Vec::with_capacity(size);
        entries.resize_with(size, || AtomicU64::new(0));
        Self { entries }
    }

    #[inline]
    pub fn insert(&self, full_hash: u64, value: u32, is_exact: bool) {
        let index = (full_hash as usize) & (self.entries.len() - 1);
        let hash51 = full_hash >> 13;
        let mut packed = (hash51 << 13) | (value as u64 & 0xFFF);
        if is_exact {
            packed |= 1 << 12;
        }

        let old = self.entries[index].load(Ordering::Relaxed);
        if old != 0 {
            let old_hash51 = old >> 13;
            if old_hash51 != hash51 {
                // Different state: keep the one with the larger value (harder subtree)
                let old_value = (old & 0xFFF) as u32;
                if value < old_value {
                    return;
                }
            } else {
                // Same state: keep exact bound over upper bound
                let old_exact = (old & (1 << 12)) != 0;
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
        let hash51 = full_hash >> 13;

        if packed != 0 && (packed >> 13) == hash51 {
            let is_exact = (packed & (1 << 12)) != 0;
            let value = (packed & 0xFFF) as u32;
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
        let cache = GlobalCache::new(1024); // Size is 1024, index uses bottom 10 bits.
        let hash1 = 0x1000000000000001; // Index 1
        let hash2 = 0x2000000000000001; // Index 1 (collision, different hash51)

        // Insert easier subtree
        cache.insert(hash1, 10, true);
        
        // Insert harder subtree (value 20 > 10)
        cache.insert(hash2, 20, true);
        
        // hash2 should have overwritten hash1
        assert!(cache.get(hash1).is_none());
        let (val, _) = cache.get(hash2).unwrap();
        assert_eq!(val, 20);

        // Insert easier subtree again (value 5 < 20)
        let hash3 = 0x3000000000000001; // Index 1
        cache.insert(hash3, 5, true);
        
        // hash2 should still be there, hash3 ignored
        assert!(cache.get(hash3).is_none());
        let (val, _) = cache.get(hash2).unwrap();
        assert_eq!(val, 20);
    }
}
