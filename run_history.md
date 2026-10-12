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
- Rows are keyed by `(commit, seed, candidates)`: one commit can have several
  rows, one per size, which is what makes the size-vs-time comparison possible.
