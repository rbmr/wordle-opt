# wordle-opt

wordle-opt computes the optimal strategy for Wordle-style word games. You give
it a list of possible answers and a list of allowed guesses. It returns the
strategy that solves every answer with the fewest guesses in total, as a
decision tree you can open in a browser.

The result is exact, not a heuristic. The solver considers every allowed guess
at every step. It skips a guess only when a bound proves that the guess cannot
beat the best strategy found so far.

## Results

For the bundled word lists (2,340 answers, 14,855 allowed guesses), the optimal
strategy needs 8,001 guesses in total, or 3.419 guesses per game on average.
It opens with `tarse` and needs at most 6 guesses for any answer. Building that
tree takes about 37 seconds on a 16-core machine.

The solver also reproduces every optimum that Alex Selby published for the
official Wordle lists, which were computed with his own independent program:

| word lists | answers | guesses | optimum | first guess |
|---|--:|--:|--:|---|
| original Wordle | 2,315 | 12,972 | 7,920 | salet |
| NYT, February 2022 | 2,309 | 12,947 | 7,897 | salet |
| NYT, March 2022 | 2,309 | 12,972 | 7,897 | salet |
| NYT, August 2022 | 2,309 | 14,855 | 7,883 | tarse |
| NYT, July 2023 | 3,158 | 14,855 | 11,219 | tarse |

The lists and the sources of these numbers are in
[`words/published/`](words/published/README.md).

## Installation

