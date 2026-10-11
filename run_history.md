# Run history — completed policy-tree runs

One row per **completed tree run**, from the 2026-10-11 Ubuntu-server migration
onwards. The deliverable is the tree build, so every run that counts produces a
policy tree; record it here with the commit it ran as and the candidate-set size
it covered. Smaller candidate sets are deliberately included: a change can make
the full run faster while making small runs slower, and that trade is worth
seeing rather than hiding.

**Only runs from this point on are comparable.** Runs before the migration (the
old Proxmox VM) were on different hardware, a different OS and older code, so
they are not listed and must not be used as a baseline for anything. The earlier
`benchmark_history*` files are kept only as historical records — AGENTS.md says
not to rewrite them — but their numbers are obsolete for comparison purposes.

The run's own artifacts stay on the compute host, under `~/full-runs/`, stamped
with the UTC day and the commit, so successive runs never overwrite each other:

- `~/full-runs/<YYYY-MM-DD>-<commit>.json` — the policy tree
- `~/full-runs/<YYYY-MM-DD>-<commit>-progress.parquet` — its progress series
- `~/full-runs/<YYYY-MM-DD>-<commit>.log` — the run log, summary block included

Add a row when a run completes; `./check_full.sh` prints the stamp and the
summary block to copy the numbers from. A `full` run only starts on a clean
tree, so the commit in the row is the commit that actually ran.

| Date (UTC) | Commit | Host | Candidates | Strategy | Wall time | Cost | Nodes | States | Peak RSS | Notes |
|---|---|---|---|---|---|---|---|---|---|---|
| 2026-10-11 | `a67dd02` | pc01 | 1000 | optimal | 40.942s | 3124 | 1049 | — | — | first post-migration tree run recorded; GPU-backed (avg busy cores 15.7); validation OK |

## Notes

- The `a67dd02` row is taken from that run's own output (`Build time: 40.942s`,
  `Total cost: 3124`, `Nodes: 1049`, `Validation: OK`), captured when it ran.
- The full N=2340 run launched the same day is added here when it completes.
- Rows are keyed by (commit, candidates): one commit can have several rows, one
  per candidate-set size, which is what makes the size-vs-time comparison
  possible.
