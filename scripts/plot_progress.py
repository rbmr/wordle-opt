#!/usr/bin/env python3
"""Plot wordle-opt progress over time, by commit.

Two panels:

  1. Full tree runs (full_run_history.md): wall time in hours per completed run,
     annotated with the commit, against the 2-hour milestone line. This is the
     headline trend - the deliverable is "full N=2340 in under 2 hours".
  2. Benchmark runs (benchmark_history.md): average solve time per candidate
     size, one line per size, chronological by commit. This is the fine-grained
     record of what each optimisation actually bought, at sizes cheaper than the
     full run. (These rows come from the cost-only benchmark tools, which are
     being removed - see issue #13 - so this panel is a historical record and
     will simply stop gaining points once that lands.)

Every axis label is an explicit string; nothing is read out of the data to
become a label, so a missing field can never render as "undefined" - it shows as
"unknown" instead.

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


def read(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError:
        return ""


def parse_full_runs(text):
    """Rows of full_run_history.md -> [(date, commit, hours)]. Ignores the header
    and separator rows; a row without a parseable wall time is skipped."""
    runs = []
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 4 or cells[0].startswith("---") or cells[0].lower().startswith("date"):
            continue
        date = cells[0]
        commit = cells[1].strip("`") or "unknown"
        m = re.search(r"([\d.]+)\s*s", cells[3])
        if not m:
            continue
        runs.append((date, commit, float(m.group(1)) / 3600.0))
    return runs


def parse_benchmarks(text):
    """benchmark_history.md -> {size: [(order, commit, avg_seconds)]} in the file's
    chronological order. Each block starts with a '## ... commit=<hash>' header."""
    out = {}
    order = 0
    commit = "unknown"
    for line in text.splitlines():
        if line.startswith("##"):
            m = re.search(r"commit=([0-9a-fA-F]+)", line)
            commit = m.group(1) if m else "unknown"
            order += 1
            continue
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 4 or not cells[0].isdigit():
            continue
        try:
            size = int(cells[0])
        except ValueError:
            continue
        times = cells[3].split("/")
        try:
            first = float(times[0] if len(times) == 1 else times[1])
        except (ValueError, IndexError):
            continue
        out.setdefault(size, []).append((order, commit, first))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(REPO, "progress.png"))
    ap.add_argument("--full-history", default=os.path.join(REPO, "full_run_history.md"))
    ap.add_argument("--bench-history", default=os.path.join(REPO, "benchmark_history.md"))
    args = ap.parse_args()

    runs = parse_full_runs(read(args.full_history))
    bench = parse_benchmarks(read(args.bench_history))

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(15, 5.2))

    if runs:
        xs = list(range(1, len(runs) + 1))
        ys = [h for _, _, h in runs]
        labels = [f"{c}\n{d}" for d, c, _ in runs]
        ax1.plot(xs, ys, marker="o", color="#2b8cbe", lw=2)
        for x, y, (_, c, h) in zip(xs, ys, runs):
            ax1.annotate(f"{h:.2f} h", (x, y), textcoords="offset points", xytext=(0, 8),
                         ha="center", fontsize=8)
        ax1.axhline(MILESTONE_HOURS, color="#d95f0e", ls="--", lw=1.5,
                    label=f"milestone ({MILESTONE_HOURS:.0f} h)")
        ax1.set_xticks(xs)
        ax1.set_xticklabels(labels, fontsize=8)
        ax1.legend(fontsize=8)
    else:
        ax1.text(0.5, 0.5, "no completed full runs recorded yet", ha="center", va="center",
                 transform=ax1.transAxes, color="#555")
    ax1.set_xlabel("completed full run (commit over date, UTC)")
    ax1.set_ylabel("wall time (hours)")
    ax1.set_title("Full N=2340 tree runs over time")
    ax1.grid(True, alpha=0.3)

    if bench:
        cmap = plt.get_cmap("viridis")
        sizes = sorted(bench)
        for i, size in enumerate(sizes):
            pts = sorted(bench[size])
            xs = [p[0] for p in pts]
            ys = [p[2] for p in pts]
            commits = [p[1] for p in pts]
            ax2.plot(xs, ys, marker="o", ms=3, lw=1.6, color=cmap(i / max(1, len(sizes) - 1)),
                     label=f"N={size}")
            for x, y, c in zip(xs, ys, commits):
                ax2.annotate(c, (x, y), textcoords="offset points", xytext=(0, -10),
                             ha="center", fontsize=6, rotation=90, color="#666")
        ax2.set_yscale("log")
        ax2.legend(fontsize=8, ncol=2)
    else:
        ax2.text(0.5, 0.5, "no benchmark history found", ha="center", va="center",
                 transform=ax2.transAxes, color="#555")
    ax2.set_xlabel("benchmark run, chronological (commit shown under each point)")
    ax2.set_ylabel("average solve time (seconds, log scale)")
    ax2.set_title("Benchmark sizes over time (legacy cost-only record)")
    ax2.grid(True, alpha=0.3)

    fig.suptitle("wordle-opt progress by commit - generated "
                 + datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%d %H:%M UTC"), fontsize=10)
    fig.tight_layout(rect=(0, 0, 1, 0.95))
    fig.savefig(args.out, dpi=110)
    print(f"wrote {args.out}")

    print("\nfull runs (newest last):")
    for date, commit, hours in runs:
        print(f"  {date}  {commit:>10}  {hours:6.2f} h")
    if not runs:
        print("  (none recorded)")

    print("\nbenchmark history: latest average time per size")
    for size in sorted(bench):
        o, c, t = sorted(bench[size])[-1]
        print(f"  N={size:<5} latest {t:9.2f}s  ({c}, run {o})")


if __name__ == "__main__":
    sys.exit(main())