There are no prebuilt binaries. You need a Rust toolchain, which you can
install with [rustup](https://rustup.rs). The repository pins the Rust version
in `rust-toolchain.toml`, and rustup downloads it on the first build.

```bash
git clone https://github.com/rbmr/wordle-opt.git
cd wordle-opt
cargo build --release
```

The binary is `target/release/wordle-opt`. To put it on your `PATH` instead:

```bash
cargo install --path .
```

The build is tuned for the CPU it is built on (`target-cpu=native` in
`.cargo/config.toml`). Build it on the machine where you will run it.

The bundled word lists are compiled into the binary, so it works from any
directory.

## Quick start

Solve the bundled lists and write the tree to a file:

```bash
wordle-opt solve --output tree.json
```

Solve your own lists:

```bash
wordle-opt solve \
  --guesses my_guesses.txt \
  --candidates my_answers.txt \
  --output my_tree.json
```

Then open the viewer at <https://rbmr.github.io/wordle-opt/>, click
**Load tree**, and choose the file. The tree is checked in the browser and
displayed. The file is read locally and is not uploaded anywhere.

## Input files

Both files contain one word per line.

- Every word must have exactly 5 characters.
- Any characters are allowed, not just `a` to `z`. Letters are converted to
  lowercase, so `Crane` and `crane` are the same word.
- Blank lines and duplicate words are ignored.
- Every answer is added to the guess list automatically, so the guess file does
  not need to repeat them.
- The guess list can hold at most 65,535 words.

Solve time grows quickly with the number of answers. A few hundred answers take
seconds. About 2,300 answers take 30 to 50 seconds on 16 cores, and 3,158
answers took about 5.5 minutes. To try out a large list, solve a subset first
with `--max-candidates`.

## Commands

### `solve`

Builds a policy tree, checks it, and writes it as JSON.

| flag | meaning |
|---|---|
| `--output <path>` | Where to write the tree. Required. |
| `--strategy <name>` | `optimal` (default), or one of the faster heuristics `min-remaining` (fewest expected remaining answers) and `max-freq` (most common letters). |
| `--guesses <path>` | Allowed guesses. Defaults to the bundled 14,855 words. |
| `--candidates <path>` | Possible answers. Defaults to the bundled 2,340 words. |
| `--max-candidates <N>` | Use only the first `N` answers in alphabetical order. |
| `--sample-seed <S>` | With `--max-candidates`, pick `N` answers at random instead, reproducibly for a given seed. |
| `--no-progress` | Do not print progress lines while solving. |
| `--cache-entries <N>` | Size of the solver's cache, as a power of two. The default is 4,194,304 entries (64 MB). |
| `--stats <path>` | Also write a progress time series of the build (nodes, cache hits, and so on). |
| `--stats-format <fmt>` | `parquet` (default) or `ndjson`. |

When it finishes, `solve` prints the total cost, the average number of guesses
and the build time. Its `Max depth` line counts the edges from the root, so the
most guesses any answer needs is that number plus one.

### `validate`

```bash
wordle-opt validate tree.json
```

Checks a tree file without trusting anything else about it. It recomputes which
answers are still possible at every node and checks that:

- each node has a child for exactly the responses that are still possible,
- every answer is guessed exactly once, at the end of its path,
- the tree matches the word lists stored in the file.

It then prints the total cost. The file contains its own word lists, so no other
files are needed.

### `verify`

```bash
wordle-opt verify
```

Compares the solver with a slow, simple reference solver (`src/naive.rs`) on
random small subsets of the bundled lists.

## Tree format

A policy tree has a guess at every node. Each edge is a response to that guess,
written as five letters: `g` for green, `y` for yellow, `b` for gray. The
all-green response has no edge, because it means the game is won.

```json
{
  "format": "wordle-policy-tree",
  "version": 1,
  "strategy": "optimal",
  "dictionary_hash": "0x890d6b04b8d49ef6",
  "guesses": ["aahed", "..."],
  "candidates": ["aback", "..."],
  "root": {
    "guess": "tarse",
    "children": {
      "bbbbb": { "guess": "godly", "children": { "...": "..." } }
    }
  }
}
```

The file stores both word lists, so it can be checked and displayed on its own.
The `dictionary_hash` is a checksum of the two lists, and a tree is rejected if
it does not match them.

The set of answers still possible at a node is not stored. It follows from the
responses on the path to that node, and the validator and the viewer recompute
it.

## Viewer

The viewer in `site/` is a static web page with no dependencies. It is
published at <https://rbmr.github.io/wordle-opt/>, and you can also open
`site/index.html` directly from disk.

It loads the bundled example trees or a tree you made with `solve`, and checks
it with the same rules as `validate` before showing it. It has two modes:

- **Play** walks through the strategy like a game. Click the letters of the
  current guess to set the response you got, then submit. The viewer shows the
  next guess, how many answers are still possible, and the expected number of
  guesses left.
- **Explore** shows statistics for the tree, a chart of how many games take 1,
  2, 3 or more guesses, and the full tree, which you can expand and search.

The viewer reads the word length from the tree, so it also works with trees
for words of other lengths. `site/examples/` has 3, 4 and 6 letter examples.
The solver itself only handles 5-letter words.

Bundled examples:

| example | strategy | answers | nodes | most guesses | total cost | mean guesses |
|---|---|--:|--:|--:|--:|--:|
| `optimal` | optimal | 2,340 | 2,478 | 6 | 8,001 | 3.419 |
| `min-remaining` | min-remaining | 2,340 | 2,934 | 5 | 8,561 | 3.659 |
| `max-freq` | max-freq | 2,340 | 2,908 | 9 | 9,545 | 4.079 |

`site/examples/README.md` explains how they were generated.

## How the solver works

The cost of a strategy is the total number of guesses it uses over all
answers. For a set of possible answers `S`, the best cost is the minimum over
all guesses `g` of `|S|` plus the best costs of the groups that `g` splits `S`
into. The solver evaluates this with a depth-first branch-and-bound search
(`src/solver.rs`):

- **Upper bound first.** A quick greedy strategy gives an initial cost to
  beat, so most guesses can be discarded early.
- **Lower bounds.** `heuristic::capacity_bound` gives the smallest possible
  cost for `n` answers when no guess can split them into more than `k` groups.
  A guess whose lower bound already reaches the best known cost is skipped.
- **Equivalent guesses.** Two guesses that split the remaining answers into
  the same groups have the same cost, so only one of them is searched.
- **Transposition table.** Results for answer sets that have already been
  solved are stored in a shared table, so a set reached by different paths is
  solved only once (`src/cache.rs`).
- **Parallel search.** The first guesses are evaluated in parallel on all
  cores with `rayon`. The threads share the best cost found so far, so each
  thread can skip guesses that another thread has already beaten.

[`ARCHITECTURE.md`](ARCHITECTURE.md) describes each of these in more detail,
along with how solve time grows with the number of answers.

## Testing

`cargo test --release` runs the test suite. Use `--release`, because the tests
run the real solver on real word lists. The main checks are:

- **Golden tests** in `src/solver.rs` fix the optimal cost for several subsets
  of the bundled lists. A change that alters one of these numbers is a bug
  until shown otherwise.
- **Tests in `tests/`**, one file per kind of check. They share helpers in
  `tests/common/`, which include their own implementation of the Wordle
  response rules, their own tree checker and their own brute-force solver, so
  a bug in the solver's code does not also affect the check.

| file | what it checks |
|---|---|
| `brute_force.rs` | Small word lists: the solver, `src/naive.rs` and the brute-force solver in `tests/common/` must find the same optimum. |
| `subtrees.rs` | Every part of an optimal tree must itself be optimal. Small parts of the full bundled tree and of other solved lists are checked by brute force, larger parts by solving them again from scratch. |
| `alternative_guesses.rs` | At every step of the tree, no other guess may lead to a lower total cost. |
| `metamorphic.rs` | Renaming letters, reordering letter positions or adding unused guesses must not change the optimum. Adding guesses must not raise it, and removing an answer must lower it. |
| `planted_optima.rs` | Lists of up to about 240 answers built so that the optimum is known in advance. |
| `published_optima.rs` | The published lists in `words/published/` must reach the published optima. |
| `full_scale.rs` | Rebuilds the full bundled tree and runs the checks above on it. |
| `parallel_consistency.rs` | Repeated multi-threaded solves of one list must always reach the single-threaded optimum, whatever the thread timing. |
| `cli.rs` | The binary reads word lists from files and writes and validates a tree. |

`published_optima.rs` and `full_scale.rs` are skipped by default because each
test solves full-size lists, which takes from seconds to minutes. Run them with:

```bash
cargo test --release --test published_optima --test full_scale -- --ignored --nocapture
```

`parallel_consistency.rs` is skipped by default too, because it solves the
same list a few hundred times (about a minute and a half). Run it with:

```bash
cargo test --release --test parallel_consistency -- --ignored --nocapture
```

CI (`.github/workflows/rust.yml`) runs the tests, `cargo clippy --release --
-D warnings`, `cargo fmt -- --check`, and the viewer's tests
(`node site/policy-core.test.js`).
