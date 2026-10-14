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
- Rows are keyed by `(commit, seed, candidates)`: one commit can have several
  rows, one per size, which is what makes the size-vs-time comparison possible.

