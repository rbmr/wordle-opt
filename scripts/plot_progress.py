#!/usr/bin/env python3
"""Plot wordle-opt progress over time, from run_history.md.

Two panels, both fed only by runs recorded since the 2026-10-11 Ubuntu-server
migration (run_history.md holds nothing else by construction):

  1. Full N=2340 tree runs: wall time in hours per run, annotated with the
     commit and date, against the 2-hour milestone line. This is the headline
     trend - the deliverable is a full solve in under 2 hours.
  2. Wall time against candidate-set size, one line per commit. A change can
     make the full run faster while making small runs slower; this is where that
     trade-off shows up.

Every axis label is an explicit string; a missing field renders as "unknown",
never "undefined".

    ./scripts/plot_progress.py                     # writes progress.png
    ./scripts/plot_progress.py --out /tmp/p.png    # or wherever
"""

import argparse
import datetime
import os
import re
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MILESTONE_HOURS = 2.0
FULL_CANDIDATES = 2340


def read(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError:
        return ""


def parse_run_history(text):
    """Rows of run_history.md -> list of dicts. The column set is read from the
    header row, so adding or reordering columns cannot silently mis-parse a row.
    A row without a parseable wall time is skipped."""
    runs = []
    header = None
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if header is None:
            if cells and cells[0].lower().startswith("date"):
                header = [c.lower() for c in cells]
            continue
        if not cells or all(set(c) <= set("-: ") for c in cells):
            continue

        def col(name, default=""):
            i = header.index(name) if name in header else -1
            return cells[i] if 0 <= i < len(cells) else default

        m = re.search(r"([\d.]+)\s*s", col("wall time"))
        if not m:
            continue
        try:
            candidates = int(col("candidates"))
        except ValueError:
            candidates = None
        seed = col("seed")
        runs.append({"date": col("date"),
                     "commit": col("commit").strip("`") or "unknown",
                     "candidates": candidates,
                     "seed": seed if seed and seed != "—" else None,
                     "strategy": col("strategy") or "unknown",
                     "seconds": float(m.group(1)),
                     "cost": col("cost", "—") or "—"})
    return runs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(REPO, "progress.png"))
    ap.add_argument("--history", default=os.path.join(REPO, "run_history.md"))
    args = ap.parse_args()

    runs = parse_run_history(read(args.history))
    full = [r for r in runs if r["candidates"] == FULL_CANDIDATES]
    full.sort(key=lambda r: (r["date"], r["seconds"]))

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(15, 5.2))

    if full:
        xs = list(range(1, len(full) + 1))
        ys = [r["seconds"] / 3600.0 for r in full]
        ax1.plot(xs, ys, marker="o", color="#2b8cbe", lw=2)
        for x, y, r in zip(xs, ys, full):
            ax1.annotate(f"{y:.2f} h", (x, y), textcoords="offset points", xytext=(0, 8),
                         ha="center", fontsize=8)
        ax1.axhline(MILESTONE_HOURS, color="#d95f0e", ls="--", lw=1.5,
                    label=f"milestone ({MILESTONE_HOURS:.0f} h)")
        ax1.set_xticks(xs)
        ax1.set_xticklabels([f"{r['commit']}\n{r['date']}" for r in full], fontsize=8)
        ax1.legend(fontsize=8)
    else:
        ax1.text(0.5, 0.5, "no completed full (N=2340) runs recorded yet",
                 ha="center", va="center", transform=ax1.transAxes, color="#555")
    ax1.set_xlabel("completed full run (commit over date, UTC)")
    ax1.set_ylabel("wall time (hours)")
    ax1.set_title("Full N=2340 tree runs over time (post-migration only)")
    ax1.grid(True, alpha=0.3)

    by_key = {}
    for r in runs:
        if r["candidates"]:
            by_key.setdefault((r["commit"], r["seed"]), []).append(r)
    if by_key:
        cmap = plt.get_cmap("viridis")
        for i, ((commit, seed), rows) in enumerate(
                sorted(by_key.items(), key=lambda kv: kv[1][0]["date"])):
            rows.sort(key=lambda r: r["candidates"])
            ax2.plot([r["candidates"] for r in rows], [r["seconds"] for r in rows],
                     marker="o", ms=4, lw=1.8, color=cmap(i / max(1, len(by_key) - 1)),
                     label=f"{commit} seed={seed if seed else '-'} ({rows[0]['date']})")
        ax2.set_xscale("log")
        ax2.set_yscale("log")
        ax2.legend(fontsize=8)
    else:
        ax2.text(0.5, 0.5, "no sized runs recorded yet", ha="center", va="center",
                 transform=ax2.transAxes, color="#555")
    ax2.set_xlabel("candidate-set size (log scale)")
    ax2.set_ylabel("wall time (seconds, log scale)")
    ax2.set_title("Wall time vs candidate-set size, one line per commit")
    ax2.grid(True, alpha=0.3)

    fig.suptitle("wordle-opt progress by commit — post-migration data only — generated "
                 + datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%d %H:%M UTC"), fontsize=10)
    fig.tight_layout(rect=(0, 0, 1, 0.94))
    fig.savefig(args.out, dpi=110)
    print(f"wrote {args.out}")

    print("\nrun history (chronological):")
    if not runs:
        print("  (none recorded)")
    for r in sorted(runs, key=lambda r: (r["date"], r["candidates"] or 0)):
        print(f"  {r['date']}  {r['commit']:>10}  N={str(r['candidates']):<5} "
              f"seed={str(r['seed']):<4} {r['strategy']:<10} {r['seconds']:9.3f}s  cost={r['cost']}")


if __name__ == "__main__":
    sys.exit(main())
