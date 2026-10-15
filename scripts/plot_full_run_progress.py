#!/usr/bin/env python3
"""Plot every recorded full N=2340 run's progress, in a form that compares speed.

A raw work counter is a poor progress axis. The optimised runs evaluate far less
of everything, so against a shared y-axis they sit *below* the baseline at every
timestamp and read as "behind" when they are in fact doing thousands of times
less work. Two panels fix that, and together they separate the two reasons a run
got faster:

  1. Progress, as a percentage of that run's own total states evaluated. Every
     curve runs 0 to 100, so the comparison is purely horizontal: the distance
     to the right-hand end is the wall-time speedup. The shape also shows where
     each run spends its effort.
  2. Total states evaluated per run, on a log axis. This is the other half: how
     much less work the optimised runs need at all.

Data comes from each run's own log, which is the authoritative record and is kept
on the compute host:

    scp user@pc01:'~/full-runs/*.log' ./full-run-logs/

    ./scripts/plot_full_run_progress.py --logs ./full-run-logs --out progress.png

Honesty notes, because this plot has been wrong in these two ways before:

  * The sampler ticks every 60 s, so the last `[progress]` line can be a minute
    before the run finished. The denominator is therefore the run's own final
    total from its summary (`States evaluated:`), not the last sample, and the
    bar chart uses the same number. An earlier version instead appended a point
    at the wall time carrying the last *sampled* count, which drew a flat tail
    asserting the counter had stalled. It had not.
  * One appended point remains, and it is not invented: at the run's wall time
    it had finished all of its work, so (wall, 100%) is known exactly. The
    segment leading to it is a straight interpolation between the last real
    sample and that certain endpoint, and nothing in between is claimed.
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
    r"^\[progress\]\s+elapsed=(\d+)s.*?states=(\d+)")
COMMIT_RE = re.compile(r"commit=([0-9a-f]+)")
WALL_RE = re.compile(r"Wall time:\s*([0-9.]+)s")
TOTAL_RE = re.compile(r"^States evaluated:\s*(\d+)")


def human_time(seconds):
    if seconds >= 3600:
        return "%.2f h" % (seconds / 3600.0)
    if seconds >= 60:
        return "%.2f min" % (seconds / 60.0)
    return "%.1f s" % seconds


def human_count(n):
    if n >= 1_000_000:
        return "%.1fM" % (n / 1_000_000.0)
    if n >= 1_000:
        return "%.1fk" % (n / 1_000.0)
    return "%d" % n


def parse(path):
    """Pull the commit, the cumulative states series and the totals from a log."""
    commit = "unknown"
    points = []
    wall = None
    total = None
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
            m = TOTAL_RE.match(line)
            if m:
                total = int(m.group(1))
    return commit, points, wall, total


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
        commit, points, wall, total = parse(os.path.join(args.logs, name))
        if not points:
            print("  skipping %s: no [progress] lines" % name)
            continue
        if total is None:
            # Without the summary total the curve cannot be normalised, and
            # guessing one is what produced the bad plot this replaces.
            print("  skipping %s: no 'States evaluated:' summary line" % name)
            continue
        if total < points[-1][1]:
            print("  skipping %s: summary total %d is below the last sample %d"
                  % (name, total, points[-1][1]))
            continue
        runs.append((commit, points, wall, total, name))
    if not runs:
        sys.exit("no usable runs found in %s" % args.logs)

    # The slowest run anchors the speedup ratios.
    slowest = max(r[2] for r in runs if r[2] is not None)

    fig, (ax, axw) = plt.subplots(
        1, 2, figsize=(14.0, 6.4), gridspec_kw={"width_ratios": [2.4, 1.0]})

    order = sorted(runs, key=lambda r: r[2] if r[2] is not None else float("inf"))
    for commit, points, wall, total, name in order:
        term = TERMS.get(commit[:8], "unlabelled")
        xs = [p[0] for p in points]
        pct = [100.0 * p[1] / total for p in points]
        if wall is not None:
            # Certain endpoint: the run had done all of its work by its wall
            # time, whatever the sampler last saw.
            xs.append(wall)
            pct.append(100.0)
        bits = []
        if wall is not None:
            bits.append(human_time(wall))
            if slowest > 0:
                bits.append("%.1fx" % (slowest / wall))
        bits.append("%s states" % human_count(total))
        label = "%s - %s  (%s)" % (commit[:8], term, ", ".join(bits))
        line, = ax.plot(xs, pct, marker="o", markersize=4, linewidth=1.9,
                        label=label)
        axw.barh("%s - %s" % (commit[:8], term), total,
                 color=line.get_color(), height=0.55)

    ax.set_xscale("log")
    ax.set_xlabel("elapsed seconds within the run (log scale)")
    ax.set_ylabel("work done, as a percentage of that run's own total states")
    ax.set_title("Full N=2340 runs: progress against elapsed time\n"
                 "every curve ends at 100 percent, so the gap to the right is "
                 "the speedup", fontsize=12)
    ax.set_ylim(0, 105)
    ax.set_axisbelow(True)
    ax.grid(True, which="both", alpha=0.25)
    ax.legend(loc="lower right", fontsize=8.5)

    axw.set_xscale("log")
    axw.set_xlim(left=1000)
    axw.set_xlabel("total states evaluated (log scale)")
    axw.set_title("Total work per run", fontsize=12)
    axw.set_axisbelow(True)
    axw.grid(True, which="both", axis="x", alpha=0.25)
    axw.tick_params(labelsize=8)

    fig.tight_layout()
    fig.savefig(args.out, dpi=140)

    print("wrote %s from %d run(s):" % (args.out, len(runs)))
    for commit, points, wall, total, name in order:
        speedup = "" if wall is None else "  %.1fx vs slowest" % (slowest / wall)
        print("  %-8s %-28s %2d samples, last sample t=%ss (%s of total), "
              "total states=%s, wall=%s%s"
              % (commit[:8], TERMS.get(commit[:8], "unlabelled"), len(points),
                 points[-1][0], "%.0f%%" % (100.0 * points[-1][1] / total),
                 human_count(total),
                 "unknown" if wall is None else human_time(wall), speedup))


if __name__ == "__main__":
    main()
