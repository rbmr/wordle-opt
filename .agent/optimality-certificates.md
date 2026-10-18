# Optimality certificates: investigation

Status: shipped. Since 2026-10-18 `solve --certify`, `certify` and
`check-cert` are in main as a runtime option (no build feature), the checker
is its own crate (`crates/wordle-checker`), and the full certificate checks in
about 45 s on pc01. The rest of this file is the original investigation, from
branch `cert-investigation`; build-feature details below are historical. The question was whether, and how, `solve` should produce a
lower-bound proof that a small independent checker verifies, without making the
plain solve slower.

## Recommendation

**Always-output, as an opt-in mode** (`--features certlog`, `solve --certify`),
not on by default and not a separate second pass. Use it as a periodic audit
(after a change to solver, bound or cache logic, like the ignored full-scale
tests), not on every solve. It needs the transposition-table fix described under
Findings, which the solver needs anyway.

Why, in one line each, at full scale (N=2340, all numbers below):

- the plain solve is unchanged: the default build compiles every hook away, and
  branch and main time the same;
- a second pass redoes the whole solve (+101-103%): the solve *is* almost entirely
  refutation, so there is no small "refutation share" to redo;
- always-output costs about +1.4% in the solve plus +34% to fill the gaps its log
  leaves, then the same assembly and check as any certificate;
- shallow-only saves nothing: refutation below depth 2 is already free, and
  stopping there only turns cheap checks into 1,703 unchecked assumptions;
- the certificates already earned their keep: building them exposed a real
  exactness bug.

The check is the expensive part and that is inherent, not a prototype artifact:
each capacity claim needs a scan of every allowed guess over its set, the same
scan the solver did to prune with it, so check volume equals the solve's own
k-scan volume. A faster checker can only lower the constant.

## Results

All timings on the compute host, via `scripts/deploy_and_bench.sh`, one job at a
time. The verbatim result lines of series 1 and 3 are in
`optimality-certificates-raw.txt`. The N=500 and N=1000 interleaved solve timings
come from series 2, whose logs were lost in a restart; the numbers were captured
when the runs finished.

Series 1 and 2 ran before the history rewrite of 2026-10-16, so they carry old
hashes: main `3724ef7` (now `54ae412`) and branch `1b1e0f4` / `615e8ce` (rewritten
into the current branch). Series 3 ran as main `d7654fc` and branch `b9989cb`. The
solver code is the same in all of them: the branch adds the generator, the checker
and hooks that only exist with `--features certlog`.

### Plain solve: branch vs main (default build)

| N (seed) | branch | main | runs each |
|---|---|---|---|
| 500 (1) | 0.491-0.505 s | 0.490-0.500 s | 5, interleaved |
| 1000 (1) | 3.804-3.817 s | 3.806-3.815 s | 5, interleaved |
| 2340 | 35.626-36.345 s, mean 35.84 s | 35.446-36.210 s, mean 35.84 s | 10 / 9 |

The last pair ran back to back in one session: main `d7654fc` 35.906, 35.949,
35.952 s, then branch `b9989cb` 35.820, 35.864, 35.872 s. One earlier main round
ran on a different boot (35.446-35.628 s), which is why the ranges are wide.

No measurable slowdown. The `certlog` build with recording off: 0.497-0.504 s
(N=500), 3.820-3.827 s (N=1000), 35.906 s (N=2340, 1 run).

### Duplicate work: refutation pass vs solve

The second pass takes the solved tree and, at every node, searches each
alternative guess with beta fixed at the node's exact cost, using the solver's
own `min_guess_val` on a fresh TT.

| N | solve | pass | pass / solve |
|---|---|---|---|
| 500 | 0.49-0.50 s | 0.433-0.442 s | ~88% |
| 1000 | 3.80-3.82 s | 3.652-3.681 s | ~96% |
| 2340 | 35.4-36.3 s | 36.157-36.859 s | ~101-103% |

The same in implementation-independent units (the same solver counters in both):

