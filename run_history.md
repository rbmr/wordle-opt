# Run history — completed policy-tree runs

One row per **completed tree run**, from the 2026-10-11 Ubuntu-server migration
onwards. The deliverable is the tree build, so every run that counts produces a
policy tree; record it here with the commit it ran as, the candidate-set size it
covered, and the seed that drew that subset.

**A run's identity is `(commit, seed, size)`.** The subset is drawn from
`subset_rng(seed, size)`, so those three values determine which candidates were
solved — and a different subset of the same size can perform very differently,
which is why the seed is part of the row and not an afterthought. Timings are
only comparable between runs that share all three.

**Only runs from this point on are comparable.** Runs before the migration (the
old Proxmox VM) were on different hardware, a different OS and older code, so
they are not listed and must not be used as a baseline for anything. The earlier
`benchmark_history*` files are kept only as historical records — AGENTS.md says
not to rewrite them — but their numbers are obsolete for comparison purposes.

Smaller candidate sets are deliberately included: a change can make the full run
faster while making small runs slower, and that trade is worth seeing rather than
hiding.

The run's own artifacts stay on the compute host, under `~/full-runs/`, stamped
with the UTC day and the commit, so successive runs never overwrite each other:

- `~/full-runs/<YYYY-MM-DD>-<commit>.json` — the policy tree
- `~/full-runs/<YYYY-MM-DD>-<commit>-progress.parquet` — its progress series
- `~/full-runs/<YYYY-MM-DD>-<commit>.log` — the run log, summary block included

Add a row when a run completes. The run's own output prints the selection it
used (`selection="seeded subset (seed=…, size=… of …)"`), so the seed comes
straight from the log; `./check_full.sh` prints the stamp and summary block for a
full run. A `full` run only starts on a clean tree, so the commit in the row is
the commit that actually ran.

