#![allow(clippy::needless_range_loop)]
use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free Two-Tier Transposition Table for caching branch results across threads.
///
/// Packs a 45-bit Zobrist signature, an 18-bit cost value, and a 1-bit `is_exact` flag
/// into a single AtomicU64 (total: 64 bits).
///
/// Two-tier replacement policy for collisions (different states at same slot):
///   - Slot 0: Always-replace (latest visited node).
///   - Slot 1: Depth-preferred (keep the entry with the larger value / harder subtree).
///
/// It relies on atomic operations to avoid stalling `rayon` threads.
/// Due to intentional relaxed atomics and TOCTOU races in insertion, updates can
/// occasionally be lost under contention. This is perfectly safe: losing an entry
/// merely forces a re-computation of a subtree rather than returning an invalid result.
pub struct GlobalCache {
    entries: Vec<AtomicU64>,
    mask: usize,
}

// Bit layout: [63..19] = hash45 (45 bits), [18] = is_exact, [17..0] = value (18 bits)
const VALUE_BITS: u32 = 18;
const VALUE_MASK: u64 = (1 << VALUE_BITS) - 1; // 0x3FFFF
const EXACT_BIT: u64 = 1 << VALUE_BITS; // bit 18
const HASH_SHIFT: u32 = VALUE_BITS + 1; // 19

impl GlobalCache {
    pub fn new(size: usize) -> Self {
        assert!(
            size.is_power_of_two(),
            "GlobalCache size must be a power of two, got {}",
            size
        );
        let mut entries = Vec::with_capacity(size * 2);
        entries.resize_with(size * 2, || AtomicU64::new(0));
        let mask = size - 1;
        Self { entries, mask }
    }

    #[inline]
    pub fn insert(&self, full_hash: u64, value: u32, is_exact: bool) {
        debug_assert!(
            value as u64 <= VALUE_MASK,
            "cache value {} overflows {}-bit field",
            value,
            VALUE_BITS
        );
        let base_index = (full_hash as usize & self.mask) << 1;
        let hash45 = full_hash >> HASH_SHIFT;
        let mut packed = (hash45 << HASH_SHIFT) | (value as u64 & VALUE_MASK);
        if is_exact {
            packed |= EXACT_BIT;
        }

        let slot1 = unsafe { self.entries.get_unchecked(base_index + 1) };
        let slot1_old = slot1.load(Ordering::Relaxed);

        if slot1_old != 0 {
            let old_hash45 = slot1_old >> HASH_SHIFT;
            if old_hash45 == hash45 {
                // Same state in slot 1: update if better (exact over lower bound, tighter lower bound)
                if packed == slot1_old {
                    return;
                }
                let old_exact = (slot1_old & EXACT_BIT) != 0;
                let old_value = (slot1_old & VALUE_MASK) as u32;
                if old_exact && !is_exact {
                    // Do nothing, old is exact and new is not
                    return;
                }
                if !old_exact && !is_exact && value < old_value {
                    // Do nothing, old lb is tighter
                    return;
                }
                slot1.store(packed, Ordering::Relaxed);
                return;
            } else {
                // Different state in slot 1. Compare values to see if new one is deeper.
                let old_value = (slot1_old & VALUE_MASK) as u32;
                if value >= old_value {
                    // New one is harder/deeper, so it replaces slot 1.
                    slot1.store(packed, Ordering::Relaxed);
                    return;
                }
            }
        } else {
            // Slot 1 is empty, put it there
            slot1.store(packed, Ordering::Relaxed);
            return;
        }

        // If we reach here, slot 1 was occupied by a DIFFERENT state that is HARDER.
        // So we fallback to slot 0 (always-replace).
        let slot0 = unsafe { self.entries.get_unchecked(base_index) };
        let slot0_old = slot0.load(Ordering::Relaxed);
        if slot0_old != 0 {
            let old_hash45 = slot0_old >> HASH_SHIFT;
            if old_hash45 == hash45 {
                if packed == slot0_old {
                    return;
                }
                let old_exact = (slot0_old & EXACT_BIT) != 0;
                let old_value = (slot0_old & VALUE_MASK) as u32;
                if old_exact && !is_exact {
                    return;
                }
                if !old_exact && !is_exact && value < old_value {
                    return;
                }
            }
        }
        slot0.store(packed, Ordering::Relaxed);
    }

    /// [`GlobalCache::insert`] for the candidate set `set`, whose hash is
    /// `full_hash`. With the `tt-audit` feature the write is also recorded
    /// (see [`audit`]); without it, `set` is unused.
    #[inline(always)]
    pub fn insert_set(&self, full_hash: u64, set: &[usize], value: u32, is_exact: bool) {
        #[cfg(feature = "tt-audit")]
        audit::record(set, value, is_exact);
        #[cfg(not(feature = "tt-audit"))]
        let _ = set;
        self.insert(full_hash, value, is_exact);
    }

