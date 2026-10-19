# Agent Instructions

This file is the single, authoritative source of process rules for whoever
(or whatever) is working on this repo autonomously. If you find guidance
elsewhere - a comment left in an old issue, a stray note on disk, something
from an earlier session - and it conflicts with this file, **this file
wins**. Update this file itself, in a pull request, when a rule needs to
change; don't leave the correction somewhere else.

## The actual goal - read this twice

**Two things are being built, and both of them are the deliverable.**

**1. A faster exact solver, indefinitely.** The solver computes the true optimal
policy for a word list, and the job is to keep making that computation faster on
the one machine we have. There is no target time and no deadline. "Fast enough"
is not a state this project reaches, and the optimization space is nowhere near
exhausted: there is always another concrete thing to measure and try.

**2. A tool anyone can actually use.** Someone who has never seen this repo should
be able to get the CLI, point it at **their own** guesses and candidates lists,
and look at the resulting policy in the visualiser. This is not a nice-to-have
bolted on at the end. It is half the goal, and it is not finished until a stranger
can do it from the documentation alone.

Concretely, the entry point is the `solve` CLI: one command that, given a guesses
file and a candidates file, produces the fully determined policy for the chosen
strategy (`optimal`, or a heuristic like `min-remaining` / `max-freq` for cheaper
testing). Its output is the **policy JSON**, not just the optimal cost, and that
JSON is what the visualiser consumes.

**Nothing here is tied to Wordle's own 2,340 words.** Those are the default input
and the benchmark set, not the product. Arbitrary word and guess lists are the
product, and any assumption that hard-codes the bundled dictionary, its size, or
its letter alphabet is a bug in the tool rather than a benchmark shortcut.

**There is no milestone to hit.** The old time-boxed targets - under 10 hours,
then under 2 hours, then under 20 minutes - were all met, the last one by a wide
margin. Chasing a number is no longer the frame. If setting yourself a target
helps you work, set one, but it is yours, disposable, and it does not change the
job, which is to keep improving the solver and to make the tool usable.

**Exactness is non-negotiable.** This is an exact branch-and-bound solver, not a
heuristic. It must always return the true optimal cost. Never trade away
exactness for speed, and never treat a speedup that changes a golden test's
expected answer as a win to keep - it is a correctness bug to fix.

**Compute is fixed at one machine.** `compute` is `pc01`: its 16 CPU cores and
30 GB RAM (an RTX 2060 was empirically evaluated and found to offer no measurable
speedup over pure CPU SIMD/Rayon, so the GPU path was removed) are the entire
compute budget for this project, permanently. No additional or distributed
compute will ever be provisioned; don't propose or design for a cluster, multiple
nodes, or offloading to any other machine. If parallelism or throughput is the
bottleneck, the lever is using more of *this one machine's* resources better
(more of its cores, a faster-per-core approach), not more machines.

**Completing a full run is a validation checkpoint, not the finish line.** Getting
an answer is not success, and getting a run to completion is not a goal in itself.
Chase the optimizations. See "Full runs" below.

**Keep trying indefinitely, and do not declare the work done.** "The algorithm is
fully optimal", "this cannot be improved further" and "there is nothing left to
do" are not conclusions available to you. This project has repeatedly found more
to do after exactly those claims, and the usability half of the goal is a long way
from finished. If you are genuinely out of concrete ideas, say so plainly in the
progress issue with the evidence for why the obvious next steps do not work. That
is a legitimate, honest update. It is not a licence to go idle, and it is not a
claim that the ceiling has been reached - that determination is the maintainer's
call, not something you declare in an issue or a commit message.

Before it would even be worth raising as a question, the obvious single-machine
avenues need to have actually been tried, not just reasoned about:

