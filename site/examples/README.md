# Example policy trees

Three example policy trees for the viewer, all built on the **same**
500-candidate subset so their strategies are directly comparable:

| file | strategy | nodes | max depth | total cost | mean guesses |
|------|----------|------:|----------:|-----------:|-------------:|
| `optimal.json` | `optimal` | 524 | 3 | 1449 | 2.898 |
| `min-remaining.json` | `min-remaining` | 594 | 3 | 1524 | 3.048 |
| `max-freq.json` | `max-freq` | 615 | 4 | 1677 | 3.354 |

All three carry the same dictionary hash `0x1db379dcaf20c72a`.

## How they were generated

The candidate subset is a reproducible random draw of 500 candidates from
`words/candidates.txt`, using the same seeded sampler as the benchmarks
(`--sample-seed 20261008`). The full 14,855-word guess list is used, so the
strategy may guess any valid word.

```bash
for s in optimal min-remaining max-freq; do
  cargo run --release -- solve \
    --strategy "$s" \
    --max-candidates 500 \
    --sample-seed 20261008 \
    --format readable \
    --output "site/examples/$s.json" \
    --no-progress
done
```

`--format readable` embeds the guess and candidate lists, so each file is
self-contained: the viewer (and `wordle-opt validate <file>`) can check it with
no other files present. The `optimal` example takes ~35 s to build; the two
heuristics are near-instant.

Each `.json` also has a sibling `.js` (`window.WordleExamples["<name>"] = ...`)
which is what the viewer's example buttons load. It is generated from the
`.json` and exists so the examples work when `site/index.html` is opened
directly from disk, where `fetch()` of a sibling file is blocked by the
browser:

```bash
node -e 'const fs=require("fs");const n=process.argv[1];const j=fs.readFileSync(n+".json","utf8").trim();
fs.writeFileSync(n+".js","window.WordleExamples=window.WordleExamples||{};window.WordleExamples[\""+n+"\"]="+j+";\n")' optimal
```

The optimal example's total cost (1449) was independently cross-checked against
the existing exact solver on the identical subset
(`wordle-opt diagnose -n 500 -s 20261008` → cost 1449).

## Scaling up

These are deliberately small. A tree for the full 2340-candidate set is a much
larger computation - the optimal one requires an exact solve of every reachable
state - and is intended to be generated on the compute host:

```bash
cargo run --release -- solve --strategy optimal --output optimal-2340.json
```

The viewer and the compact/readable formats handle trees of any size; the
browser renderer expands lazily.