| N | guesses evaluated, solve / pass | k-scans, solve / pass |
|---|---|---|
| 500 | 8,534 / 8,269 | 8,180 / 7,944 |
| 1000 | 16,798 / 12,961-13,127 | 51,810 / 51,502 |
| 2340 | 741,789 / 738,292-740,647 | 540,837 / 550,150 |

At N=2340 the pass spends 568-571 CPU-seconds on root alternatives, 2-3 on depth
1 and none deeper. The theory's split into upper-bound work, refutations and
waste does not hold for this solver: the best guess is found almost immediately
(greedy order plus the first-guess evaluation), and the rest of the solve is
refuting the other root guesses. Fixing beta at the exact value saves almost
nothing because the solve's beta is already exact by then.

### Facts proved vs facts the proof needs

| N | solve logs (records / distinct sets) | proof claims (capacity / expansion) | certificate |
|---|---|---|---|
| 500 | 62.7k / 62.3k | 13.0-13.2k (5.9k / 7.2k) | 195-198 KB |
| 1000 | 174.5k / 172.2k | 57.3k (35.6k / 21.7k) | 860 KB |
| 2340 | 1.35M / 1.28M | 630-633k (443-445k / 187-188k) | 9.4-9.5 MB |

So the proof uses a fifth to a half of what the solve proves. The log is small
(59 MB of member data at N=2340) and logging adds about 1% to the solve; the
larger cost of always-output is the gap fill (below).

### Always-output

| N | solve with logging | plain | gap fill |
|---|---|---|---|
| 500 | 0.494, 0.497 s | 0.49-0.50 s | none needed |
| 1000 | 3.841, 3.847 s | 3.80-3.82 s | 6 refutations, 0.09 s |
| 2340 | 36.360, 36.338 s | 35.4-36.3 s | 582-583 refutations, 12.2 s |

The solve's log has gaps where its search was cut short by the shared global
beta (1,074-1,078 such searches at N=2340): what it returns there is not all
proven, so it is not logged. Assembly reports the guesses it cannot refute from
the log, and exactly those are refuted again (recorded). Logging also keeps
cut-short results out of the TT (see Findings), which costs about 1.7% more
k-scans at N=2340. Logging the proven part of each cut-short result instead
would remove most gap fill, at the price of extra bookkeeping in the search;
not built or measured.

### Shallow-only (depth <= 2)

At N=500 and N=1000 every tree node with 3+ candidates is at depth <= 2, so
shallow equals full. At N=2340: pass 36.314-36.988 s (283 of 287 nodes),
assembly 226 s, 615k claims of which 1,703 assumed (all with |S| <= 16, within
brute-force reach), check 242 s and "valid given 1,703 assumed claims". It is the
full certificate minus a few cheap expansions.

### Assembly and check

| N | assembly | check | check volume | result |
|---|---|---|---|---|
| 500 | 1.9 s | 1.475-1.527 s | 6.7e9 reads | valid, optimal |
| 1000 | 13.9-14.2 s | 13.566-13.618 s | 4.65e10 reads | valid, optimal |
| 2340 | 226-233 s | 257 s (pass), 266 s (always-output) | 3.86-3.93e11 reads | valid, optimal |

Both full-scale certificates prove the bundled tree (cost 8,001) optimal.

The check volume matches the solve's k-scan volume (3.3e11 reads at N=2340):
every capacity claim needs `|G| x |S|` reads to compute `k`, and capacity claims
are 70% of the proof. The prototype checker runs at about 1.5e9 reads/s; the
solver's own optimized parallel scan measures 4.4-5.6e9 reads/s on the same host.
At that rate the full check would take roughly 70-90 s: an estimate, assuming a
checker as efficient as the solver's scan without its shortcuts. Assembly does
the same scans and could reuse the solve's own `k` values in always-output mode;
not built.

### Costs relative to a plain solve (N=2340, mean 35.84 s)