- **The GPU avenue is closed, with evidence.** A custom CUDA kernel and driver
  integration were implemented to offload histogram counting and capacity bounds
  to the RTX 2060. Direct measurement on identical candidate sets on `pc01`
  (commit `0ba60c3`) showed the difference was negligible - CPU 2.3% faster at
  N=1000, 2.4% at N=1600, within noise - while CPU used 40% less memory (163 MB
  vs 269 MB). Policy tree child sets partition rapidly into buckets of 50 or
  fewer, below the kernel's offload threshold, so the kernel was virtually never
  invoked during tree search. The path was removed, along with the `nvcc` /
  `g++-13` toolchain dependency. This is the standard to hold other "it can't be
  done" arguments to: measure it on `pc01`, quote the output, then decide.
- **Whether the current architecture or language is the right one** for the
  ceiling you are hitting. A from-scratch rewrite with a fundamentally different
  data layout is a legitimate thing to actually attempt, not just muse about.

If you have genuinely exhausted concrete attempts along those lines and still feel
stuck, open an issue laying out *specifically what you tried and what the evidence
shows*, framed as a question for the maintainer, then pick something else to try
while you wait. Never treat a discouraging estimate as a reason to stop or slow
down.

If you are not certain a change preserves exactness, it is not safe to ship.

## Making it usable by anyone

The second half of the goal, and the half that is easiest to leave unfinished
because nothing forces it. Concretely:

- **Arbitrary input.** `--guesses` and `--candidates` take any two lists of words.
  Nothing may assume the bundled dictionary, its 2,340 candidates, 5-letter words,
  or the A-Z alphabet beyond what the input actually contains. If something only
  works for the default input, that is a bug to fix, not a documented limitation.
- **Obtainable.** Someone with a checkout, or better without one, must be able to
  build or download the CLI by following the README. A build that only works on
  this homelab, or instructions that assume the reader knows the setup, do not
  count.
- **Results they can look at.** The policy JSON goes into the visualiser under
  `site/`, and that has to work for a policy produced from *their* lists, not just
  for the bundled one.
- **Documentation written for a stranger.** The README is the front door. Assume
  the reader has never heard of this repo, this homelab, or Wordle's official word
  lists, and is not going to read this file.

When you make the solver faster, that is the first half. When a stranger can
follow the README end to end and get a policy for their own word list into the
visualiser, that is the second.

## Reporting discipline (read this one twice too)

This project's history includes multiple confident "under 10 hours!" claims that
were later retracted after actually being measured (see the closed milestone
issues for the pattern, or ARCHITECTURE.md's "Known Scaling Behavior" for *why*
small-N extrapolation keeps failing: the search hits discrete depth-transition
cliffs, not a smooth curve, so a power-law fit from N≤1000 data has repeatedly
overshot). Concretely:

- **Never claim a benchmark is running, a job succeeded, or a result was achieved
  without checking the actual exit code, the actual live process, or the actual
  log output.** "I started it" and "it is running" are different claims. Verify
  the second one before asserting it, every time.
