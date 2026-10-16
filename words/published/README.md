# Word lists with published optima

These are the word lists Alex Selby used for his exact Wordle results, copied
unchanged from [alex1770/wordle](https://github.com/alex1770/wordle) at commit
`8fd3f1bdf884ff6757c24ad1227dc561fab7787d` (MIT licensed, see
`LICENSE-selby`), with `.txt` appended to each name. They are here because each
pair has an optimum that was computed independently, by a different program,
so `wordle-opt` can be checked against it at full scale.

`*_hidden` is the candidate (answer) list and `*_all` the allowed-guess list.
The guess list for `nyt20220830` and `nyt20230701` is Selby's `wordlist_all`,
which is word-for-word the bundled `words/guesses.txt`, so it is not
duplicated here.

| lists | candidates | guesses | published optimum | first guess | Selby's source file |
|---|---:|---:|---:|---|---|
| `orig` | 2,315 | 12,972 | 7,920 | salet | `normal.top105.proven` |
| `nyt20220215` | 2,309 | 12,947 | 7,897 | salet | `results_easy_nyt20220215` |
| `nyt20220316` | 2,309 | 12,972 | 7,897 | salet | `results_easy_nyt20220316` |
| `nyt20220830` | 2,309 | 14,855 (`words/guesses.txt`) | 7,883 | tarse | `results_easy_nyt20220830` |
| `nyt20230701` | 3,158 | 14,855 (`words/guesses.txt`) | 11,219 | tarse | `results_easy_nyt20230701` |

The optimum is the total number of guesses over all candidates, in normal (not
hard) mode, which is the cost `wordle-opt` reports. The `orig` result
(average 3.4212 with SALET) was also reproduced independently by Bertsimas and
Paskov, "An Exact Solution to Wordle", *Operations Research* 73(3), 2025.

The bundled `words/candidates.txt` (2,340 words) is the `nyt20220830` hidden
list plus 31 further words, so its optimum (8,001) has no published counterpart.

`tests/optimality_audit.rs` re-solves these instances and requires the
published optimum (`published_optima`, ignored by default because each is a
full-scale solve):

```bash
cargo test --release --test optimality_audit published_optima -- --ignored --nocapture
```
