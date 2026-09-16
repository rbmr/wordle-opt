# Agent Instructions

This file is the single, authoritative source of process rules for whoever
(or whatever) is working on this repo autonomously. If you find guidance
elsewhere - a comment left in an old issue, a stray note on disk, something
from an earlier session - and it conflicts with this file, **this file
wins**. Update this file itself, in a commit, when a rule needs to change;
don't leave the correction somewhere else.

## The actual goal - read this twice

**The deliverable is the algorithm, not the answer.** The target is a
solver that is *provably capable* of computing the true optimal Wordle
strategy for the full ~2340-word set in under 10 hours of wall-clock time
on the `compute` host - the engineering artifact (the code, its
correctness guarantees, the optimizations that get it under that bar) is
what's being built. The optimal cost number itself doesn't change between
commits; only how fast it's reached does, and that speed is the entire
point.

A consequence that's easy to get backwards: **completing one full N=2340
run is a rare validation checkpoint, not the finish line.** Don't treat
"we got an answer" as success, and don't chase getting a run to completion
as a goal in itself - chase the optimizations. See "The `full` run is rare"
below.

**Exactness is non-negotiable.** This is an exact branch-and-bound solver,
not a heuristic. It must always return the true optimal cost. Never trade
away exactness for speed, and never treat a speedup that changes a golden
test's expected answer as a win to keep - it's a correctness bug to fix.
If you're not certain a change preserves exactness, it isn't safe to ship.

## Reporting discipline (read this one twice too)

This project's history includes multiple confident "under 10 hours!"
milestone claims that were later retracted after actually being measured
(see the closed/superseded milestone issues for the pattern, or
ARCHITECTURE.md's "Known Scaling Behavior" for *why* small-N extrapolation
keeps failing: the search hits discrete depth-transition cliffs, not a
smooth curve, so a power-law fit from N≤1000 data has repeatedly
overshot). Concretely, from here on:

- **Never claim a benchmark is running, a job succeeded, or a milestone is
  met without checking the actual exit code / actual live process /
  actual log output.** "I started it" and "it is running" are different
  claims - verify the second one before asserting it, every time.
- **Never extrapolate an ETA from small-N data and report it as if it were
  measured.** If you're extrapolating, say so explicitly, give your
  confidence level, and say what evidence would firm it up. A number
  without that context reads as a measurement, not a guess, and this repo
  has been burned by exactly that.
- **Only trust reproducible, seeded methodology** (`benchmark`,
  `benchmark-random` - see README) for anything you report as a real
  result. A one-off unseeded sample is not evidence of a trend.
- **Never declare the algorithm "fully optimal" or "can't be improved
  further" as a way to stop working.** There is always more available:
  more tests, more analysis of existing benchmark data, another
  optimization angle, documentation, cleanup. If you're genuinely out of
  concrete ideas, say exactly that in the progress issue, with the
  specific evidence for why the obvious next steps don't work - that's a
  legitimate, honest update. It is not a license to go idle.
- Don't misinterpret or run benchmarks in a way that produces unreliable
  numbers (see "One job at a time" below) and then report the result as if
  it were clean.

## Workflow on compute

- **Never hand-roll rsync/ssh/cargo commands against `compute`.** Use
  `deploy_and_bench.sh` for benchmark iteration and `run_full.sh` /
  `check_full.sh` for the full run (see README). They exist so every run -
  yours or a future session's - syncs, tests, and benchmarks the same
  way, and so results are stamped with the commit that actually produced
  them (`WORDLE_OPT_COMMIT` - see README's note on this; `git rev-parse` on
  compute itself cannot work, since compute's build directory intentionally
  has no `.git`).
- **One job at a time on compute, always.** Before starting anything
  there, confirm nothing else is already running
  (`ssh compute pgrep -f target/release/wordle-opt`). Concurrent jobs
  contend for the same cores/cache and silently invalidate both jobs'
  timings - a number produced next to another running job is not real
  data, don't report or commit it as if it were.
- **Never let a command run unbounded**, even on compute. Wrap anything
  whose runtime isn't already bounded in `timeout`.
- **Don't leave stray files on compute.** Its build directory is a
  disposable rsync target (`--delete`d and rebuilt every run) - it is not
  a place to keep notes, scratch scripts, or alternate checkouts. If you
  need to leave code changes anywhere, commit them to this repo.
- **Don't block synchronously waiting on a long job.** `run_full.sh`
  starts the full run detached and returns immediately; poll it later with
  `check_full.sh` instead of sitting idle. The same applies to anything
  you background manually - background it, note it in the progress issue,
  and go do something else in the meantime.

## The `full` run is rare

`run_full.sh` is bounded by `timeout` at the 10-hour milestone itself (a
run that hasn't finished by then has already answered the question) and
refuses to start a second concurrent run. But the bigger constraint is
upstream of the script: **only launch a `full` run when diagnose/benchmark
data at large N gives you a specific, verified reason to expect it will
finish in a bounded time.** Don't relaunch it reflexively after every
change hoping it now works, and don't launch it just to "get to an
answer" - every hour it runs is an hour `compute` is unavailable for the
benchmark iteration that's the actual day-to-day work.

## Correctness practice

- `cargo test --release` (golden regression tests + the `verify`
  differential fuzzer against `src/naive.rs`) must pass before and after
  every change that touches solver/heuristic/cache logic. A golden test
  failure means "figure out why the new number is correct" or "this change
  is wrong" - never "update the constant to match."
- Extend the golden/regression suite as a normal part of ongoing work, the
  same way a senior engineer would maintain test coverage on a codebase
  they own - not just when told to.
- If you add a new correctness-critical code path (concurrency, unsafe,
  lock-free structures), think about it adversarially yourself before an
  independent review catches it: race conditions, memory ordering, ABA
  problems, use-after-free-shaped bugs.

## Communication and repo hygiene

- Commit code to this repo; communicate with the maintainer through GitHub
  Issues, not through commit messages or files scattered on disk. Post
  progress to the single ongoing "Agent progress log" issue rather than
  creating a new issue per update.
- Before opening a new "MILESTONE" issue, check whether an equivalent one
  is already open - comment on the existing one instead of creating a
  duplicate. (This repo has had 3 separate "MILESTONE: full-scale estimate
  under 10 hours" issues open at once from repeated claims that each later
  turned out to be wrong; don't repeat that.)
- Check open issues for replies each time you start or resume a session.
- Keep the working tree clean: commit or discard changes before moving to
  new work, and delete one-off/scratch files as soon as you're done with
  them rather than leaving them untracked.
- Commit messages: terse, professional, describe what changed and why -
  no AI branding, no narrating your own process. The existing git history
  on this repo (`git log --oneline`) is a good model to match.
- An independent review pass (a different model) periodically audits this
  repo and posts findings to issue #1 prefixed "### Independent Review".
  Treat its findings seriously - it has caught real correctness
  regressions and a broken test-registration bug before. If it opens a
  "REVIEW FINDING" issue, that takes priority over other work until
  resolved or explicitly triaged as a false positive (with reasoning
  posted to the issue, not just closed silently).

## If you're blocked

If compute or GitHub is unreachable, or you're stuck on a decision only
the maintainer can make: open an issue describing it and move on to other
independent work (more tests, more analysis of existing data, cleanup) -
don't stall waiting for a reply, and don't spend a long stretch reasoning
about an untestable change in the abstract. A small verified step beats a
large unverified argument.
