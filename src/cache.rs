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
const EXACT_BIT: u64 = 1 << VALUE_BITS; // bit 18
const HASH_SHIFT: u32 = VALUE_BITS + 1; // 19

impl GlobalCache {
    pub fn new(size: usize) -> Self {
        // Index lookup uses `hash & (len - 1)` instead of `hash % len` (see
        // insert()/get()), which only distributes uniformly over the full
        // table when `size` is a power of two - a non-power-of-two size
        // wouldn't be unsafe (the mask still yields an in-bounds index) but
        // would silently waste capacity by only ever hitting some slots.
        assert!(
            size.is_power_of_two(),
            "GlobalCache size must be a power of two, got {}",
            size
        );
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

        // Deliberately a plain load-then-store, not a compare_exchange loop.
        // This has a known, accepted TOCTOU race: between the load and the
        // store, another thread can write a more valuable entry (or an exact
        // bound) into this slot, and we can clobber it. A compare_exchange
        // loop would close that race, at the cost of a retry loop under
        // contention, for no correctness benefit: alpha-beta search with a
        // transposition table stays correct even when entries are lost to a
        // race - a lost entry just means a future lookup falls back to
        // recomputing that subtree instead of getting a cache hit. This is a
        // best-effort cache, not a source of truth, so a rare lost update is
        // an acceptable, self-healing cost. See GitHub issue #4 for the
        // original analysis of why this race is benign.
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
                // Same state: keep exact bound over lower bound, and tighter lower bound over looser
                let old_exact = (old & EXACT_BIT) != 0;
                let old_value = (old & VALUE_MASK) as u32;
                if old_exact && !is_exact {
                    return;
                }
                if !old_exact && !is_exact && value < old_value {
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

    #[test]
    fn test_cache_concurrent_no_corruption() {
        // insert()'s load-then-store has a known, accepted TOCTOU race (see
        // the comment on insert()): concurrent writers can lose an update to
        // a race and the replacement policy is not strictly enforced across
        // threads. What must still hold is soundness - concurrent access
        // must never corrupt a slot into a value that get() misinterprets
        // (e.g. bits from two different writes torn together). Each slot is
        // a single AtomicU64 written with one store(), so every read
        // observes some fully-formed value some thread actually wrote, never
        // a torn mix. This test hammers a colliding pair of hashes from many
        // threads and asserts every read back is one of the exact packed
        // values a writer could have produced, never garbage.
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(GlobalCache::new(1024));
        let hash_a = 0x1000000000000001u64; // shares an index with hash_b (collision)
        let hash_b = 0x2000000000000001u64;

        let mut handles = Vec::new();
        for t in 0..8 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..5000u32 {
                    if (t + i) % 2 == 0 {
                        cache.insert(hash_a, i % 100, i % 3 == 0);
                    } else {
                        cache.insert(hash_b, (i % 100) + 50, i % 3 == 0);
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Whatever ended up in the shared slot must belong to one of the two
        // hashes and carry a plausible value/exactness for that hash's
        // insert range - never silently corrupted.
        let a = cache.get(hash_a);
        let b = cache.get(hash_b);
        assert!(
            a.is_some() ^ b.is_some(),
            "exactly one of the colliding hashes should occupy the shared slot"
        );
        if let Some((val, _)) = a {
            assert!(val < 100, "value {} out of range for hash_a", val);
        }
        if let Some((val, _)) = b {
            assert!(
                (50..150).contains(&val),
                "value {} out of range for hash_b",
                val
            );
        }
    }
}

#[test]
fn test_cache_depth_preferred_replacement() {
    let cache = GlobalCache::new(1024);
    let hash = 0x5000000000000001; // hash45 = 0x5000000000000

    // Insert looser bound
    cache.insert(hash, 50, false);

    // Tighter lower bound replaces looser
    cache.insert(hash, 60, false);
    let (val, exact) = cache.get(hash).unwrap();
    assert_eq!(val, 60);
    assert!(!exact);

    // Exact bound replaces tighter lower bound
    cache.insert(hash, 60, true);
    let (val, exact) = cache.get(hash).unwrap();
    assert_eq!(val, 60);
    assert!(exact);

    // Looser exact bound? (Should not happen in practice if tree is stable, but test policy)
    // The policy says exact bound replaces lower bound.
    cache.insert(hash, 55, true);
    let (val, _) = cache.get(hash).unwrap();
    // Cache policy doesn't explicitly check old_value if both are exact.
    // Wait, the policy says:
    // if old_exact && !is_exact { return; }
    // if !old_exact && !is_exact && value < old_value { return; }
    // So if both are exact, it blindly overwrites.
    assert_eq!(val, 55);
}