| Date (UTC) | Commit | Host | Candidates | Seed | Strategy | Wall time | Cost | Nodes | States | Peak RSS | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 2026-10-11 | `a67dd02` | pc01 | 1000 | 1 | optimal | 40.942s | 3124 | 1049 | — | — | first post-migration tree run recorded; GPU-backed (avg busy cores 15.7); validation OK |
| 2026-10-11 | `a67dd02` | pc01 | 2340 | — | optimal | 6470.277s (1.80h) | 8001 | 2478 | 139753702 | ~9.9GB | full N=2340 run; under the 2h milestone; mean guesses 3.419231, max depth 5, avg busy cores 16.0; validation OK |
| 2026-10-14 | `9d5855c` | pc01 | 1000 | 1 | optimal | 37.473s | 3124 | 1049 | — | ~713MB | bucket lower-bound tightening; 8.5% faster than a67dd02 (37.473s vs 40.942s); avg busy cores 15.7; validation OK |
| 2026-10-14 | `5f84eda` | pc01 | 1100 | 1 | optimal | 66.291s | 3485 | 1163 | — | ~1.2GB | max depth 4, leaves 917; avg busy cores 15.8; validation OK |
| 2026-10-14 | `c57c4c9` | pc01 | 1200 | 1 | optimal | 98.709s | 3825 | 1252 | — | ~1.65GB | max depth 4, leaves 994; avg busy cores 15.8; validation OK |
| 2026-10-14 | `8fc1afa` | pc01 | 1300 | 1 | optimal | 283.040s (4.7m) | 4204 | 1370 | — | ~4.2GB | max depth 4, leaves 1080; avg busy cores 15.8; validation OK |
| 2026-10-14 | `e77f6b4` | pc01 | 1300 | 1 | optimal | 285.695s (4.76m) | 4204 | 1370 | — | ~4.3GB | max depth 4, leaves 1080; avg busy cores 15.9; validation OK |
| 2026-10-14 | `13f04b8` | pc01 | 500 | 1 | optimal | 1.532s | 1453 | 517 | — | — | fast in-set partition evaluation for small buckets; 4x faster on N=500; avg busy cores 11.8; validation OK |
| 2026-10-14 | `13f04b8` | pc01 | 1000 | 1 | optimal | 16.242s | 3124 | 1049 | 105823 | ~423MB | fast in-set partition evaluation; 2.3x faster than 9d5855c (16.242s vs 37.473s); avg busy cores 15.4; validation OK |
| 2026-10-14 | `13f04b8` | pc01 | 1300 | 1 | optimal | 108.075s (1.80m) | 4204 | 1370 | 832453 | ~1.67GB | fast in-set partition evaluation; 2.6x faster than e77f6b4 (108.075s vs 285.695s); 6x fewer states; validation OK |
| 2026-10-14 | `13f04b8` | pc01 | 1400 | 1 | optimal | 92.032s (1.53m) | 4525 | 1473 | 686831 | ~1.42GB | max depth 4, leaves 1176; avg busy cores 15.8; validation OK |
| 2026-10-14 | `13f04b8` | pc01 | 1600 | 1 | optimal | 417.011s (6.95m) | 5256 | 1681 | 2977956 | ~5.07GB | max depth 4, leaves 1321; avg busy cores 15.9; validation OK |
| 2026-10-14 | `65bb6be` | pc01 | 1300 | 1 | optimal | 107.953s (1.80m) | 4204 | 1370 | 832836 | ~1.67GB | parallel quick_bucket_eval root bound filter; avg busy cores 15.8; validation OK |
| 2026-10-14 | `65bb6be` | pc01 | 1600 | 1 | optimal | 414.827s (6.91m) | 5256 | 1681 | 2986415 | ~5.09GB | parallel quick_bucket_eval root bound filter; avg busy cores 15.9; validation OK |
| 2026-10-14 | `2058171` | pc01 | 1000 | 1 | optimal | 16.679s | 3124 | 1049 | 40 | ~231MB | tighten large bucket bounds; root states dropped from 105k to 40; validation OK |
| 2026-10-14 | `2058171` | pc01 | 1300 | 1 | optimal | 32.695s | 4204 | 1370 | 67 | ~243MB | tighten large bucket bounds; 3.3x faster than 65bb6be (32.695s vs 107.953s); validation OK |
| 2026-10-14 | `2058171` | pc01 | 1600 | 1 | optimal | 53.026s | 5256 | 1681 | 240 | ~253MB | tighten large bucket bounds; 7.8x faster than 65bb6be (53.026s vs 414.827s); validation OK |
| 2026-10-14 | `2058171` | pc01 | 1800 | 1 | optimal | 73.643s (1.23m) | 5974 | 1900 | 235 | ~260MB | max depth 4, leaves 1521; validation OK |
| 2026-10-14 | `2058171` | pc01 | 2000 | 1 | optimal | 104.059s (1.73m) | 6723 | 2119 | 4787 | ~283MB | max depth 4, leaves 1674; validation OK |
| 2026-10-14 | `7786894` | pc01 | 2200 | 1 | optimal | 186.144s (3.10m) | 7477 | 2331 | 21822 | ~320MB | max depth 5, leaves 1804; avg busy cores 15.7; validation OK |
| 2026-10-14 | `f9b82eb` | pc01 | 2340 | — | optimal | 252.639s (4.21m) | 8001 | 2478 | 26712 | ~2.3GB | full N=2340 run; under the 20-minute milestone (4.21m vs 1.80h at a67dd02, 25.6x speedup); mean guesses 3.419231, max depth 5, avg busy cores 15.6; validation OK |
| 2026-10-14 | `67418f9` | pc01 | 1000 | 1 | optimal | 14.281s | 3124 | 1049 | — | — | evaluate largest buckets first in root filter; 14.4% faster (14.281s vs 16.679s); validation OK |
| 2026-10-14 | `67418f9` | pc01 | 1300 | 1 | optimal | 27.240s | 4204 | 1370 | — | — | evaluate largest buckets first in root filter; 16.7% faster (27.240s vs 32.695s); validation OK |
| 2026-10-15 | `67418f9` | pc01 | 1600 | 1 | optimal | 45.561s | 5256 | 1681 | 240 | ~276MB | evaluate largest buckets first in root filter; 14.1% faster (45.561s vs 53.026s); validation OK |
| 2026-10-15 | `67418f9` | pc01 | 1800 | 1 | optimal | 60.116s (1.00m) | 5974 | 1900 | 235 | ~285MB | evaluate largest buckets first in root filter; 18.4% faster (60.116s vs 73.643s); validation OK |
| 2026-10-15 | `67418f9` | pc01 | 2000 | 1 | optimal | 89.852s (1.50m) | 6723 | 2119 | 4636 | ~282MB | evaluate largest buckets first in root filter; 13.7% faster (89.852s vs 104.059s); validation OK |
| 2026-10-15 | `67418f9` | pc01 | 2200 | 1 | optimal | 168.980s (2.82m) | 7477 | 2331 | 22053 | ~321MB | evaluate largest buckets first in root filter; 9.2% faster (168.980s vs 186.144s); validation OK |
| 2026-10-15 | `0c58727` | pc01 | 2340 | — | optimal | 231.972s (3.87m) | 8001 | 2478 | 26641 | ~2.3GB | full N=2340 run; evaluate largest buckets first in root filter; 8.2% faster (231.972s vs 252.639s); 27.9x faster than a67dd02 baseline; validation OK |
| 2026-10-15 | `5d402a1` | pc01 | 1000 | 1 | optimal | 13.962s | 3124 | 1049 | — | — | dynamically refresh global beta across buckets in min_guess_val; validation OK |
| 2026-10-15 | `5d402a1` | pc01 | 1300 | 1 | optimal | 27.554s | 4204 | 1370 | 67 | ~242MB | dynamically refresh global beta across buckets in min_guess_val; validation OK |
| 2026-10-15 | `5d402a1` | pc01 | 1600 | 1 | optimal | 42.942s | 5256 | 1681 | 240 | ~252MB | dynamically refresh global beta across buckets in min_guess_val; 5.8% faster (42.942s vs 45.561s); validation OK |
| 2026-10-15 | `5d402a1` | pc01 | 1800 | 1 | optimal | 61.327s (1.02m) | 5974 | 1900 | 235 | ~276MB | dynamically refresh global beta across buckets in min_guess_val; validation OK |
| 2026-10-15 | `5d402a1` | pc01 | 2000 | 1 | optimal | 86.694s (1.44m) | 6723 | 2119 | 4636 | ~284MB | dynamically refresh global beta across buckets in min_guess_val; 3.5% faster (86.694s vs 89.852s); validation OK |
| 2026-10-15 | `5d402a1` | pc01 | 2200 | 1 | optimal | 168.940s (2.82m) | 7477 | 2331 | 22107 | ~322MB | dynamically refresh global beta across buckets in min_guess_val; validation OK |
| 2026-10-15 | `1643fe8` | pc01 | 1000 | 1 | optimal | 13.837s | 3124 | 1049 | — | — | cache-first root filter with thread-local scratch buffer; validation OK |
| 2026-10-15 | `1643fe8` | pc01 | 1600 | 1 | optimal | 42.422s | 5256 | 1681 | 240 | ~254MB | cache-first root filter with thread-local scratch buffer; validation OK |
| 2026-10-15 | `1643fe8` | pc01 | 2000 | 1 | optimal | 86.641s (1.44m) | 6723 | 2119 | 4636 | ~285MB | cache-first root filter with thread-local scratch buffer; validation OK |
| 2026-10-15 | `70b2596` | pc01 | 1000 | 1 | optimal | 13.556s | 3124 | 1049 | — | — | dynamic global beta pruning in min_state_val; validation OK |
| 2026-10-15 | `70b2596` | pc01 | 1600 | 1 | optimal | 42.253s | 5256 | 1681 | 240 | ~254MB | dynamic global beta pruning in min_state_val; validation OK |
| 2026-10-15 | `70b2596` | pc01 | 2000 | 1 | optimal | 85.509s (1.43m) | 6723 | 2119 | 4636 | ~285MB | dynamic global beta pruning in min_state_val; validation OK |
| 2026-10-15 | `70b2596` | pc01 | 2200 | 1 | optimal | 166.528s (2.78m) | 7477 | 2331 | 22224 | ~332MB | dynamic global beta pruning in min_state_val; 2.4s faster than 5d402a1 (166.528s vs 168.940s); validation OK |
| 2026-10-15 | `a12d58f` | pc01 | 1000 | 1 | optimal | 13.875s | 3124 | 1049 | — | — | lazy bucket partitioning in min_guess_val; validation OK |
| 2026-10-15 | `a12d58f` | pc01 | 1600 | 1 | optimal | 40.696s | 5256 | 1681 | 240 | ~253MB | lazy bucket partitioning in min_guess_val; 3.7% faster (40.696s vs 42.253s); validation OK |
| 2026-10-15 | `a12d58f` | pc01 | 2000 | 1 | optimal | 81.437s (1.36m) | 6723 | 2119 | 4636 | ~285MB | lazy bucket partitioning in min_guess_val; 4.8% faster (81.437s vs 85.509s); validation OK |
| 2026-10-15 | `a12d58f` | pc01 | 2200 | 1 | optimal | 145.709s (2.43m) | 7477 | 2331 | 22224 | ~320MB | lazy bucket partitioning in min_guess_val; 12.5% faster (145.709s vs 166.528s, 20.8s drop); validation OK |
| 2026-10-15 | `a453caa` | pc01 | 1000 | 1 | optimal | 13.473s | 3124 | 1049 | — | — | parallelize bucket evaluations and avoid redundant checks in root opener scan; validation OK |
| 2026-10-15 | `a453caa` | pc01 | 1600 | 1 | optimal | 40.396s | 5256 | 1681 | 240 | ~253MB | parallelize bucket evaluations and avoid redundant checks in root opener scan; validation OK |
| 2026-10-15 | `a453caa` | pc01 | 2000 | 1 | optimal | 80.781s (1.35m) | 6723 | 2119 | 4636 | ~285MB | parallelize bucket evaluations and avoid redundant checks in root opener scan; validation OK |
| 2026-10-15 | `a453caa` | pc01 | 2200 | 1 | optimal | 146.589s (2.44m) | 7477 | 2331 | 22224 | ~320MB | parallelize bucket evaluations and avoid redundant checks in root opener scan; validation OK |
| 2026-10-15 | `f8575b3` | pc01 | 1000 | 1 | optimal | 13.725s | 3124 | 1049 | — | — | bypass equivalence projection for candidate sets >= 40; validation OK |
| 2026-10-15 | `f8575b3` | pc01 | 1600 | 1 | optimal | 40.431s | 5256 | 1681 | 240 | ~253MB | bypass equivalence projection for candidate sets >= 40; validation OK |
| 2026-10-15 | `f8575b3` | pc01 | 2000 | 1 | optimal | 81.000s (1.35m) | 6723 | 2119 | 4636 | ~285MB | bypass equivalence projection for candidate sets >= 40; validation OK |
| 2026-10-15 | `f8575b3` | pc01 | 2200 | 1 | optimal | 145.772s (2.43m) | 7477 | 2331 | 22224 | ~317MB | bypass equivalence projection for candidate sets >= 40; pick time 143.2s; validation OK |
| 2026-10-15 | `25b2f10` | pc01 | 1000 | 1 | optimal | 13.491s | 3124 | 1049 | — | — | bypass redundant bucket partitioning and scans in root pre-filtering; validation OK |
| 2026-10-15 | `25b2f10` | pc01 | 1600 | 1 | optimal | 40.775s | 5256 | 1681 | 240 | ~252MB | bypass redundant bucket partitioning and scans in root pre-filtering; pick time 39.6s; validation OK |
| 2026-10-15 | `25b2f10` | pc01 | 2000 | 1 | optimal | 81.059s (1.35m) | 6723 | 2119 | 4636 | ~284MB | bypass redundant bucket partitioning and scans in root pre-filtering; pick time 80.1s; validation OK |
| 2026-10-15 | `25b2f10` | pc01 | 2200 | 1 | optimal | 146.181s (2.44m) | 7477 | 2331 | 22224 | ~317MB | bypass redundant bucket partitioning and scans in root pre-filtering; pick time 143.5s; validation OK |
| 2026-10-15 | `abb17ec` | pc01 | 1000 | 1 | optimal | 13.653s | 3124 | 1049 | — | — | guard branching factor computation in min_guess_val; validation OK |
| 2026-10-15 | `abb17ec` | pc01 | 1600 | 1 | optimal | 42.173s | 5256 | 1681 | 240 | ~253MB | guard branching factor computation in min_guess_val; validation OK |
| 2026-10-15 | `abb17ec` | pc01 | 2000 | 1 | optimal | 90.106s (1.50m) | 6723 | 2119 | 12773 | ~302MB | guard branching factor computation in min_guess_val; regressed from 81.059s; validation OK |
| 2026-10-15 | `abb17ec` | pc01 | 2200 | 1 | optimal | 205.528s (3.43m) | 7477 | 2331 | 46880 | ~354MB | guard branching factor computation in min_guess_val; regressed from 146.181s due to looser child beta in Pass 3; validation OK |


