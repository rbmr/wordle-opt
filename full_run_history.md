# Full-run history — N=2340 optimal policy tree

One row per **completed full tree run**, so the tree build's wall time can be
plotted against the commit that produced it. A run is listed here only if it
produced the policy tree; cost-only full runs predate the tree and are not
listed (they measured the wrong thing — see the tree-only task).

This file is the index that survives a compute rebuild. The run's own
artifacts stay on the compute host, under `~/full-runs/`, stamped with the UTC
day and the commit so successive runs never overwrite each other:

- `~/full-runs/<YYYY-MM-DD>-<commit>.json` — the policy tree
- `~/full-runs/<YYYY-MM-DD>-<commit>-progress.parquet` — its progress series
- `~/full-runs/<YYYY-MM-DD>-<commit>.log` — the run log, summary block included

Add a row when a run completes; `./check_full.sh` prints the stamp and the
summary block to copy the numbers from. A run only starts on a clean tree, so
the commit in the row is the commit that actually ran.

| Date (UTC) | Commit | Host | Wall time | Cost | Nodes | Edges | Leaves | Max depth | States | Guesses | Peak RSS | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 2026-10-09 | `5807e98` | compute (pre-migration VM) | 15413.673s (4.28h) | 8001 | 2478 | 2477 | 1942 | 5 | — | — | ~11 GB | first completed full tree run; tree published as the site's `optimal` example |

## Notes on the rows above

- The `5807e98` row is recorded retroactively from that run's own output, as
  quoted in the "Full optimal policy tree completed (2026-10-09)" comment on
  issue #1: nodes 2478, edges 2477, leaves 1942, max depth 5, total cost 8001,
  mean guesses 3.419231, validated (edge iff possible at every node). Its
  states/guesses totals were not captured in that report, hence the dashes.
- An earlier "4.89h" completion claimed for 2026-10-08 was **retracted** — it
  was never verified and the claim was withdrawn on issue #1. It is not a run
  and is not listed.
- Cost-only full runs (for example the 5.19h cost-only solve at `560bb26`)
  predate the tree-based logic and are deliberately not listed.