| option | added to the solve | assembly | check |
|---|---|---|---|
| no certificate | 0 | - | - |
| second pass | +36.2-36.9 s (+101-103%) | 226 s | 257 s |
| always-output | +0.5 s (+1.4%) and 12.2 s gap fill (+34%) | 233 s | 266 s |
| shallow-only | +36.3-37.0 s (+101-103%) | 226 s | 242 s, 1,703 claims assumed |

Assembly and check are prototype times (about 6-7 solves each); the estimate above
puts an optimized check near 2-2.5 solves. Extrapolating any of this to larger or
different word lists is not supported by these runs: the root-refutation share
and the gap count depend on how the root scan prunes.

## Findings

1. **Exactness bug: cut-short searches poison the transposition table.** In the
   parallel scans the shared global beta can tighten mid-search. `min_state_val`
   then stops early (phase-1 and phase-2 loops, the phase-2 gate) but still stores
   its result as exact (`best_val < beta`) or as a lower bound (`max(beta, ...)`).
   Later searches trust those entries. Measured at N=500 seed 1: logged "exact"
   values 1-2 above the true optimum (|S|=51: 118 vs 116; |S|=14: 31 vs 30, with a
   guess achieving 30 visible from buckets of size <= 2). Single-threaded: none in
   2 runs. Keeping cut-short results out of the TT removed every such fact in 5 of
   5 runs. The trees produced so far are still optimal (proven above), because the
   wrong values sat on losing alternatives, but nothing guarantees that.
2. **`heuristic::capacity_bound` is not a valid lower bound when `n >= k^2 - k + 3`.**
   A non-candidate guess gives up its own win for one more bucket, and from depth
   4 on that bucket is worth more. Example: n = 9, k = 3, cost 24 achievable, the
   formula says 25. An exact DP over abstract trees confirms the threshold for
   k <= 40. No real word list was found in that regime (12 random 2-3 letter
   instances, every subset of 12 candidates), but the solver uses the formula
   unconditionally. The checker uses the depth-3 truncation, which is provably
   sound and equal to the formula wherever the formula is valid.
3. **A log of what the solve uses is not a certificate.** Cut-short searches
   return values that are only partly proven, so always-output needs either gap
   filling (built) or proven-bound bookkeeping in the search (not built).


## What a certificate is

A list of claims `LB(S) >= b` about candidate sets `S`. Each set is named
explicitly: the root (every candidate), or one bucket of an earlier claim's set
(`parent claim, guess, response`), and the checker recomputes its members. Claims
are looked up by exact member list, never by hash alone. Each claim cites one rule:

- **base**: `|S| = 1` costs 1, `|S| = 2` costs 3;
- **capacity**: `b <= L3(|S|, k)`, `k` = the most distinct responses any allowed
  guess gives on `S`, computed by the checker;
- **expansion**: for every allowed guess (the checker enumerates the whole list
  itself) that splits `S`, `|S| + sum over non-win buckets of LB(bucket) >= b`,
  where `LB(bucket)` is the largest claim on exactly that bucket or the default
  `base`/`L3(|B|, k(S))`;
- **assumed**: unchecked; only a shallow certificate uses it, and the checker
  reports how many.

The root claim `LB(all) >= cost(tree)` plus the checker's own tree walk proves the
tree optimal. Soundness is induction on `|S|`: expansions only rely on strict
subsets.

Format: `WOCERT01`, FNV-1a digest of both word lists, list sizes, claim count,
then per claim a set definition (1 or 10 bytes), a `u32` bound and a rule byte.

## The checker (`crates/wordle-checker/src/lib.rs`)

About 450 lines, `std` + `serde_json` + `rayon` only. It reads the readable policy
JSON (word lists and tree) and the certificate. It has its own feedback function,
its own tree walker, and no search, TT, SIMD or solver code. It does use a
feedback table (`|G| x |C|` bytes) and skips a guess whose response row on `S`
it has already checked, by exact row comparison.

