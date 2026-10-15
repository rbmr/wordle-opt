#!/usr/bin/env python3
"""Plot every recorded full N=2340 run's progress against elapsed time.

One line per full run, x is elapsed seconds (log scale, because the runs span
1.8 hours down to under 4 minutes) and y is root guesses evaluated. The point is
to see *how* the search spends its time, not just how long it took: the baseline
chews through thousands of root guesses, while the later runs settle the same
tree after a few hundred, because they prune far harder.

Data comes from each run's own log, which is the authoritative record and is
kept on the compute host:

    scp user@pc01:'~/full-runs/*.log' ./full-run-logs/

    ./scripts/plot_full_run_progress.py --logs ./full-run-logs --out progress.png

Every axis label is an explicit string, and a run with too few samples to draw a
curve is still plotted as points rather than silently dropped.
"""

import argparse
import os
import re
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

# Human-readable term for each run, so the legend says what changed rather than
# just a hash. Keyed by the commit the run recorded, which is the commit that
# actually produced it. Anything not listed falls back to "unlabelled".
TERMS = {
    "a67dd02": "baseline (pre-optimisation)",
    "f9b82eb": "large-bucket bounds",
    "0c58727": "largest buckets first",
}

PROGRESS_RE = re.compile(
    r"^\[progress\]\s+elapsed=(\d+)s.*?root_guesses=(\d+)")
COMMIT_RE = re.compile(r"commit=([0-9a-f]+)")
WALL_RE = re.compile(r"Wall time:\s*([0-9.]+)s")


def human(seconds):
    if seconds >= 3600:
        return "%.2f h" % (seconds / 3600.0)
    if seconds >= 60:
        return "%.2f min" % (seconds / 60.0)
    return "%.1f s" % seconds


def parse(path):
    """Pull the commit, the progress series and the wall time out of one log."""
    commit = "unknown"
    points = []
    wall = None
    with open(path, "r", errors="replace") as fh:
        for line in fh:
            if commit == "unknown":
                m = COMMIT_RE.search(line)
                if m:
                    commit = m.group(1)
            m = PROGRESS_RE.match(line)
            if m:
                points.append((int(m.group(1)), int(m.group(2))))
            m = WALL_RE.search(line)
            if m:
                wall = float(m.group(1))
    # A run that finished has its wall time; one that was capped does not.
    if wall is not None:
        points.append((int(wall), points[-1][1] if points else 0))
    return commit, points, wall


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--logs", default="./full-run-logs",
                    help="directory holding the runs' .log files")
    ap.add_argument("--out", default="progress.png")
    args = ap.parse_args()

    if not os.path.isdir(args.logs):
        sys.exit("no such directory: %s (see the docstring for how to fetch "
                 "the logs from compute)" % args.logs)

    runs = []
    for name in sorted(os.listdir(args.logs)):
        if not name.endswith(".log"):
            continue
        commit, points, wall = parse(os.path.join(args.logs, name))
        if not points:
            print("  skipping %s: no [progress] lines" % name)
            continue
        runs.append((commit, points, wall, name))

    if not runs:
        sys.exit("no runs with progress data found in %s" % args.logs)

    fig, ax = plt.subplots(figsize=(11, 6.5))
    series = []
    for commit, points, wall, name in runs:
        xs = [p[0] for p in points]
        ys = [p[1] for p in points]
        term = TERMS.get(commit[:8], "unlabelled")
        label = "%s - %s" % (commit[:8], term)
        if wall is not None:
            label += " (%s)" % human(wall)
        series.append((label, xs, ys))
        # A curve if there is one to draw, otherwise visible points.
        if len(points) >= 2:
            ax.plot(xs, ys, marker="o", markersize=4, linewidth=1.8, label=label)
        else:
            ax.plot(xs, ys, marker="o", markersize=6, linestyle="none",
                    label=label)

    ax.set_xscale("log")
    ax.set_xlabel("elapsed seconds within the run (log scale)")
    ax.set_ylabel("root guesses evaluated")
    ax.set_title("Full N=2340 policy tree runs: search progress over elapsed time")
    ax.grid(True, which="both", alpha=0.25)
    ax.legend(loc="upper left", fontsize=9)

    # The modern runs finish inside the first few minutes, where the main axes
    # squash them into the origin. Zoom that region so they can be compared.
    zoom = ax.inset_axes([0.55, 0.06, 0.42, 0.32])
    for label, xs, ys in series:
        keep = [(x, y) for x, y in zip(xs, ys) if x <= 320]
        if keep:
            zoom.plot([p[0] for p in keep], [p[1] for p in keep], marker="o",
                      markersize=4, linewidth=1.6, label=label.split(" (")[0])
    zoom.set_xlim(0, 320)
    zoom.set_ylim(0, max(1, max((y for _, xs, ys in series
                                 for x, y in zip(xs, ys) if x <= 320), default=1) * 1.15))
    zoom.set_title("first 320 s", fontsize=8)
    zoom.tick_params(labelsize=7)
    zoom.grid(True, alpha=0.25)

    fig.tight_layout()
    fig.savefig(args.out, dpi=140)

    print("wrote %s from %d run(s):" % (args.out, len(runs)))
    for commit, points, wall, name in runs:
        last = points[-1]
        print("  %-8s %-30s %4d samples, final t=%ss root=%s%s"
              % (commit[:8], TERMS.get(commit[:8], "unlabelled"), len(points),
                 last[0], last[1],
                 "" if wall is None else " (wall %s)" % human(wall)))


if __name__ == "__main__":
    main()
