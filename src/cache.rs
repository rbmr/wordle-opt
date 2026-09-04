use std::sync::atomic::{AtomicU64, Ordering};

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
