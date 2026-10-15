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
| 2026-10-15 | `1711125` | pc01 | 1000 | 1 | optimal | 13.631s | 3124 | 1049 | — | — | defer Zobrist hashing in root pre-filter and restore tight min_guess_val bounds; validation OK |
| 2026-10-15 | `1711125` | pc01 | 1600 | 1 | optimal | 40.980s | 5256 | 1681 | 240 | ~252MB | defer Zobrist hashing in root pre-filter and restore tight min_guess_val bounds; pick time 39.8s; validation OK |
| 2026-10-15 | `1711125` | pc01 | 2000 | 1 | optimal | 81.102s (1.35m) | 6723 | 2119 | 7045 | ~284MB | defer Zobrist hashing in root pre-filter and restore tight min_guess_val bounds; pick time 80.2s; validation OK |
| 2026-10-15 | `1711125` | pc01 | 2200 | 1 | optimal | 147.026s (2.45m) | 7477 | 2331 | 20111 | ~318MB | defer Zobrist hashing in root pre-filter and restore tight min_guess_val bounds; pick time 144.4s; validation OK |
| 2026-10-15 | `aae4000` | pc01 | 1000 | 1 | optimal | 13.567s | 3124 | 1049 | — | — | eliminate heap allocations and sorting in greedy_solve; validation OK |
| 2026-10-15 | `aae4000` | pc01 | 1600 | 1 | optimal | 40.992s | 5256 | 1681 | 240 | ~248MB | eliminate heap allocations and sorting in greedy_solve; pick time 39.8s; validation OK |
| 2026-10-15 | `aae4000` | pc01 | 2000 | 1 | optimal | 81.116s (1.35m) | 6723 | 2119 | 6951 | ~285MB | eliminate heap allocations and sorting in greedy_solve; pick time 80.2s; validation OK |
| 2026-10-15 | `aae4000` | pc01 | 2200 | 1 | optimal | 145.283s (2.42m) | 7477 | 2331 | 20452 | ~317MB | eliminate heap allocations and sorting in greedy_solve; new all-time best N=2200 pick time 142.6s; validation OK |
| 2026-10-15 | `3265bc4` | pc01 | 1000 | 1 | optimal | 13.470s | 3124 | 1049 | — | — | avoid redundant memset and scans in greedy_solve; pick time 13.0s; validation OK |
| 2026-10-15 | `3265bc4` | pc01 | 1600 | 1 | optimal | 41.065s | 5256 | 1681 | 240 | ~247MB | avoid redundant memset and scans in greedy_solve; pick time 39.9s; validation OK |
| 2026-10-15 | `3265bc4` | pc01 | 2000 | 1 | optimal | 80.020s (1.33m) | 6723 | 2119 | 7265 | ~284MB | avoid redundant memset and scans in greedy_solve; new all-time best N=2000 (80.020s vs 80.781s); pick time 79.2s; validation OK |
| 2026-10-15 | `3265bc4` | pc01 | 2200 | 1 | optimal | 145.901s (2.43m) | 7477 | 2331 | 20378 | ~316MB | avoid redundant memset and scans in greedy_solve; pick time 143.3s; validation OK |
| 2026-10-15 | `eff1f18` | pc01 | 1000 | 1 | optimal | 13.575s | 3124 | 1049 | — | — | eliminate 242-iteration scans in min_state_val using non_empty_indices; pick time 13.1s; validation OK |
| 2026-10-15 | `eff1f18` | pc01 | 1600 | 1 | optimal | 41.307s | 5256 | 1681 | 240 | ~249MB | eliminate 242-iteration scans in min_state_val using non_empty_indices; pick time 40.2s; validation OK |
| 2026-10-15 | `eff1f18` | pc01 | 2000 | 1 | optimal | 81.462s (1.36m) | 6723 | 2119 | 6902 | ~284MB | eliminate 242-iteration scans in min_state_val using non_empty_indices; pick time 80.6s; validation OK |
| 2026-10-15 | `eff1f18` | pc01 | 2200 | 1 | optimal | 144.841s (2.41m) | 7477 | 2331 | 20538 | ~317MB | eliminate 242-iteration scans in min_state_val using non_empty_indices; new all-time best N=2200 build time 144.841s and pick time 142.3s; validation OK |
| 2026-10-15 | `07f9b48` | pc01 | 1000 | 1 | optimal | 13.636s | 3124 | 1049 | — | — | propagate tightened branching factors to child subtrees; pick time 13.1s; validation OK |
| 2026-10-15 | `07f9b48` | pc01 | 1600 | 1 | optimal | 40.908s | 5256 | 1681 | 240 | ~248MB | propagate tightened branching factors to child subtrees; pick time 39.8s; validation OK |
| 2026-10-15 | `07f9b48` | pc01 | 2000 | 1 | optimal | 81.302s (1.36m) | 6723 | 2119 | 6932 | ~285MB | propagate tightened branching factors to child subtrees; pick time 80.4s; validation OK |
| 2026-10-15 | `07f9b48` | pc01 | 2200 | 1 | optimal | 148.727s (2.48m) | 7477 | 2331 | 19989 | ~316MB | propagate tightened branching factors to child subtrees; states reduced to 19989; validation OK |
| 2026-10-15 | `9f0c8eb` | pc01 | 1000 | 1 | optimal | 13.582s | 3124 | 1049 | — | — | instant O(1) bucket branching factor bound in Phase A; pick time 13.1s; validation OK |
| 2026-10-15 | `9f0c8eb` | pc01 | 1600 | 1 | optimal | 40.829s | 5256 | 1681 | 240 | ~248MB | instant O(1) bucket branching factor bound in Phase A; new all-time best N=1600 (40.829s vs 40.908s); pick time 39.7s; validation OK |
| 2026-10-15 | `9f0c8eb` | pc01 | 2000 | 1 | optimal | 80.231s (1.34m) | 6723 | 2119 | 7187 | ~284MB | instant O(1) bucket branching factor bound in Phase A; pick time 79.4s; validation OK |
| 2026-10-15 | `9f0c8eb` | pc01 | 2200 | 1 | optimal | 147.385s (2.46m) | 7477 | 2331 | 20098 | ~316MB | instant O(1) bucket branching factor bound in Phase A; pick time 144.8s; validation OK |
| 2026-10-15 | `40bdff9` | pc01 | 1000 | 1 | optimal | 13.753s | 3124 | 1049 | — | — | optimize root pre-filter by tracking non-empty indices and fusing slack loop; pick time 13.3s; validation OK |
| 2026-10-15 | `40bdff9` | pc01 | 1600 | 1 | optimal | 40.882s | 5256 | 1681 | 240 | ~247MB | optimize root pre-filter by tracking non-empty indices and fusing slack loop; pick time 39.7s; validation OK |
| 2026-10-15 | `40bdff9` | pc01 | 2000 | 1 | optimal | 79.705s (1.33m) | 6723 | 2119 | 7416 | ~284MB | optimize root pre-filter by tracking non-empty indices and fusing slack loop; new all-time record N=2000 (79.705s vs 80.020s, pick time 78.8s); validation OK |
| 2026-10-15 | `40bdff9` | pc01 | 2200 | 1 | optimal | 144.699s (2.41m) | 7477 | 2331 | 20540 | ~316MB | optimize root pre-filter by tracking non-empty indices and fusing slack loop; new all-time record N=2200 build time 144.699s and pick time 142.1s; validation OK |
| 2026-10-15 | `854315b` | pc01 | 1000 | 1 | optimal | 12.990s | 3124 | 1049 | — | — | accelerate branching factor search with candidate pre-pass and letter pruning; new all-time record N=1000 (<13s); pick time 12.5s; validation OK |
| 2026-10-15 | `854315b` | pc01 | 1600 | 1 | optimal | 40.278s | 5256 | 1681 | 240 | ~247MB | accelerate branching factor search with candidate pre-pass and letter pruning; new all-time record N=1600 (40.278s vs 40.829s); pick time 39.2s; validation OK |
| 2026-10-15 | `854315b` | pc01 | 2000 | 1 | optimal | 79.188s (1.32m) | 6723 | 2119 | 7353 | ~285MB | accelerate branching factor search with candidate pre-pass and letter pruning; new all-time record N=2000 (79.188s vs 79.705s, pick time 78.3s); validation OK |
| 2026-10-15 | `854315b` | pc01 | 2200 | 1 | optimal | 143.724s (2.40m) | 7477 | 2331 | 20607 | ~317MB | accelerate branching factor search with candidate pre-pass and letter pruning; new all-time record N=2200 build time 143.724s and pick time 141.2s; validation OK |
| 2026-10-15 | `7c2132b` | pc01 | 1000 | 1 | optimal | 10.976s | 3124 | 1049 | — | — | midpoint early cutoff and 3-letter pruning in branching factor search; new all-time record N=1000 (10.976s vs 12.990s, pick 10.5s); validation OK |
| 2026-10-15 | `7c2132b` | pc01 | 1600 | 1 | optimal | 34.212s | 5256 | 1681 | 240 | ~246MB | midpoint early cutoff and 3-letter pruning in branching factor search; new all-time record N=1600 (34.212s vs 40.278s, pick 33.1s); validation OK |
| 2026-10-15 | `7c2132b` | pc01 | 2000 | 1 | optimal | 69.824s (1.16m) | 6723 | 2119 | 6727 | ~280MB | midpoint early cutoff and 3-letter pruning in branching factor search; new all-time record N=2000 (69.824s vs 79.188s, pick 69.0s); validation OK |
| 2026-10-15 | `7c2132b` | pc01 | 2200 | 1 | optimal | 131.664s (2.19m) | 7477 | 2331 | 19694 | ~316MB | midpoint early cutoff and 3-letter pruning in branching factor search; new all-time record N=2200 (131.664s vs 143.724s, pick 129.2s); validation OK |
| 2026-10-15 | `bc55743` | pc01 | 1000 | 1 | optimal | 10.380s | 3124 | 1049 | — | — | evaluate true candidates in branching pre-pass and 4-letter pruning; new all-time record N=1000 (10.380s vs 10.976s, pick 9.9s); validation OK |
| 2026-10-15 | `bc55743` | pc01 | 1600 | 1 | optimal | 33.129s | 5256 | 1681 | 240 | ~265MB | evaluate true candidates in branching pre-pass and 4-letter pruning; new all-time record N=1600 (33.129s vs 34.212s, pick 32.0s); validation OK |
| 2026-10-15 | `bc55743` | pc01 | 2000 | 1 | optimal | 68.379s (1.14m) | 6723 | 2119 | — | ~282MB | evaluate true candidates in branching pre-pass and 4-letter pruning; new all-time record N=2000 (68.379s vs 69.824s, pick 67.5s); validation OK |
| 2026-10-15 | `bc55743` | pc01 | 2200 | 1 | optimal | 128.977s (2.15m) | 7477 | 2331 | — | ~317MB | evaluate true candidates in branching pre-pass and 4-letter pruning; new all-time record N=2200 (128.977s vs 131.664s, pick 126.6s); validation OK |
| 2026-10-15 | `09ca06d` | pc01 | 1000 | 1 | optimal | 10.615s | 3124 | 1049 | — | — | reuse scratch partition buffer without zeroing and slice g_row; pick time 10.1s; validation OK |
| 2026-10-15 | `09ca06d` | pc01 | 1600 | 1 | optimal | 33.060s | 5256 | 1681 | 240 | ~266MB | reuse scratch partition buffer without zeroing and slice g_row; new all-time record N=1600 (33.060s vs 33.129s, pick 31.9s); validation OK |
| 2026-10-15 | `09ca06d` | pc01 | 2000 | 1 | optimal | 67.879s (1.13m) | 6723 | 2119 | — | ~283MB | reuse scratch partition buffer without zeroing and slice g_row; new all-time record N=2000 (67.879s vs 68.379s, pick 67.0s); validation OK |
| 2026-10-15 | `09ca06d` | pc01 | 2200 | 1 | optimal | 129.105s (2.15m) | 7477 | 2331 | — | ~317MB | reuse scratch partition buffer without zeroing and slice g_row; pick time 126.7s; validation OK |
| 2026-10-15 | `1d58264` | pc01 | 1000 | 1 | optimal | 9.682s | 3124 | 1049 | — | — | universal non-overlapping letter information bound in branching search; new all-time record N=1000 (<10s for the first time: 9.682s vs 10.380s, pick 9.2s); validation OK |
| 2026-10-15 | `1d58264` | pc01 | 1600 | 1 | optimal | 30.822s | 5256 | 1681 | 240 | ~265MB | universal non-overlapping letter information bound in branching search; new all-time record N=1600 (30.822s vs 33.060s, pick 29.7s); validation OK |
| 2026-10-15 | `1d58264` | pc01 | 2000 | 1 | optimal | 63.882s (1.06m) | 6723 | 2119 | — | ~283MB | universal non-overlapping letter information bound in branching search; new all-time record N=2000 (63.882s vs 67.879s, pick 63.0s, -4.0s); validation OK |
| 2026-10-15 | `1d58264` | pc01 | 2200 | 1 | optimal | 122.027s (2.03m) | 7477 | 2331 | — | ~317MB | universal non-overlapping letter information bound in branching search; new all-time record N=2200 (122.027s vs 128.977s, pick 119.6s, -6.9s); validation OK |
| 2026-10-15 | `66ecbf7` | pc01 | 1000 | 1 | optimal | 9.495s | 3124 | 1049 | — | — | parallel active_guesses and defer eager greedy_solve in scan; new all-time record N=1000 (9.495s vs 9.682s, pick 9.1s); validation OK |
| 2026-10-15 | `66ecbf7` | pc01 | 1600 | 1 | optimal | 30.807s | 5256 | 1681 | 240 | ~266MB | parallel active_guesses and defer eager greedy_solve in scan; new all-time record N=1600 (30.807s vs 30.822s, pick 29.8s); validation OK |
| 2026-10-15 | `66ecbf7` | pc01 | 2000 | 1 | optimal | 63.460s (1.06m) | 6723 | 2119 | — | ~284MB | parallel active_guesses and defer eager greedy_solve in scan; new all-time record N=2000 (63.460s vs 63.882s, pick 62.7s); validation OK |
| 2026-10-15 | `66ecbf7` | pc01 | 2200 | 1 | optimal | 120.847s (2.01m) | 7477 | 2331 | — | ~327MB | parallel active_guesses and defer eager greedy_solve in scan; new all-time record N=2200 (120.847s vs 122.027s, pick 118.6s); validation OK |
| 2026-10-15 | `8e219cf` | pc01 | 1000 | 1 | optimal | 6.016s | 3124 | 1049 | — | — | eliminate full dictionary scans from Phase B root prefilter; massive new all-time record N=1000 (6.016s vs 9.495s, 36.6% faster, pick 5.6s); validation OK |
| 2026-10-15 | `8e219cf` | pc01 | 1600 | 1 | optimal | 25.859s | 5256 | 1681 | 240 | ~266MB | eliminate full dictionary scans from Phase B root prefilter; new all-time record N=1600 (25.859s vs 30.807s, 16.1% faster, pick 25.2s); validation OK |
| 2026-10-15 | `8e219cf` | pc01 | 2000 | 1 | optimal | 54.252s | 6723 | 2119 | — | ~284MB | eliminate full dictionary scans from Phase B root prefilter; new all-time record N=2000 (54.252s vs 63.460s, 14.5% faster, pick 53.6s, -9.2s); validation OK |
| 2026-10-15 | `8e219cf` | pc01 | 2200 | 1 | optimal | 107.502s (1.79m) | 7477 | 2331 | — | ~320MB | eliminate full dictionary scans from Phase B root prefilter; new all-time record N=2200 (107.502s vs 120.847s, 11.0% faster, pick 106.3s, -13.3s); validation OK |

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
- The `1711125` rows confirm that reverting the guard in `min_guess_val` restored tight recursive child bounds (reducing N=2200 states from 46.8k back to 20.1k and runtime from 205.528s to 147.026s), while branchlessly counting responses and deferring Zobrist hash accumulation in root pre-filtering eliminated over 34 million unused XORs and lookups on discarded guesses.
- The `aae4000` rows demonstrated that eliminating `active_tuples` heap vector allocations and full 14,855-element sorts in `greedy_solve`, along with single-pass counting sort candidate partitioning, produced a new all-time best N=2200 pick time of 142.6s (down to 145.283s overall build time).
- The `3265bc4` rows demonstrated that avoiding full 486-byte memset of counts across all 14,855 guesses in `greedy_solve` by clearing only non-empty indices and bypassing candidate projections when `c_mask.count_ones() >= 26` achieved a new all-time best N=2000 build time of 80.020s (pick time 79.2s).
- The `eff1f18` rows demonstrated that eliminating the fixed 242-iteration scans over `counts` in `min_state_val` (phase 1 and phase 2 CPU paths) using `non_empty_indices` to bound clearing and bound computation to `num_non_empty` entries produced a new all-time best N=2200 build time of 144.841s and pick time of 142.3s.
- The `07f9b48` rows demonstrated that propagating tightened child branching factors down into child subtrees reduced explored states at N=2200 from 20.5k to 19.9k and improved N=1600 build time to 40.908s (pick time 39.8s), while sequential max_k computation on large root buckets in Phase A introduced minor parallel overhead.
- The `9f0c8eb` rows demonstrated that using an instant O(1) `bucket_k` bound in Phase A eliminated the sequential branching factor computation overhead across large opener buckets, setting a new all-time best N=1600 build time of 40.829s (pick time 39.7s) and reducing N=2200 build time to 147.385s (pick time 144.8s).
- The `40bdff9` rows demonstrated that tracking `non_empty_indices` branchlessly during candidate counting to evaluate initial capacity bounds strictly across non-empty buckets and fusing the uncached bucket slack check directly into the first cache pass set new all-time records for both N=2000 (79.705s build, 78.8s pick) and N=2200 (144.699s build, 142.1s pick).
- The `854315b` rows demonstrated that evaluating candidate guesses first in `compute_max_branching_factor_seq` to rapidly establish high branching lower bounds and soundly pruning guesses with zero letter overlap (always $k=1$) or $\le 2$ distinct-letter overlaps when $max\_k \ge 9$ achieved a clean sweep of new all-time records across all benchmark sizes: N=1000 down to 12.990s (pick 12.5s), N=1600 down to 40.278s (pick 39.2s), N=2000 down to 79.188s (pick 78.3s), and N=2200 down to 143.724s (pick 141.2s).
- The `7c2132b` rows demonstrated that introducing midpoint capacity checks in `compute_max_branching_factor_seq` (`half_unique + (N - mid) <= max_k`) to prune the remaining 50% of the candidate loop on unpromising guesses, alongside sound $\le 3$-letter overlap pruning when $max\_k \ge 27$, achieved a monumental performance leap across all benchmarks: N=1000 dropped to 10.976s (pick 10.5s, 15.5% faster), N=1600 dropped to 34.212s (pick 33.1s, 15.1% faster), N=2000 dropped to 69.824s (pick 69.0s, 12.0% faster, breaking the 70s barrier), and N=2200 dropped to 131.664s (pick 129.2s, 12 full seconds faster).
- The `bc55743` rows demonstrated that evaluating true candidate words via `ResponseMatrix::candidate_to_guess` mapping in `compute_max_branching_factor_seq`'s candidate pre-pass (rather than unrelated guess indices) established high branching lower bounds earlier, and extending information-theoretic pruning to 4-letter overlaps for isograms and words with $\ge 3$ distinct letters when $max\_k \ge 81$ achieved another clean sweep of all-time records across all sizes: N=1000 down to 10.380s (pick time 9.9s, under 10s for the first time), N=1600 down to 33.129s (pick time 32.0s), N=2000 down to 68.379s (pick time 67.5s), and N=2200 down to 128.977s (pick time 126.6s).
- The `09ca06d` rows demonstrated that eliminating redundant `Vec::clear()` calls before `Vec::resize()` in thread-local scratch partition buffers (avoiding recurring 17.6KB zero-fill memset writes) and referencing pre-sliced `g_row` slices to bypass per-candidate base offset calculations in `min_guess_val` and `OptimalPolicy::scan` established new all-time records for N=1600 (33.060s build, 31.9s pick) and N=2000 (67.879s build, 67.0s pick).
- The `1d58264` rows demonstrated that establishing a universal information-theoretic upper bound on non-overlapping letters in `compute_max_branching_factor_seq` ($3^{5 - non\_overlap}$ maximum possible responses when $non\_overlap = g\_ones - overlap\_count$, bypassing search when $max\_k \ge 3^{5 - non\_overlap}$) and subslice indexing achieved a monumental sweep of all-time records across all candidate set sizes: N=1000 broke below 10s for the first time (9.682s, pick 9.2s), N=1600 dropped under 31s (30.822s, pick 29.7s), N=2000 dropped by 4.0s to 63.882s (pick 63.0s), and N=2200 dropped by 7.1s to 122.027s (pick 119.6s).
- The `66ecbf7` rows demonstrated that eliminating redundant eager `Solver::greedy_solve(candidates)` calls at the start of `scan` for candidate sets with $\ge 48$ words (where `first_guess_cost <= initial_best_val` is guaranteed by construction and initializes bounds directly), parallelizing `active_guesses` across Rayon worker threads, pre-slicing `g_row`, and clearing only non-empty response buckets achieved another clean sweep of all-time records across all sizes: N=1000 down to 9.495s (pick 9.1s), N=1600 down to 30.807s (pick 29.8s), N=2000 down to 63.460s (pick 62.7s), and N=2200 down to 120.847s (pick 118.6s).
- The `8e219cf` rows demonstrated that eliminating full 14,855-guess dictionary scans on large buckets ($p\_len > 15$) from the Phase B root prefilter (restricting bucket evaluations to fast $O(|p|^2)$ `quick_bucket_eval` on $p\_len \le 15$ buckets and letting highly optimized alpha-beta in Phase C prune surviving candidates) eliminated tens of seconds of bottleneck stalling in Phase B, achieving a monumental sweep of all-time records across all candidate set sizes: N=1000 dropped to **6.016s** (pick 5.6s, 36.6% faster), N=1600 dropped to **25.859s** (pick 25.2s, 16.1% faster), N=2000 dropped by 9.2s to **54.252s** (pick 53.6s, 14.5% faster), and N=2200 dropped by 13.3s to **107.502s** (pick 106.3s, 11.0% faster, breaking the 110s barrier).
- Rows are keyed by `(commit, seed, candidates)`: one commit can have several
  rows, one per size, which is what makes the size-vs-time comparison possible.