**Capacity rule.** The checker's capacity bound is `L3(n, k) = max(2n - 1,
3n - k - 1)`: the first guess wins at most one candidate, leaves at most `k - 1`
non-win buckets if it is a candidate (`k` if not, with no depth-1 win), each of
those wins at most one more at depth 2, and everything else costs at least 3.
This is `heuristic::capacity_bound` truncated at depth 3. The full
`capacity_bound` is **not** a valid lower bound once `n >= k^2 - k + 3` (see
Findings).

**Validation** (`tests/certificate.rs`): certificates written from brute-force
optima alone, on 12 small instances (bundled subsets and 3-5 letter alphabets),
must be accepted, and the solver's tree must cost the brute-force optimum. Then
every corruption must be rejected: each claim raised by one (one test per claim),
the root raised under each rule, a certificate computed as if one indispensable
guess were missing (6 instances), truncation, trailing bytes, another word list,
a forward parent reference, a missing root claim. `L3 <= optimum` is checked on
every memoized subset. A feature-gated test runs the real generator end to end
on the same 12 instances.

## The generator (`crates/wordle-solver/src/cert.rs`, `crates/wordle-solver/src/certlog.rs`)

Facts `(set, value, exact)` come from `cert_record!` hooks at every place the
search stores or returns a bound. The macro expands to nothing unless built with
`--features certlog`, so the default build's search code is unchanged.

- **Second pass** (`wordle-opt certify tree.json --cert out`): walks the solved
  tree; at every node with `|S| >= 3` it skips alternatives whose capacity sum
  already reaches the node's cost, then runs the solver's own `min_guess_val` on
  each remaining alternative with beta fixed at the node's exact cost, on a fresh
  TT. Any result below the cost fails the run.
- **Always-output** (`wordle-opt solve ... --certify out`): logs every fact the
  solve proves, keeps cut-short search results out of the TT while logging, and
  after assembly refutes again exactly the guesses the log could not justify.
- **Assembly** (both): top-down from the root, waves by decreasing set size, the
  same rule logic as the checker. A claim is requested on a bucket only where the
  default bound is not enough, and at the smallest target that still closes its
  parent's sum. Facts are never trusted: a fact the assembler cannot justify is
  reported as a failure.
- **Shallow** (`--max-depth 2`): refute only at tree nodes of depth <= 2, expand
  claims only at depth <= 2, and leave deeper claims assumed (or capacity, where
  that suffices).

## Follow-up (2026-10-17 evening)

The maintainer agreed: proof logging and checking stay fully opt-in.

**An invalid output, filed as issue #14.** A multi- vs single-threaded differential
fuzz on seeded subsets found inputs where the multi-threaded solve returns a valid
but suboptimal tree. The most reliable on `main` (d7654fc): `solve --guesses
words/candidates.txt --candidates words/candidates.txt --max-candidates 200
--sample-seed 5059` gives 539 in 2 of 60 runs and 538 otherwise; single-threaded,
30 of 30 give 538, and a 51-claim certificate proves 538 optimal. Rarer cases on
the default lists: N=250 seeds 203 (688 vs 687) and 222 (678 vs 677). Likely the
transposition-table finding below.

**Checker and assembler pruning.** A guess position whose letter occurs in no word
of `S` is gray on all of `S`, so a guess with `v` such live positions gives at most
`min(3^v, |S|)` responses. The checker and the assembler skip guesses that cannot
raise `k` or whose default sum (`>= 3|S| - 2 - min(3^v, |S|)`) already clears the
claim, without reading them. The checker's brute-force and corruption tests still
pass. On the compute host (uncommitted tree, stamped `3d3aa39-dirty`): checking
N=1000 took 10.978-11.114 s (3 runs, was 13.566-13.618 s) and N=2340 197.983 s
(was 257-266 s), both "valid, optimal".

Assembly with the same pruning: 214.361 s at N=2340 (was 225.8-233.1 s), 616,545
claims, no failures. The assembler's time is mostly elsewhere: its waves by set size
run sequentially between sizes, and every expansion looks up bucket facts by member
list. Those are the next things to measure.

Checking that certificate: 195.501 s, "valid, optimal". Raw lines for this section are
at the end of `optimality-certificates-raw.txt` (series 4).

## Follow-up (2026-10-18)

Issue #14 is fixed on main (f1debca): `min_state_val` keeps searches cut short by
the global beta out of the transposition table, with a deterministic unit test and
the ignored `tests/parallel_consistency.rs`. That is the same rule this branch's
always-output mode applied while logging, so always-output no longer needs its own
keep-out once the branch is rebased onto main.

The `capacity_bound` finding (`n >= k^2 - k + 3`) could not be turned into a real
input. A brute-force search over random 2-4 letter instances (15-22 candidates,
guesses mostly the candidates) never produced a reachable set in that regime: the
closest had `n - (k^2 - k + 3) = -197`, because every candidate used as a guess
separates nearly everything. Sets where no guess gives more than about `sqrt(n)`
responses split very unevenly, which makes the bound loose rather than too tight.
It stays a latent unsoundness in the formula, not a reproduced bug.

**Candidate-major scans.** The checker was latency-bound: every claim read `|S|`
scattered bytes from each of the 14,855 guess rows of a 35 MB table, about one
cache miss per guess. It now keeps a candidate-major copy of the feedback table and
reads it in chunks of 256 guesses: each candidate's responses stream contiguously
and the chunk's per-guess state stays in cache. The assembler does the same on the
solver's own candidate-major table. Three further changes, each in checker and
assembler alike:

- **One pass per expansion.** A bucket of size b defaults to
  `L3(b, k) = (2b - 1) + max(0, b - k)`, and `k` is at least any guess's own number
  of buckets m, so only buckets larger than m can contribute the second term. One
  pass therefore finds `k` and every default sum (it keeps those large buckets until
  `k` is known), where there used to be two.
- **Capacity claims read only guesses that could break them.** `b <= L3(n, k)` holds
  if `b < 2n` or if no guess gives more than `3n - 1 - b` responses. A guess
  position whose letter occurs in no word of `S` is gray on all of `S`, so a guess
  with `v` other positions gives at most `min(3^v, n)` responses; only guesses where
  that exceeds the limit are read.
- **Revisit only short guesses.** Only guesses whose default sum falls short of the
  claim are looked at again, with the claims on their buckets.

Checker tests (brute force and corruption) pass after each step. On the compute host
(uncommitted tree on `3d3aa39`, stamped `-dirty`), same tree and certificate type:

| step | check N=1000 | check N=2340 | assembly N=1000 | assembly N=2340 |
|---|---|---|---|---|
| 2026-10-17 baseline | 13.566-13.618 s | 257-266 s | 13.9-14.2 s | 226-233 s |
| live-position skip (evening) | 10.978-11.114 s | 195.501-197.983 s | - | 214.361 s |
| candidate-major scans | 9.183-9.308 s | - | 10.276 s | 92.532 s |
| + one pass per expansion | - | 103.214-103.314 s | - | - |
| + capacity filter | 7.305-7.534 s | 65.538 s | 9.502 s | 87.735 s |
| one pass, base sums streamed (final) | 6.821-7.468 s | 67.538-68.903 s | 8.844 s | 78.046 s |

All checks "valid, optimal". The one-pass step first kept per-guess vectors for all
14,855 guesses per expansion; that made full-scale assembly slower (100.965 s), so
the final version streams instead: a guess's base sum `n + sum(2b - 1)` is already a
lower bound for any `k`, so only guesses whose base sum falls short keep their large
buckets until `k` is known. The always-output path still works end to end: a full
`solve --certify` (36.353 s solve, 583 gap-fill refutations in 12.242 s) gave a
633,993-claim certificate that the final checker accepts in 63.493 s.

At N=2340 the check now costs about 1.9 plain solves (about 68 s vs 35.8 s) and the
assembly about 2.2, down from about 7 and 6.5. Raw lines: series 5 at the end of
`optimality-certificates-raw.txt`.
