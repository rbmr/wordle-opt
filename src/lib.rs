#![allow(clippy::needless_range_loop)]

pub mod core;
pub mod dict;
pub mod heuristic;
pub mod matrix;
pub mod naive;
pub mod policy;
pub mod policy_stats;
pub mod solver;
pub mod verify;

use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::policy::{BuildOptions, Strategy};
use std::collections::HashMap;
use std::env;
use std::time::Instant;

/// Short git commit hash of the working tree, or "unknown" if it can't be
/// determined. Every run is stamped with this so a number in
/// `run_history.md` can always be traced back to the exact code that
/// produced it - a claim about performance is only as good as being able to
/// check it against the commit it came from.
///
/// Prefers `WORDLE_OPT_COMMIT` (set by `deploy_and_bench.sh` from the
/// *local* repo before rsyncing) over running `git rev-parse` here: the
/// remote build directory deliberately has no `.git` (rsync excludes it), so
/// `git rev-parse` there either fails or - worse - silently reports the HEAD
/// of some unrelated, stale git checkout that happens to sit in the same
/// directory, mislabeling a run with a commit hash that isn't what actually
/// ran. Only a git repo that is *actually the source of the code running
/// right now* can answer this correctly, which on the remote is never true.
fn git_commit_hash() -> String {
    if let Ok(commit) = std::env::var("WORDLE_OPT_COMMIT")
        && !commit.trim().is_empty()
    {
        return commit.trim().to_string();
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Whether `host` is a hostname of the designated compute machine. `pc01` is
/// the bare-metal compute host since the October 2026 migration; the older
/// names are kept so historical setups keep working.
fn is_compute_hostname(host: &str) -> bool {
    matches!(host, "pc01" | "ubuntu-main" | "compute")
}

/// Whether this process is running on the designated high-memory/high-core
/// compute host, as opposed to a laptop/dev machine. Gates both the
/// transposition table size and which CLI operations are
/// allowed to run locally (`run_full`, large optimal tree builds) - kept as a
/// single check so the two decisions can't silently drift apart if the
/// compute host's name ever changes.
pub fn is_compute_host() -> bool {
    is_compute_hostname(&hostname())
}

/// The message to print when a build with no CUDA support runs on the compute
/// host, or `None` when there is nothing to warn about. Split out from the
/// printer so the wording and the host gating are testable without spawning a
/// process.
///
/// This is the runtime backstop for the failure that silently cost the pc01
/// reinstall its GPU: `build.rs` only sets `cuda_enabled` when `nvcc` is
/// present, so a compute host without the CUDA toolkit compiles every
/// `#[cfg(cuda_enabled)]` path away and runs CPU-only - invisibly, since the
/// build script's own warning does not reappear once its output is cached
/// (installing `nvcc` afterwards does not force a re-run).
fn compute_cuda_mismatch_warning(host: &str, cuda_enabled: bool) -> Option<String> {
    if is_compute_hostname(host) && !cuda_enabled {
        Some(format!(
            "WARNING: this is the compute host ('{host}') but the build has NO CUDA support: \
             nvcc was absent when build.rs ran, so every #[cfg(cuda_enabled)] path (the GPU) \
             compiled away and this run is CPU-only. Fix: install `nvidia-cuda-toolkit` and \
             `g++-13` on the compute host, then force build.rs to re-run (e.g. `touch \
             src/gpu_kernel.cu`) and rebuild."
        ))
    } else {
        None
    }
}

/// Prints [`compute_cuda_mismatch_warning`] for this host and build, if any.
fn warn_if_compute_without_cuda() {
    if let Some(msg) = compute_cuda_mismatch_warning(&hostname(), cfg!(cuda_enabled)) {
        eprintln!("{msg}");
    }
}

/// Draws one random subset of `size` distinct candidate indices (no
/// replacement) from `rng`, sorted. Shared by `benchmark-random` and
/// `diagnose` so both draw samples the same way.
pub(crate) fn sample_random_subset(
    rng: &mut fastrand::Rng,
    n_candidates: usize,
    size: usize,
) -> Vec<usize> {
    let mut subset = Vec::with_capacity(size);
    while subset.len() < size {
        let idx = rng.usize(0..n_candidates);
        if !subset.contains(&idx) {
            subset.push(idx);
        }
    }
    subset.sort_unstable();
    subset
}

/// splitmix64 finalizer - avalanches a u64 so nearby inputs don't produce
/// correlated outputs. Used to combine an independent (seed, size) pair into
/// a single RNG seed below.
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Derives the RNG stream that draws the subset for one (seed, size) pair.
///
/// Both `run_benchmark_random` and the old `run_diagnose` used to share one
/// `fastrand::Rng` sequentially across an entire `sizes` list, so the subset
/// drawn for a given size silently depended on what other sizes preceded it
/// in that specific invocation's list - two runs asking for "the same N"
/// could draw different subsets if their `sizes` lists differed, which
/// silently invalidated any comparison keyed only on (seed, N). This was
/// caught via `diagnose_history.md`: commit f618964 (`sizes=[750, 1000,
/// 1100, ...]`) and commit ca7f4a9 (`sizes=[1000, 1200]`) drew two different
/// N=1000 subsets under the identical seed, since N=1000 was the 2nd draw in
/// one run and the 1st in the other.
///
/// Deriving the stream from `(seed, size)` alone - independent of list
/// content or order - fixes this: the subset for a given size now depends
/// only on the seed and that size, never on what else was requested
/// alongside it.
fn subset_rng(seed: u64, size: usize) -> fastrand::Rng {
    let combined = mix64(seed) ^ mix64((size as u64).wrapping_mul(0x9E3779B97F4A7C15));
    fastrand::Rng::with_seed(mix64(combined))
}

/// Output paths for a full run, given the environment values they depend on.
/// They live in the home directory rather than the rsync target: the build
/// directory is deleted and rewritten by the next deploy, and a finished
/// run's policy tree and progress series must survive it.
fn full_output_paths_from(
    home: Option<std::ffi::OsString>,
    out: Option<String>,
    stats: Option<String>,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = home
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let out = out
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join("optimal-2340.json"));
    let stats = stats
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join("optimal-2340-progress.parquet"));
    (out, stats)
}