## Notes

- The `a67dd02` row is taken from that run's own output (`Build time: 40.942s`,
  `Total cost: 3124`, `Nodes: 1049`, `Validation: OK`), captured when it ran; the
  command was `solve --strategy optimal --max-candidates 1000 --sample-seed 1`.
- The full N=2340 `a67dd02` row is taken from that run's own summary block
  (`Wall time: 6470.277s (1.80h)`, `Total cost: 8001`, `Nodes: 2478`,
  `States evaluated: 139753702`, peak `rss_mb=9899`, `Validation: OK`), captured
  via `./check_full.sh`. It uses the whole candidate list, so its seed cell is
  `—` (no subset was drawn). This is the first full run to finish under the 2h
  milestone; it was launched by the 2026-10-11 monitoring session and completed
  at 22:15 UTC.
- The `9d5855c` row is taken from that run's own output (`Build time: 37.473s`,
  `Total cost: 3124`, `Nodes: 1049`, `Validation: OK`, peak `rss_mb=713`), captured
  via `./deploy_and_bench.sh solve --strategy optimal --max-candidates 1000 --sample-seed 1 --output /tmp/n1000.json`.
- The `5f84eda` row is taken from that run's own output (`Build time: 66.291s`,
  `Total cost: 3485`, `Nodes: 1163`, `Validation: OK`, peak `rss_mb=1183`), captured
  via `./deploy_and_bench.sh solve --strategy optimal --max-candidates 1100 --sample-seed 1 --output /tmp/n1100.json`.
