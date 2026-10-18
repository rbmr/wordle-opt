//! Lower-bound fact log for the optimality-certificate investigation.
//!
//! Compiled only with `--features certlog`, so the default build's search
//! code is untouched: every hook in the solver is a `cert_record!` that
//! expands to nothing without the feature. With the feature, recording is
//! still off until [`set_enabled`] turns it on, so one binary can time a
//! solve with and without logging.
//!
//! A fact is `(candidate set, value, exact)`: the search proved the set's
//! optimal cost is at least `value` (and, if `exact`, also found a policy of
//! that cost). Each thread appends to its own buffer; [`take_all`] merges
//! them into one map keeping the largest bound per set.

use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Facts from searches the shared global beta cut short. Kept apart: a cut
/// search has not proven what it returns.
static CUT: Mutex<Vec<CutFact>> = Mutex::new(Vec::new());

/// (sorted set, value, exact)
pub type CutFact = (Box<[u16]>, u32, bool);

/// Like [`record`], but a fact from a search cut short by the global beta is
/// set aside instead of logged (see [`take_cut`]).
#[inline]
pub fn record_tagged(site: u8, set: &[usize], value: u32, exact: bool, cut: bool) {
    if !cut {
        return record(site, set, value, exact);
    }
    if !ENABLED.load(Ordering::Relaxed) || set.len() <= 2 {
        return;
    }
    let mut v: Box<[u16]> = set.iter().map(|&c| c as u16).collect();
    v.sort_unstable();
    CUT.lock().unwrap().push((v, value, exact));
}

/// Drains the facts [`record_tagged`] set aside.
pub fn take_cut() -> Vec<CutFact> {
    std::mem::take(&mut *CUT.lock().unwrap())
}

#[derive(Default)]
struct Buf {
    members: Vec<u16>,
    /// (set length, value, exact, site)
    recs: Vec<(u16, u32, bool, u8)>,
}

static REGISTRY: Mutex<Vec<Arc<Mutex<Buf>>>> = Mutex::new(Vec::new());

thread_local! {
    static LOCAL: Arc<Mutex<Buf>> = {
        let b = Arc::new(Mutex::new(Buf::default()));
        REGISTRY.lock().unwrap().push(Arc::clone(&b));
        b
    };
}

static KSCAN_CALLS: AtomicU64 = AtomicU64::new(0);
static KSCAN_LOOKUPS: AtomicU64 = AtomicU64::new(0);
static NODESCAN_LOOKUPS: AtomicU64 = AtomicU64::new(0);

/// One branching-factor scan (`k` of a bucket) of `lookups` response reads,
/// an upper bound where the scan exits early.
pub fn count_kscan(lookups: usize) {
    KSCAN_CALLS.fetch_add(1, Ordering::Relaxed);
    KSCAN_LOOKUPS.fetch_add(lookups as u64, Ordering::Relaxed);
}

/// A scan of a node's guesses (partitioning every active guess), in response reads.
pub fn count_nodescan(lookups: usize) {
    NODESCAN_LOOKUPS.fetch_add(lookups as u64, Ordering::Relaxed);
}

/// (k-scan calls, k-scan lookups, node-scan lookups) since the last call.
pub fn take_work() -> (u64, u64, u64) {
    (
        KSCAN_CALLS.swap(0, Ordering::Relaxed),
        KSCAN_LOOKUPS.swap(0, Ordering::Relaxed),
        NODESCAN_LOOKUPS.swap(0, Ordering::Relaxed),
    )
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

#[inline]
pub fn record(site: u8, set: &[usize], value: u32, exact: bool) {
    if !ENABLED.load(Ordering::Relaxed) || set.len() <= 2 {
        return;
    }
    LOCAL.with(|b| {
        let mut b = b.lock().unwrap();
        b.members.extend(set.iter().map(|&c| c as u16));
        b.recs.push((set.len() as u16, value, exact, site));
    });
}

/// Every recorded fact, merged.
pub struct Facts {
    /// Largest proven lower bound per sorted candidate set.
    pub bound: FxHashMap<Box<[u16]>, u32>,
    /// Call site (and exactness) that recorded each set's largest bound.
    pub source: FxHashMap<Box<[u16]>, (u8, bool)>,
    pub records: u64,
    pub exact_records: u64,
    pub member_bytes: u64,
}

/// Drains all thread buffers. Call only once no search is running.
pub fn take_all() -> Facts {
    let mut facts = Facts {
        bound: FxHashMap::default(),
        source: FxHashMap::default(),
        records: 0,
        exact_records: 0,
        member_bytes: 0,
    };
    for buf in REGISTRY.lock().unwrap().iter() {
        let b = std::mem::take(&mut *buf.lock().unwrap());
        facts.member_bytes += 2 * b.members.len() as u64;
        let mut off = 0;
        for &(len, value, exact, site) in &b.recs {
            let mut set: Box<[u16]> = b.members[off..off + len as usize].into();
            off += len as usize;
            set.sort_unstable();
            facts.records += 1;
            facts.exact_records += exact as u64;
            let e = facts.bound.entry(set.clone()).or_insert(0);
            if value >= *e {
                *e = value;
                facts.source.insert(set, (site, exact));
            }
        }
    }
    facts
}