/// [`full_output_paths_from`] with the current environment:
/// `WORDLE_OPT_POLICY_OUT` / `WORDLE_OPT_POLICY_STATS` override the defaults.
fn full_output_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    full_output_paths_from(
        std::env::var_os("HOME"),
        std::env::var("WORDLE_OPT_POLICY_OUT").ok(),
        std::env::var("WORDLE_OPT_POLICY_STATS").ok(),
    )
}

fn run_full(matrix: &ResponseMatrix, dict: &Dictionary) {
    // Restored 2026-09-16: this guard was silently commented out in commit
    // ffb81d2 ("Perf: Prune branches using parent's max_k before full
    // capacity check") - a commit whose message describes an unrelated
    // pruning optimization and says nothing about disabling this check.
    // Without it, nothing stops `full` (an exhaustive N=2340 solve) from
    // being launched on the tiny 2-core/4GB `assistant` VM, which is
    // supposed to stay lightweight and available - see AGENTS.md and the
    // task guidance on why that machine must never run heavy computation.
    if !is_compute_host() {
        eprintln!(
            "HARD GUARD: full run must execute on the compute host (hostname 'pc01'). Use rsync + ssh, or deploy_and_bench.sh."
        );
        std::process::exit(1);
    }

    let all_candidates: Vec<usize> = (0..dict.candidates.len()).collect();
    let n_candidates = all_candidates.len();
    let commit = git_commit_hash();
    let host = hostname();
    println!(
        "Running full N={} optimal policy tree build... commit={} host={}",
        n_candidates, commit, host
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());

    // The policy JSON is the deliverable, and the progress series is what
    // makes day-over-day comparison possible. Both live in the home
    // directory rather than the build directory: the latter is deleted and
    // rewritten by the next deploy, and these files must survive it.
    let (out_path, stats_path) = full_output_paths();

    let observer = std::sync::Arc::new(crate::policy::BuildObserver::default());
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    apply_memory_bounds();
    let opts = BuildOptions {
        collect_samples: true,
        progress: true,
        // 1 << 27 units x 16 bytes = 2 GB. Deliberately smaller than the
        // cost-only full run's 512 M units (8 GB): this run also holds the
        // equivalence cache, and the first attempt was OOM-killed with both
        // at their maxima.
        cache_entries: 1 << 27,
        observer: Some(std::sync::Arc::clone(&observer)),
        ..Default::default()
    };
    let cpu_start = process_cpu_seconds();
    let start = Instant::now();
    let (progress_thread, progress_samples) =
        spawn_build_progress(&observer, std::time::Duration::from_secs(60), start, &done);
    let (tree, stats) = match crate::policy::build_policy_tree(
        matrix,
        dict,
        Strategy::Optimal,
        &all_candidates,
        &opts,
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("full: {e}");
            std::process::exit(1);
        }
    };
    let elapsed = start.elapsed();
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = progress_thread.join();
    let mut stats = stats;
    merge_progress_samples(&mut stats, &progress_samples);

    // Report the build's own numbers before touching the filesystem: a write
    // failure must not discard the result of a multi-hour run.
    println!("\n=== FULL POLICY TREE COMPLETE ===");
    println!("Strategy: {}", Strategy::Optimal.name());
    println!("Candidates: {}", n_candidates);
    println!("Nodes: {}", stats.summary.nodes);
    println!("Edges: {}", stats.summary.edges);
    println!("Leaves: {}", stats.summary.leaves);
    println!("Max depth: {}", stats.summary.max_depth);
    println!("Total cost: {}", stats.summary.total_cost);
    println!(
        "Mean guesses: {:.6}",
        stats.summary.total_cost as f64 / n_candidates as f64
    );
    println!(
        "Wall time: {:.3}s ({:.2}h)",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() / 3600.0
    );
    if let (Some(c0), Some(c1)) = (cpu_start, process_cpu_seconds()) {
        println!(
            "Avg busy cores: {:.1} (cpu {:.1}s over wall {:.1}s; GPU spin-waits count as busy)",
            (c1 - c0) / elapsed.as_secs_f64(),
            c1 - c0,
            elapsed.as_secs_f64()
        );
    }
    println!("States evaluated: {}", stats.summary.states_evaluated);
    println!("Guesses evaluated: {}", stats.summary.guesses_evaluated);
    println!("Bounds pruned: {}", stats.summary.pruned_by_bounds);
    println!("Equiv pruned: {}", stats.summary.pruned_by_equivalence);
    println!("Cache hits: {}", stats.summary.cache_hits);
    println!(
        "Dictionary hash: {}",
        crate::policy::hash_hex(tree.dictionary_hash)
    );

    // The solve is finished. Only now pay for the readable serialization, so
    // it adds no meaningful overhead to the solve itself.
    let serialized = match tree.to_readable(dict) {
        Ok(r) => r.to_json(),
        Err(e) => {
            eprintln!("full: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::write(&out_path, &serialized) {
        eprintln!("full: cannot write {}: {e}", out_path.display());
        std::process::exit(1);
    }

    // Self-validate the written bytes (round-trip through the reader), so a
    // full run can never report success on a tree the validator rejects.
    let report = match validate_bytes(&serialized) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("full: internal error: written tree failed validation: {e}");
            std::process::exit(1);
        }
    };
    if report.total_cost != stats.summary.total_cost {
        eprintln!(
            "full: internal error: validator total {} disagrees with builder total {}",
            report.total_cost, stats.summary.total_cost
        );
        std::process::exit(1);
    }

    let meta = crate::policy_stats::StatsMeta {
        strategy: Strategy::Optimal.name().to_string(),
        dictionary_hash: crate::policy::hash_hex(tree.dictionary_hash),
        commit: commit.clone(),
        num_guesses: dict.guesses.len() as u32,
        num_candidates: n_candidates as u32,
        root_candidates: n_candidates as u32,
    };
    if let Err(e) =
        crate::policy_stats::write_parquet(std::path::Path::new(&stats_path), &stats, &meta)
    {
        eprintln!("full: warning: could not write progress series: {e}");
    }

    println!("Wrote: {} ({} bytes)", out_path.display(), serialized.len());
    println!(
        "Stats: wrote {} progress sample(s) to {} (parquet)",
        stats.samples.len(),
        stats_path.display()
    );
    println!("Validation: OK (edge iff possible at every node)");
}