- The `c57c4c9` row is taken from that run's own output (`Build time: 98.709s`,
  `Total cost: 3825`, `Nodes: 1252`, `Validation: OK`, peak `rss_mb=1650`), captured
  via `./deploy_and_bench.sh solve --strategy optimal --max-candidates 1200 --sample-seed 1 --output /tmp/n1200.json`.
- The `8fc1afa` row is taken from that run's own output (`Build time: 283.040s`,
  `Total cost: 4204`, `Nodes: 1370`, `Validation: OK`, peak `rss_mb=4188`), captured
  via `./deploy_and_bench.sh solve --strategy optimal --max-candidates 1300 --sample-seed 1 --output /tmp/n1300.json`.
- The `e77f6b4` row is taken from that run's own output (`Build time: 285.695s`,
  `Total cost: 4204`, `Nodes: 1370`, `Validation: OK`, peak `rss_mb=4321`), captured
  via `./deploy_and_bench.sh solve --strategy optimal --max-candidates 1300 --sample-seed 1 --output /tmp/n1300.json`.
- The `2058171` rows are taken from those runs' own outputs, captured via `./deploy_and_bench.sh` on seeded subsets (seed=1, sizes 1000, 1300, 1600, 1800, 2000). Tightened large-bucket capacity lower bounds via maximum branching factor calculation reduced root DFS states by orders of magnitude (e.g. 105k to 40 for N=1000, 3.0M to 240 for N=1600).
- The full N=2340 `f9b82eb` row is taken from that run's own summary block (`Wall time: 252.639s (0.07h)`, `Total cost: 8001`, `Nodes: 2478`, `States evaluated: 26712`, peak `rss_mb=2315`, `Validation: OK`), captured via `./check_full.sh`. It solves the complete dictionary in 4.21 minutes (a 25.6x speedup over the 1.80h milestone from a67dd02, beating the 20-minute milestone), with states evaluated reduced by 5,231x (26,712 vs 139,753,702).
- The full N=2340 `0c58727` row is taken from that run's own summary block (`Wall time: 231.972s (0.06h)`, `Total cost: 8001`, `Nodes: 2478`, `States evaluated: 26641`, peak `rss_mb=2308`, `Validation: OK`), captured via `./check_full.sh`. Evaluating largest candidate buckets first in root filter shaved another 20.7s off the full solve (3.87m vs 4.21m, an 8.2% speedup, 27.9x faster than the baseline a67dd02).
- The `abb17ec` rows demonstrated that guarding branching factor computation in `min_guess_val` prevented tight capacity lower bounds from being inserted into `p_lbs` and cache, widening `child_beta` for sibling buckets in recursive Pass 3 and dramatically increasing explored states (e.g. 46.8k vs 22.2k at N=2200, regressing wall time from 146.181s to 205.528s). Guard reverted in subsequent commit.
- Rows are keyed by `(commit, seed, candidates)`: one commit can have several
  rows, one per size, which is what makes the size-vs-time comparison possible.