- **Every timing or cost claim must trace to captured tool output.** Before
  posting a result, find the exact output line (e.g. `Build time: ...` /
  `Total cost: ...` from a *completed* run) and quote the command and that line in
  the report. A `[progress]` line is not a result, a process that was
  `Killed`/cancelled is not a result, and numbers that only exist in your own
  reasoning are not a result. (A prior session reported a 69 s full run and closed
  issues on figures that never appeared in any output; it was retracted on
  issue #1.)
- **Never extrapolate an ETA from small-N data and report it as if it were
  measured.** If you are extrapolating, say so explicitly, give your confidence
  level, and say what evidence would firm it up. A number without that context
  reads as a measurement, not a guess, and this repo has been burned by exactly
  that.
- **Only trust reproducible, seeded methodology** (e.g. `solve` on a seeded subset
  via `scripts/deploy_and_bench.sh` - see "Tree-only runs" below) for anything you report
  as a real result. A one-off unseeded sample is not evidence of a trend.
- **Don't run benchmarks in a way that produces unreliable numbers** (see "One job
  at a time" below) and then report the result as if it were clean.
- **Correct a claim as soon as you find it was wrong.** Including your own, in
  public, on the issue where you made it. A retraction costs a paragraph; a
  silently wrong number in `.agent/run_history.md` costs everyone who reads it next.

## What runs where

The agent runs on `helios`. `compute` is the machine `pc01`, and every heavy run
happens there. `cbook` is a small always-on helper that runs the control and
monitoring sites; it is not a compute host. Other machines exist on the same
tailnet and are not part of this project: don't design for them, and don't move
work onto them.

- **Fine on `helios`, directly:** `cargo test --release` (the fast unit suite),
  `cargo build`, `cargo clippy`, small and quick checks, anything on the order of
  a few tens of seconds.
- **Must go to `compute` via the scripts below:** anything exhaustive, full-scale
  or open-ended in runtime - tree iteration via `scripts/deploy_and_bench.sh`, and
  especially full runs via `scripts/run_full.sh`.

If you are unsure which side of that line something falls on, wrap it in a short
`timeout` locally rather than guessing.

## Power control goes through UpSnap

`compute` and `helios` are normally powered off when they are idle, and UpSnap on
`cbook` is the single control plane for that. Use the `upsnap` helper rather than
raw Wake-on-LAN packets or an ad-hoc `ssh ... poweroff`, so that every action lands
in one place with one audit trail:

```
upsnap list                 # every device, its address, its state, its capabilities
upsnap wake <device>        # power it on
upsnap shutdown <device>    # power it off
```

Device names are the tailnet names (`pc01`, `helios`, `cbook`, and so on), and the
helper is installed on `helios` at `~/.local/bin/upsnap`. If a device is off and
the helper is unavailable for some reason, `ssh wake-pc01` still works as a
fallback.

The homelab itself - the machines, the tailnet, the control site, the power rules -
is documented separately in `~/projects/homelab/OPERATIONS.md`. Keep that material
there rather than in this repo: this file is about the solver, and that file is
about the machines. Do not put credentials, addresses or MAC addresses in commits
or issues here, because this repo is public.

## Public files and agent files

Everything aimed at you lives in this file, plus the run log and its plot script
in `.agent/`. Everything else in the repo is for the public: the README,
ARCHITECTURE.md, the other docs, the CLI's output and help text, code comments,
tests and the scripts in `scripts/`. Keep those free of this homelab (host
names, the tailnet, power control), the agent and its process, issue numbers,
and retracted or superseded claims. Write them for a stranger who has a
checkout and nothing else. If a public file needs a host, take it from an
environment variable or an argument, and put the value here.

## Workflow on compute

- **Check which machine you are on first** (`hostname`). Reach `compute` over
  Tailscale as `user@pc01`. The scripts in `scripts/` take the host from
  `WORDLE_OPT_COMPUTE_HOST` and refuse to run without it, so prefix every call
  with `WORDLE_OPT_COMPUTE_HOST=user@pc01` (the scripts are public and must not
  name this homelab). The tailnet policy grants agent
  traffic to the homelab nodes in accept mode, so this SSH path needs no
  interactive re-auth. If `pc01` is unreachable, check whether it is simply
  powered off first - wake it with `upsnap wake pc01` - and only treat it as a
  real problem if it is up and the connection still fails.
- **Never hand-roll rsync/ssh/cargo commands against `compute`.** Use
  `scripts/deploy_and_bench.sh` for benchmark iteration and `scripts/run_full.sh` /
  `scripts/check_full.sh` for a full run (see "Full runs" below). They exist so that
  every run, yours or a future session's, syncs, tests and benchmarks the same
  way, and so results are stamped with the commit that actually produced them
  (`WORDLE_OPT_COMMIT`). `git rev-parse` on compute itself cannot work, because
  compute's build directory intentionally has no `.git`.
- **One job at a time on compute, always.** Before starting anything there,
  confirm nothing else is running
  (`ssh user@pc01 "pgrep -fa '[t]arget/release/wordle-opt'"`; the brackets stop
  the pattern from matching the ssh command itself). Concurrent jobs contend
  for the same cores and cache and silently invalidate both jobs' timings. A
  number produced next to another running job is not real data. Don't report or
  commit it as if it were.
- **Never let a command run unbounded**, even on compute. Wrap anything whose
  runtime isn't already bounded in `timeout`.
- **Don't leave stray files on compute.** Its build directory is a disposable
  rsync target (`--delete`d and rebuilt every run), not a place for notes, scratch
  scripts or alternate checkouts. If you need to leave code changes anywhere,
  commit them to this repo.
- **Don't block synchronously waiting on a long job.** `scripts/run_full.sh` launches the
  full run detached and returns immediately; poll it later with `scripts/check_full.sh`
  instead of sitting idle. The same goes for anything you background manually:
  background it, note it in the progress issue, and do something else meanwhile.

## Tree-only runs: all runs output the policy tree

Every run's output is the **policy tree**. The cost-only modes - the `benchmark` /
`benchmark-random` / `diagnose` subcommands, `solve --compare`'s cost-only
baseline, and the standalone cost-only entry points behind them - have been
removed (issue #13). A run that reports a cost instead of producing a tree
measures the wrong thing: it skips exactly the work the deliverable is about,
which is how the information the tree needs is determined, stored, maintained
through the run, and finally serialized. Optimizing the algorithm therefore means
optimizing the tree build.

What stays is the machinery the tree is built from: the solver internals that
compute costs and bounds, and the heuristic strategies (`min-remaining`,
`max-freq`) that produce trees and exercise the same path cheaply. What goes is
any user-facing run whose output is a bare cost.

To run one cheaply, a tree at a smaller candidate set, reproducible from a seed:

```
WORDLE_OPT_COMPUTE_HOST=user@pc01 scripts/deploy_and_bench.sh solve --strategy optimal --max-candidates 500 --sample-seed 1 --output /tmp/n500.json
```

The subset is drawn from `subset_rng(seed, size)`, so **(commit, seed, size) is the
run's identity**: the run prints that triple into its log, and its timing is only
comparable against a run with the same triple. Record every completed run, seed
included, in `.agent/run_history.md`. That is what builds the size-vs-time trend, and it
is how a full run that got faster while small runs got slower becomes visible
rather than hidden.

## Full runs

A full run solves the whole default candidate set end to end. There is no `full`
subcommand: `scripts/run_full.sh` runs `solve --strategy optimal` on the bundled
lists with a 2 GB transposition table. Its output is the
**policy tree**, not just the optimal cost. Construct the readable JSON only after
the solve has finished: during the run keep everything in the efficient in-memory
representation, and build the JSON once the solve completes, so serialization adds
no meaningful overhead to the solve itself.

**One per performance-relevant change, not one per day.** The old cap of one a
day was written when a run took hours; it now takes minutes, so the clock is not
the constraint. What that cap was really protecting was signal - a full run is a
checkpoint only if it marks a real change, and running one after every commit
turns the trend into noise. So run one whenever a branch carries a
performance-relevant change, as many times in a day as that happens, and skip it
when nothing performance-relevant changed: a run whose commit differs only in
docs or tests measures nothing new. Two runs cannot collide, because every
artifact is stamped with the commit it ran as.

When to run it is your judgment, not a schedule:

- Run one when a branch carries a **performance-relevant change**, and skip it
  when nothing performance-relevant changed. A run whose commit differs only in
  docs or tests measures nothing new. The result goes in the pull request, so
  the maintainer sees it before merging.
- Nothing schedules it. There is no daily job.
- Only launch it when large-N evidence gives a specific, verified reason to expect
  it will finish in a bounded time (see ARCHITECTURE.md's "Known Scaling
  Behavior" for why small-N extrapolation alone is not that evidence).
- It **requires a clean tree**. `scripts/run_full.sh` refuses to start with uncommitted
  changes, because the run records the commit it ran as and an uncommitted edit
  would make that record wrong.
- Every run keeps its own artifacts, stamped with the UTC day and the commit
  (`~/full-runs/<YYYY-MM-DD>-<commit>.{json,log}` in compute's home), so no
  run can overwrite another's data. **Add a row for each completed run to
  `.agent/run_history.md`.** That repo-side index is what makes the trend visible and what
  survives a compute rebuild.

Operationally:

- `scripts/run_full.sh` syncs, builds, tests, then launches the solve **detached** and
  returns immediately. It does not wait for it to finish. The `timeout` is a safety
  cap: killing a run early throws away the tree it was building.
- `scripts/check_full.sh` is the cheap, near-instant status check: still running?
  crashed? done? Poll it on your own schedule instead of blocking on the run.
- The run keeps its artifacts in `~/full-runs/` in compute's home directory, not
  in the rsync target, so neither the next deploy nor a later run can delete or
  overwrite them.
- The run bounds its own memory: a 2 GB transposition table and a
  4,096-entry-per-shard equivalence-cache cap. (The transposition table allocates
  two AtomicU64 slots per unit - 16 bytes - and each cached projection is about
  2 KB, so the historical defaults could reach tens of GB: the first full tree run
  was OOM-killed at 38 minutes before these bounds.) Progress lines include
  `rss_mb` so memory can be watched live.

## Correctness practice

- `cargo test --release` (golden regression tests plus the brute-force
  comparisons against `src/naive.rs`) must pass before and after every change that
  touches solver, heuristic or cache logic. A golden test failure means "figure
  out why the new number is correct" or "this change is wrong" - never "update the
  constant to match."
- The integration tests in `tests/` are split by kind of check (the README has
  the table) and share independent helpers in `tests/common/`: their own
  feedback function, tree walker and bitmask brute-force oracle.
  `brute_force.rs` compares the solver with two brute-force solvers live on
  small lists, so it needs no cached answers. `subtrees.rs`,
  `alternative_guesses.rs`, `metamorphic.rs` and `planted_optima.rs` check
  optimality where whole-instance brute force cannot reach. The ignored
  `full_scale.rs` rebuilds the 2,340-candidate tree with the current solver and
  audits it, and the ignored `published_optima.rs` requires the optima Alex
  Selby's independent solver published for the lists in `words/published/`
  (7,920 for the original lists, among others). Run both after any change to
  solver, bound or cache logic:
  `cargo test --release --test published_optima --test full_scale -- --ignored`.
  A disagreement there is a correctness bug in this solver until proven
  otherwise. After any change to the parallel scans, the global beta or what
  goes into the transposition table, also run the ignored
  `parallel_consistency.rs`, which repeats a multi-threaded solve that used to
  go wrong only under some thread timings (issue #14), and `tt_audit.rs`
  (`--features tt-audit`), which checks every table write rather than only the
  answer: with the issue #14 guard removed it flagged 33 of 600 runs, against
  6 of 600 with a wrong answer.
- Optimality certificates are a further, independent audit: `solve --certify`
  writes one and `check` verifies it with a checker that shares no search
  code with the solver (`src/certcheck.rs`). After a change to solver,
  bound or cache logic, certify a full run now and then and check it, as with
  the ignored full-scale tests. A rejected certificate for a tree the solver
  called optimal is a correctness bug until proven otherwise.
- Extend the golden and regression suite as a normal part of ongoing work, the way
  a senior engineer maintains test coverage on a codebase they own, not just when
  told to.
- If you add a correctness-critical code path (concurrency, unsafe, lock-free
  structures), review it adversarially yourself before trusting it: race
  conditions, memory ordering, ABA problems, use-after-free-shaped bugs. There is
  no separate review pass checking this for you. The self-review is the review.

## Branches, pull requests and releases

`main` is the published branch: every commit on it that passes CI is built and
put in the `latest` release, so whatever lands there is what strangers
download.

Two rules:

1. **Every change to `main` goes through a pull request.** GitHub branch
   protection enforces this, so a direct push or force-push is rejected.
2. **Never merge a pull request yourself.** Only the maintainer merges, after
   reviewing it or explicitly saying to go ahead. This is a rule to keep, not
   something the repository enforces.

Also: never push tags, never create, edit or delete releases, and never run the
"Prebuilt binaries" workflow by hand.

### One long-lived pull request

Performance work, and generalising beyond five-letter words, all belongs in
**one** pull request that stays open: keep committing to its branch and pushing
as you go. Do not open a separate PR for each experiment or micro-optimisation.
The maintainer reviews that PR when it is ready, and CI runs then; you do not
need a PR (or a CI run) per push.

Cut a second branch only for work that is genuinely separate from performance
and generalisation, and say in its PR why it is not part of the main one.

Rebase on `main` when it moves, so the long-lived branch stays reviewable. Before
marking the PR ready for review, run these and report the results in the PR
description:

- `cargo test --release`;
- the ignored tests:
  `cargo test --release --test published_optima --test full_scale --test parallel_consistency -- --ignored`;
- `cargo test --release --features tt-audit --test tt_audit -- --ignored`
  if the change touches the parallel scans, the global beta or what is stored
  in the transposition table;
- a full certify-and-check on the compute machine for any change to solver,
  bound or cache logic (see "Correctness practice").

Every performance claim needs timing evidence: interleaved runs against `main`,
with the seeds recorded in `.agent/run_history.md`, and the command and its
captured output quoted in the PR. A difference smaller than the run-to-run
spread is not a speedup; quote the runs you compared rather than calling it a
record.

Progress goes to the single open "Agent progress log" issue (#1), not to a new
issue per update. Review requests are pull requests. (Issue #9 is closed; do not
reopen or duplicate it.) A failed release build is a bug to fix like a failed
test: report it in the progress issue, do not re-run or edit the release
yourself.

## Communication and repo hygiene

- Commit code to this repo, and communicate with the maintainer through GitHub
  Issues rather than commit messages or files scattered on disk. Post progress to
  the single ongoing "Agent progress log" issue (#1) rather than opening a new
  issue per update.
- Before opening a new milestone-style issue, check whether an equivalent one is
  already open and comment there instead of creating a duplicate. (This repo once
  had three separate "MILESTONE: full-scale estimate under 10 hours" issues open at
  once, from repeated claims that each later turned out to be wrong. Don't repeat
  that.)
- Check open issues for replies each time you start or resume a session.
- Keep the working tree clean: commit or discard changes before moving to new
  work, and delete one-off and scratch files when you are done with them rather
  than leaving them untracked.
- Push your branch regularly. Don't let commits sit local-only: a commit that
  exists on one machine only is one disk failure away from being lost, and the
  pull request is the only place the maintainer sees the work. (This has
  happened: 11 commits, including a correctness fix to `build.rs`, sat unpushed
  for over a day.)
- Every commit on `main` that passes CI is built and published as the
  `latest` release (`.github/workflows/release.yml`), so whatever lands on
  `main` is what strangers download. Only the maintainer merges to `main`.
  After a merge, check that both "Rust CI" and "Prebuilt binaries" succeeded; a
  failed release build is a bug to fix like a failed test, and it is reported,
  not fixed by hand.
- **Always run the exact CI gates before pushing.** You must have a pre-push hook
  configured (`git config core.hooksPath .githooks`) containing:
  `cargo fmt --all -- --check && cargo clippy --release -- -D warnings && cargo test --release`.
  The hook also refuses any push to `main` or to a tag.
- Commit messages: terse and professional, describing what changed and why. No AI
  branding, no narrating your own process. The existing git history
  (`git log --oneline`) is a good model to match.
- The maintainer reviews and merges every pull request. There is no separate
  automated reviewer, whether a different model or a scheduled process: you are
  responsible for your own correctness review before opening a PR.

## If you're blocked

If compute or GitHub is unreachable, or you are stuck on a decision only the
maintainer can make: open an issue describing it and move on to other independent
work (more tests, more analysis of existing data, documentation, cleanup). Don't
stall waiting for a reply, and don't spend a long stretch reasoning about an
untestable change in the abstract. A small verified step beats a large unverified
argument.