/// Minimal `--key value` / `--key=value` / `--flag` argument parser. Positional
/// arguments (used by `validate <file>`) are collected separately.
struct Flags {
    values: HashMap<String, String>,
    positional: Vec<String>,
}

impl Flags {
    fn parse(args: &[String]) -> Self {
        let mut values = HashMap::new();
        let mut positional = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if let Some(key) = a.strip_prefix("--") {
                let (k, v) = match key.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => {
                        if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                            i += 1;
                            (key.to_string(), args[i].clone())
                        } else {
                            (key.to_string(), "true".to_string())
                        }
                    }
                };
                values.insert(k, v);
            } else {
                positional.push(a.clone());
            }
            i += 1;
        }
        Self { values, positional }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }
}

/// Builds a `Dictionary` from `--guesses`/`--candidates` (defaulting to the
/// shipped lists), optionally reduced to a smaller candidate set:
/// - `--max-candidates N` takes the first `N` candidates (the same
///   deterministic first-N convention the golden tests use), or
/// - `--max-candidates N --sample-seed S` takes `N` candidates drawn from the
///   reproducible stream `subset_rng(S, N)` - a representative spread rather
///   than one alphabetically-adjacent slice.
fn load_dict_from_flags(flags: &Flags, max_candidates: usize) -> (Dictionary, String) {
    let guesses = flags.get_or("guesses", "words/guesses.txt");
    let candidates = flags.get_or("candidates", "words/candidates.txt");
    let full = Dictionary::load(guesses, candidates);
    let total = full.candidates.len();
    // The candidate selection is part of a run's identity - (commit, seed, size)
    // is what makes a timing reproducible - so it is described in the returned
    // string and printed by the caller, into the run's log.
    if max_candidates == 0 || max_candidates >= total {
        return (
            full,
            format!("full list ({total} candidates, no subset, no seed)"),
        );
    }
    let (idx, description) = match flags.get("sample-seed").and_then(|v| v.parse::<u64>().ok()) {
        Some(seed) => {
            let mut rng = subset_rng(seed, max_candidates);
            let idx = sample_random_subset(&mut rng, total, max_candidates);
            (
                idx,
                format!("seeded subset (seed={seed}, size={max_candidates} of {total})"),
            )
        }
        None => (
            (0..max_candidates).collect(),
            format!("deterministic first-{max_candidates} subset of {total} (no seed)"),
        ),
    };
    let subset: Vec<crate::core::Word> = idx.iter().map(|&i| full.candidates[i]).collect();
    (
        Dictionary::from_words(full.guesses.clone(), subset),
        description,
    )
}