    #[inline]
    pub fn get(&self, full_hash: u64) -> Option<(u32, bool)> {
        let base_index = (full_hash as usize & self.mask) << 1;
        let hash45 = full_hash >> HASH_SHIFT;

        // Check slot 1 (depth-preferred) first, since harder subtrees are queried more often.
        let packed1 = unsafe { self.entries.get_unchecked(base_index + 1) }.load(Ordering::Relaxed);
        if packed1 != 0 && (packed1 >> HASH_SHIFT) == hash45 {
            let is_exact = (packed1 & EXACT_BIT) != 0;
            let value = (packed1 & VALUE_MASK) as u32;
            return Some((value, is_exact));
        }

        // Check slot 0 (always-replace)
        let packed0 = unsafe { self.entries.get_unchecked(base_index) }.load(Ordering::Relaxed);
        if packed0 != 0 && (packed0 >> HASH_SHIFT) == hash45 {
            let is_exact = (packed0 & EXACT_BIT) != 0;
            let value = (packed0 & VALUE_MASK) as u32;
            return Some((value, is_exact));
        }

        None
    }
}

/// Every value the search writes to a transposition table, with the set it
/// is for, so a test can check each one against an independent solver. Each
/// thread appends to its own buffer, so recording adds no contention between
/// threads.
#[cfg(feature = "tt-audit")]
pub mod audit {
    use std::sync::{Arc, Mutex};

    /// One write: the candidate set (as written by the search), the value, and
    /// whether it was stored as exact (otherwise as a lower bound).
    pub type Entry = (Vec<usize>, u32, bool);

    static BUFFERS: Mutex<Vec<Arc<Mutex<Vec<Entry>>>>> = Mutex::new(Vec::new());

    thread_local! {
        static LOCAL: Arc<Mutex<Vec<Entry>>> = {
            let buf = Arc::new(Mutex::new(Vec::new()));
            BUFFERS.lock().unwrap().push(Arc::clone(&buf));
            buf
        };
    }

    pub(crate) fn record(set: &[usize], value: u32, is_exact: bool) {
        LOCAL.with(|buf| buf.lock().unwrap().push((set.to_vec(), value, is_exact)));
    }

    /// Removes and returns everything recorded so far, on every thread.
    pub fn take() -> Vec<Entry> {
        let buffers = BUFFERS.lock().unwrap();
        let mut out = Vec::new();
        for buf in buffers.iter() {
            out.append(&mut buf.lock().unwrap());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_exact_vs_lower_bound() {
        let cache = GlobalCache::new(1024);
        let hash = 0x123456789ABCDEF0;

        cache.insert(hash, 50, false);
        let (val, exact) = cache.get(hash).unwrap();
        assert_eq!(val, 50);
        assert!(!exact);

        cache.insert(hash, 55, true);
        let (val, exact) = cache.get(hash).unwrap();
        assert_eq!(val, 55);
        assert!(exact);

        cache.insert(hash, 40, false);
        let (val, exact) = cache.get(hash).unwrap();
        assert_eq!(val, 55);
        assert!(exact);
    }

    #[test]
    fn test_cache_collision_harder_subtree() {
        let cache = GlobalCache::new(1024);
        let hash1 = 0x1000000000000001;
        let hash2 = 0x2000000000000001; // collision

        cache.insert(hash1, 10, true);
        // Both hashes are placed in slot 1 initially since it's empty
        // Wait, hash1 goes to slot 1.
        // hash2 comes, slot 1 has hash1 (value 10). hash2 has value 20 (harder).
        // hash2 overwrites slot 1.
        cache.insert(hash2, 20, true);

        let (val, _) = cache.get(hash2).unwrap();
        assert_eq!(val, 20);

        // Now hash3 comes, value 5 (easier). Slot 1 has hash2 (value 20).
        // hash3 is easier, so it goes to slot 0!
        let hash3 = 0x3000000000000001;
        cache.insert(hash3, 5, true);

        // We should be able to retrieve BOTH hash2 and hash3!
        let (val, _) = cache.get(hash2).unwrap();
        assert_eq!(val, 20);
        let (val, _) = cache.get(hash3).unwrap();
        assert_eq!(val, 5);

        // hash4 comes, value 6 (easier than hash2). Goes to slot 0, overwriting hash3.
        let hash4 = 0x4000000000000001;
        cache.insert(hash4, 6, true);
        assert!(cache.get(hash3).is_none()); // Overwritten!
        let (val, _) = cache.get(hash4).unwrap();
        assert_eq!(val, 6);
        let (val, _) = cache.get(hash2).unwrap();
        assert_eq!(val, 20); // Still there!
    }

    #[test]
    fn test_cache_large_values() {
        let cache = GlobalCache::new(1024);
        let hash = 0xDEADBEEF00000001;
        for &v in &[0u32, 1, 1000, 4095, 4096, 10000, 100000, 262143] {
            cache.insert(hash, v, true);
            let (val, exact) = cache.get(hash).unwrap();
            assert_eq!(val, v);
            assert!(exact);
        }
    }
}

#[test]
#[should_panic(expected = "GlobalCache size must be a power of two, got 1000")]
fn test_cache_size_not_power_of_two() {
    let _cache = GlobalCache::new(1000);
}

/// The largest storable cost, (1 << 18) - 1, round-trips without overflowing
/// into the neighbouring fields.
#[test]
fn test_cache_stores_max_value() {
    let cache = GlobalCache::new(1024);
    let max_val = (1 << 18) - 1;
    cache.insert(12345, max_val, true);
    assert_eq!(cache.get(12345), Some((max_val, true)));
}