/// Resident set size of this process, in MiB, from `/proc/self/status`. Used
/// by the progress thread so a long run's memory is visible while it runs
/// (an unbounded equivalence cache is the one thing that has OOM-killed a
/// full run). `None` where /proc is unavailable.
fn rss_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

/// Total CPU time (user + system) this process has consumed so far, in
/// seconds, from `/proc/self/stat`. Dividing a CPU-time delta by the wall-time
/// delta gives the average number of cores kept busy, which shows whether a
/// run is actually using the machine or is waiting on a few straggler
/// threads. Caveat: threads spinning on a GPU kernel (`cudaDeviceScheduleSpin`)
/// count as busy, so this is an upper bound on useful work. Assumes the
/// standard Linux clock tick of 100 Hz. `None` where /proc is unavailable.
fn process_cpu_seconds() -> Option<f64> {
    const CLK_TCK: f64 = 100.0;
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command name (field 2) may contain spaces; fields resume after the
    // last ')'. utime/stime are fields 14/15, i.e. indices 11/12 from there.
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) / CLK_TCK)
}

/// Applies the memory bounds a large run needs. The equivalence cache caches
/// one ~2 KB projection per distinct candidate set, so its default cap allows
/// tens of GB in the worst case (it OOM-killed the first full tree run);
/// `WORDLE_OPT_EQUIV_CACHE_CAP` overrides the per-shard default used here.
fn apply_memory_bounds() {
    let cap = std::env::var("WORDLE_OPT_EQUIV_CACHE_CAP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(4_096);
    crate::solver::set_equiv_cache_cap(cap);
}

/// Spawns a thread that reports a build's live counters every `interval`, to
/// stderr and as [`crate::policy::ProgressSample`]s. The builder only samples
/// at node boundaries, so without this a long node evaluation (the root scan
/// can run for hours) produces no output and no time series at all.
///
/// The caller sets `done` when the build returns, joins the thread, and merges
/// the returned samples into the build's own series with
/// [`merge_progress_samples`].
fn spawn_build_progress(
    observer: &std::sync::Arc<crate::policy::BuildObserver>,
    interval: std::time::Duration,
    start: Instant,
    done: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> (
    std::thread::JoinHandle<()>,
    std::sync::Arc<std::sync::Mutex<Vec<crate::policy::ProgressSample>>>,
) {
    use std::sync::atomic::Ordering;

    let observer = std::sync::Arc::clone(observer);
    let done = std::sync::Arc::clone(done);
    let samples = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let thread_samples = std::sync::Arc::clone(&samples);
    let handle = std::thread::spawn(move || {
        // Wake in short ticks so the caller's join returns promptly, but only
        // record a sample once per `interval`. A short interval shortens the
        // tick with it, so the cadence is honored exactly.
        let tick = interval.min(std::time::Duration::from_millis(250));
        // Previous (wall seconds, cpu seconds), for the per-interval average
        // number of busy cores.
        let mut prev_wall = start.elapsed().as_secs_f64();
        let mut prev_cpu = process_cpu_seconds();
        loop {
            let mut waited = std::time::Duration::ZERO;
            while waited < interval {
                std::thread::sleep(tick);
                if done.load(Ordering::Relaxed) {
                    return;
                }
                waited += tick;
            }
            let m = &observer.metrics;
            let t = &observer.tree;
            let sample = crate::policy::ProgressSample {
                elapsed_s: start.elapsed().as_secs_f64(),
                nodes: t.nodes.load(Ordering::Relaxed) as u64,
                edges: t.edges.load(Ordering::Relaxed) as u64,
                frontier: t.frontier.load(Ordering::Relaxed) as u64,
                depth: t.depth.load(Ordering::Relaxed) as u32,
                leaves: t.leaves.load(Ordering::Relaxed) as u64,
                // Only the builder's node-boundary samples carry a pick-time
                // total; it is not tracked live.
                pick_ms_total: 0.0,
                states_evaluated: m.states_evaluated.load(Ordering::Relaxed) as u64,
                guesses_evaluated: m.guesses_evaluated.load(Ordering::Relaxed) as u64,
                cache_hits: m.cache_hits.load(Ordering::Relaxed) as u64,
                cache_misses: m.cache_misses.load(Ordering::Relaxed) as u64,
                pruned_by_bounds: m.pruned_by_bounds.load(Ordering::Relaxed) as u64,
                pruned_by_equivalence: m.pruned_by_equivalence.load(Ordering::Relaxed) as u64,
            };
            let cpu_now = process_cpu_seconds();
            let busy_cores = match (prev_cpu, cpu_now) {
                (Some(p), Some(c)) if sample.elapsed_s > prev_wall => {
                    format!("{:.1}", (c - p) / (sample.elapsed_s - prev_wall))
                }
                _ => "?".into(),
            };
            prev_wall = sample.elapsed_s;
            prev_cpu = cpu_now;
            eprintln!(
                "[progress] elapsed={:.0}s nodes={} states={} guesses={} root_guesses={} cache_hits={} bounds_pruned={} busy_cores={} rss_mb={}",
                sample.elapsed_s,
                sample.nodes,
                sample.states_evaluated,
                sample.guesses_evaluated,
                m.root_guesses_done.load(Ordering::Relaxed),
                sample.cache_hits,
                sample.pruned_by_bounds,
                busy_cores,
                rss_mb()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into())
            );
            thread_samples
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(sample);
        }
    });
    (handle, samples)
}

/// Merges a progress thread's samples into a build's own series in time
/// order, so an exported series covers the whole run rather than only its
/// node-boundary phase.
fn merge_progress_samples(
    stats: &mut crate::policy::BuildStats,
    extra: &std::sync::Arc<std::sync::Mutex<Vec<crate::policy::ProgressSample>>>,
) {
    let mut all = std::mem::take(&mut stats.samples);
    let extra = extra.lock().unwrap_or_else(|e| e.into_inner());
    all.extend(extra.iter().cloned());
    all.sort_by(|a, b| {
        a.elapsed_s
            .partial_cmp(&b.elapsed_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    stats.samples = all;
}

/// `wordle-opt solve` - capture a fully determined policy tree for the chosen
/// strategy and write it as readable JSON, with an optional per-node
/// Parquet/NDJSON trace.
fn run_solve_cli(args: &[String]) {
    let flags = Flags::parse(args);

    let Some(output) = flags.get("output") else {
        eprintln!("solve: --output <path> is required");
        std::process::exit(2);
    };
    let Some(strategy) = flags.get("strategy").and_then(Strategy::parse) else {
        eprintln!(
            "solve: --strategy must be one of: {}",
            Strategy::all()
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
        std::process::exit(2);
    };
    let max_candidates: usize = flags
        .get("max-candidates")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let cache_entries: usize = flags
        .get("cache-entries")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1 << 22);
    let progress = !flags.values.contains_key("no-progress");
    let collect_samples = flags.get("stats").is_some();
    let stats_format = flags.get_or("stats-format", "parquet");

    let (dict, selection) = load_dict_from_flags(&flags, max_candidates);
    let n = dict.candidates.len();
    println!(
        "solve: strategy={} candidates={} guesses={} selection=\"{}\"",
        strategy.name(),
        n,
        dict.guesses.len(),
        selection
    );
    if strategy == Strategy::Optimal && !is_compute_host() && n > 500 {
        eprintln!(
            "WARNING: building an optimal policy tree for N={n} off the compute host will be very slow; \
             run it on `compute` (or use --max-candidates for an example)."
        );
    }

    println!("Computing response matrix...");
    let matrix = ResponseMatrix::new(&dict);
    let root: Vec<usize> = (0..n).collect();

    let observer = std::sync::Arc::new(crate::policy::BuildObserver::default());
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    apply_memory_bounds();
    let opts = BuildOptions {
        collect_samples,
        progress,
        cache_entries,
        observer: Some(std::sync::Arc::clone(&observer)),
        ..Default::default()
    };
    let cpu_start = process_cpu_seconds();
    let start = Instant::now();
    let (progress_thread, progress_samples) =
        spawn_build_progress(&observer, std::time::Duration::from_secs(15), start, &done);
    let (tree, stats) =
        match crate::policy::build_policy_tree(&matrix, &dict, strategy, &root, &opts) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("solve: {e}");
                std::process::exit(1);
            }
        };
    let elapsed = start.elapsed();
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = progress_thread.join();
    let mut stats = stats;
    merge_progress_samples(&mut stats, &progress_samples);

    // Report the build's own numbers before touching the filesystem: a write
    // failure must not discard the result of a long build.
    println!("=== POLICY TREE COMPLETE ===");
    println!("Strategy: {}", strategy.name());
    println!("Candidates: {} ({})", n, selection);
    println!("Nodes: {}", stats.summary.nodes);
    println!("Edges: {}", stats.summary.edges);
    println!("Leaves: {}", stats.summary.leaves);
    println!("Max depth: {}", stats.summary.max_depth);
    println!("Total cost: {}", stats.summary.total_cost);
    println!(
        "Mean guesses: {:.6}",
        stats.summary.total_cost as f64 / n as f64
    );
    println!("Build time: {:.3}s", elapsed.as_secs_f64());
    if let (Some(c0), Some(c1)) = (cpu_start, process_cpu_seconds()) {
        println!(
            "Avg busy cores: {:.1} (cpu {:.1}s over wall {:.1}s; GPU spin-waits count as busy)",
            (c1 - c0) / elapsed.as_secs_f64(),
            c1 - c0,
            elapsed.as_secs_f64()
        );
    }
    println!(
        "Dictionary hash: {}",
        crate::policy::hash_hex(tree.dictionary_hash)
    );

    // Serialize (readable JSON only).
    let serialized = match tree.to_readable(&dict) {
        Ok(r) => r.to_json(),
        Err(e) => {
            eprintln!("solve: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::write(output, &serialized) {
        eprintln!("solve: cannot write {output}: {e}");
        std::process::exit(1);
    }

    // Self-validate the written bytes (round-trip through the reader).
    let report = match validate_bytes(&serialized) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("solve: internal error: written tree failed validation: {e}");
            std::process::exit(1);
        }
    };
    if report.total_cost != stats.summary.total_cost {
        eprintln!(
            "solve: internal error: validator total {} disagrees with builder total {}",
            report.total_cost, stats.summary.total_cost
        );
        std::process::exit(1);
    }
    println!("Wrote: {output} ({} bytes)", serialized.len());
    println!("Validation: OK (edge iff possible at every node)");

    if let Some(stats_path) = flags.get("stats") {
        let meta = crate::policy_stats::StatsMeta {
            strategy: strategy.name().to_string(),
            dictionary_hash: crate::policy::hash_hex(tree.dictionary_hash),
            commit: git_commit_hash(),
            num_guesses: dict.guesses.len() as u32,
            num_candidates: n as u32,
            root_candidates: n as u32,
        };
        let res = match stats_format {
            "ndjson" | "json" => {
                crate::policy_stats::write_ndjson(std::path::Path::new(stats_path), &stats, &meta)
            }
            "parquet" => {
                crate::policy_stats::write_parquet(std::path::Path::new(stats_path), &stats, &meta)
            }
            other => {
                eprintln!("solve: --stats-format must be 'parquet' or 'ndjson' (got {other})");
                std::process::exit(2);
            }
        };
        match res {
            Ok(()) => println!(
                "Stats: wrote {} progress sample(s) to {stats_path} ({stats_format})",
                stats.samples.len()
            ),
            Err(e) => {
                eprintln!("solve: failed to write stats: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Parses `bytes` as a readable policy tree and validates it. The dictionary is
/// embedded in the file, so no external word lists are needed.
fn validate_bytes(
    bytes: &str,
) -> Result<crate::policy::ValidationReport, crate::policy::PolicyError> {
    let peek: serde_json::Value = serde_json::from_str(bytes)?;
    let format = peek.get("format").and_then(|v| v.as_str()).unwrap_or("");
    if format != crate::policy::FORMAT_READABLE {
        return Err(crate::policy::PolicyError::BadHeader(format!(
            "expected a readable policy tree ({:?}), found {format:?}",
            crate::policy::FORMAT_READABLE
        )));
    }
    let file = crate::policy::ReadableTreeFile::from_json(bytes)?;
    let dict = file.embedded_dictionary()?;
    let matrix = ResponseMatrix::new(&dict);
    let tree = file.to_tree()?;
    tree.validate(&matrix, &dict)
}

/// `wordle-opt validate <tree.json>` - validates a serialized policy tree.
fn run_validate_cli(args: &[String]) {
    let flags = Flags::parse(args);
    let Some(path) = flags.positional.first() else {
        eprintln!("validate: usage: wordle-opt validate <tree.json>");
        std::process::exit(2);
    };
    let bytes = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("validate: cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    let peek: serde_json::Value = match serde_json::from_str(&bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("validate: {path} is not valid JSON: {e}");
            std::process::exit(1);
        }
    };
    match validate_bytes(&bytes) {
        Ok(r) => {
            println!("=== POLICY TREE VALID ===");
            println!("File: {path}");
            println!(
                "Strategy: {}",
                peek.get("strategy").and_then(|v| v.as_str()).unwrap_or("?")
            );
            println!(
                "Dictionary hash: {}",
                peek.get("dictionary_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
            );
            println!("Nodes: {}", r.nodes);
            println!("Edges: {}", r.edges);
            println!("Leaves: {}", r.leaves);
            println!("Max depth: {}", r.max_depth);
            println!("Total cost: {}", r.total_cost);
            println!("Mean guesses: {:.6}", r.mean_guesses);
            println!("Depth histogram: {:?}", r.depth_histogram);
        }
        Err(e) => {
            eprintln!("INVALID: {e}");
            std::process::exit(1);
        }
    }
}

// Agent integrating IDA*
/// Command line interface entrypoint for solving, benchmarking, and verifying the optimal strategy.
pub fn run_cli() {
    warn_if_compute_without_cuda();
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("solve") => return run_solve_cli(&args[2..]),
        Some("validate") => return run_validate_cli(&args[2..]),
        _ => {}
    }

    println!("Loading dictionary...");
    let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
    println!(
        "Loaded {} guesses and {} candidates.",
        dict.guesses.len(),
        dict.candidates.len()
    );

    println!("Computing response matrix...");
    let num_u64s = dict.guesses.len().div_ceil(64);
    let mut all_guesses_bits = vec![u64::MAX; num_u64s];
    let rem = dict.guesses.len() % 64;
    if rem != 0 {
        all_guesses_bits[num_u64s - 1] = (1 << rem) - 1;
    }
    let start = Instant::now();
    let matrix = ResponseMatrix::new(&dict);
    let duration = start.elapsed();
    println!(
        "Computed matrix of size {}x{} in {:?}",
        matrix.num_guesses, matrix.num_candidates, duration
    );

    if args.len() > 1 && args[1] == "full" {
        run_full(&matrix, &dict);
    } else if args.len() > 1 && args[1] == "verify" {
        let fuzz_ok = verify::run_verification(&dict, &matrix, 50, 4);
        let stress_ok = verify::run_stress_test(&dict, &matrix);
        if !fuzz_ok || !stress_ok {
            std::process::exit(1);
        }
    } else {
        println!(
            "Usage: wordle-opt <solve --output <path> --strategy <optimal|min-remaining|max-freq> [--guesses P] [--candidates P] [--max-candidates N] [--sample-seed S] [--stats <path>] [--stats-format parquet|ndjson] [--no-progress] | full | validate <tree.json> | verify>"
        );
    }
}
pub mod cache;
pub mod parallel_depth;

#[cfg(cuda_enabled)]
pub mod gpu;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// The progress thread must record samples while a build is in flight and
    /// stop promptly once the build is done, so a caller can merge them into
    /// the build's own series.
    #[test]
    fn progress_thread_records_samples_and_stops() {
        let observer = Arc::new(crate::policy::BuildObserver::default());
        observer
            .metrics
            .states_evaluated
            .store(1234, Ordering::Relaxed);
        observer.tree.nodes.store(7, Ordering::Relaxed);

        let done = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        let (handle, samples): (
            std::thread::JoinHandle<()>,
            Arc<Mutex<Vec<crate::policy::ProgressSample>>>,
        ) = spawn_build_progress(&observer, std::time::Duration::from_millis(1), start, &done);

        std::thread::sleep(std::time::Duration::from_millis(50));
        done.store(true, Ordering::Relaxed);
        handle.join().expect("progress thread must not panic");

        let samples = samples.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            !samples.is_empty(),
            "expected at least one progress sample while the build ran"
        );
        let last = samples.last().unwrap();
        assert_eq!(last.nodes, 7);
        assert_eq!(last.states_evaluated, 1234);
        assert!(last.elapsed_s > 0.0);
    }

    /// Process CPU time is readable here (Linux) and grows when CPU is burned,
    /// so a busy-cores figure derived from it is meaningful.
    #[test]
    fn process_cpu_seconds_increases_with_work() {
        let before = process_cpu_seconds().expect("/proc/self/stat must be readable");
        let t0 = Instant::now();
        let mut acc = 0u64;
        // Burn ~0.3 s of CPU (well above the 10 ms clock tick).
        while t0.elapsed() < std::time::Duration::from_millis(300) {
            for i in 0..10_000u64 {
                acc = acc.wrapping_mul(31).wrapping_add(i);
            }
            std::hint::black_box(acc);
        }
        let after = process_cpu_seconds().expect("/proc/self/stat must be readable");
        assert!(
            after - before >= 0.1,
            "cpu time grew by only {}s after ~0.3s of work",
            after - before
        );
    }

    /// A run's identity is (commit, seed, candidate-set size): without the seed
    /// in the log, a subset timing cannot be reproduced or compared, because a
    /// different subset of the same size can perform very differently.
    #[test]
    fn candidate_selection_description_records_the_seed() {
        let flags =
            |args: &[&str]| Flags::parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let (_, d) = load_dict_from_flags(
            &flags(&["--max-candidates", "10", "--sample-seed", "7"]),
            10,
        );
        assert!(
            d.contains("seed=7"),
            "seeded subset must name its seed: {d}"
        );
        assert!(
            d.contains("size=10"),
            "seeded subset must name its size: {d}"
        );
        let (_, d) = load_dict_from_flags(&flags(&["--max-candidates", "10"]), 10);
        assert!(d.contains("no seed"), "an unseeded subset must say so: {d}");
        let (_, d) = load_dict_from_flags(&flags(&[]), 0);
        assert!(
            d.contains("full list"),
            "the full list must be described: {d}"
        );
    }

    /// The compute host must be recognised by its current name, or the full
    /// run / benchmark guards refuse to run and the transposition table
    /// silently falls back to its small non-compute size. Dev machines must
    /// not be recognised.
    #[test]
    fn compute_hostnames_are_recognised() {
        assert!(is_compute_hostname("pc01"));
        assert!(is_compute_hostname("compute"));
        assert!(is_compute_hostname("ubuntu-main"));
        assert!(!is_compute_hostname("helios"));
        assert!(!is_compute_hostname("assistant"));
        assert!(!is_compute_hostname(""));
    }

    /// A compute-host build that lost its CUDA toolkit must say so at runtime,
    /// since the GPU paths compile away silently. Dev machines building
    /// without CUDA are normal and must stay quiet, as must a compute host
    /// that does have CUDA.
    #[test]
    fn compute_host_without_cuda_warns() {
        assert!(compute_cuda_mismatch_warning("pc01", false).is_some());
        assert!(compute_cuda_mismatch_warning("compute", false).is_some());
        assert!(compute_cuda_mismatch_warning("pc01", true).is_none());
        assert!(compute_cuda_mismatch_warning("helios", false).is_none());
    }

    /// A full run's outputs default to the home directory (not the rsync
    /// target, which the next deploy deletes) and can be overridden.
    #[test]
    fn full_output_paths_default_to_home_and_honor_overrides() {
        use std::path::PathBuf;
        let (out, stats) = full_output_paths_from(Some("/home/robert".into()), None, None);
        assert_eq!(out, PathBuf::from("/home/robert/optimal-2340.json"));
        assert_eq!(
            stats,
            PathBuf::from("/home/robert/optimal-2340-progress.parquet")
        );

        let (out, stats) = full_output_paths_from(
            Some("/home/robert".into()),
            Some("/tmp/tree.json".to_string()),
            Some("/tmp/progress.ndjson".to_string()),
        );
        assert_eq!(out, PathBuf::from("/tmp/tree.json"));
        assert_eq!(stats, PathBuf::from("/tmp/progress.ndjson"));

        // Without HOME the defaults are still relative, never absolute paths
        // into someone else's directory.
        let (out, stats) = full_output_paths_from(None, None, None);
        assert_eq!(out, PathBuf::from("./optimal-2340.json"));
        assert_eq!(stats, PathBuf::from("./optimal-2340-progress.parquet"));
    }

    /// Samples from the progress thread and the builder must come out in time
    /// order, whichever source they came from.
    #[test]
    fn merged_progress_samples_are_time_ordered() {
        let mut stats = crate::policy::BuildStats {
            samples: vec![crate::policy::ProgressSample {
                elapsed_s: 10.0,
                nodes: 1,
                edges: 0,
                frontier: 0,
                depth: 0,
                leaves: 0,
                pick_ms_total: 0.0,
                states_evaluated: 0,
                guesses_evaluated: 0,
                cache_hits: 0,
                cache_misses: 0,
                pruned_by_bounds: 0,
                pruned_by_equivalence: 0,
            }],
            summary: crate::policy::BuildSummary {
                nodes: 0,
                edges: 0,
                leaves: 0,
                wins: 0,
                max_depth: 0,
                total_cost: 0,
                total_ms: 0.0,
                pick_ms_total: 0.0,
                states_evaluated: 0,
                guesses_evaluated: 0,
                cache_hits: 0,
                pruned_by_bounds: 0,
                pruned_by_equivalence: 0,
            },
        };
        let extra = Arc::new(Mutex::new(vec![
            crate::policy::ProgressSample {
                elapsed_s: 20.0,
                nodes: 2,
                edges: 1,
                frontier: 1,
                depth: 1,
                leaves: 0,
                pick_ms_total: 0.0,
                states_evaluated: 5,
                guesses_evaluated: 5,
                cache_hits: 0,
                cache_misses: 0,
                pruned_by_bounds: 0,
                pruned_by_equivalence: 0,
            },
            crate::policy::ProgressSample {
                elapsed_s: 5.0,
                nodes: 1,
                edges: 0,
                frontier: 0,
                depth: 0,
                leaves: 0,
                pick_ms_total: 0.0,
                states_evaluated: 1,
                guesses_evaluated: 1,
                cache_hits: 0,
                cache_misses: 0,
                pruned_by_bounds: 0,
                pruned_by_equivalence: 0,
            },
        ]));
        merge_progress_samples(&mut stats, &extra);
        let times: Vec<f64> = stats.samples.iter().map(|s| s.elapsed_s).collect();
        assert_eq!(times, vec![5.0, 10.0, 20.0]);
    }
}
