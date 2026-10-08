use criterion::{Criterion, black_box, criterion_group, criterion_main};
use wordle_opt::dict::Dictionary;
use wordle_opt::matrix::ResponseMatrix;
use wordle_opt::solver::{Metrics, Solver};

fn criterion_benchmark(c: &mut Criterion) {
    let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
    let matrix = ResponseMatrix::new(&dict);
    let subset: Vec<usize> = (0..50).collect();

    c.bench_function("greedy_solve_n50", |b| {
        b.iter(|| Solver::greedy_solve(black_box(&matrix), black_box(&dict), black_box(&subset)))
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches, benches_n100, benches_solve);

fn criterion_benchmark_n100(c: &mut Criterion) {
    let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
    let matrix = ResponseMatrix::new(&dict);
    let subset: Vec<usize> = (0..100).collect();

    c.bench_function("greedy_solve_n100", |b| {
        b.iter(|| Solver::greedy_solve(black_box(&matrix), black_box(&dict), black_box(&subset)))
    });
}

criterion_group!(benches_n100, criterion_benchmark_n100);
// Update main

fn criterion_benchmark_solve_n100(c: &mut Criterion) {
    let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
    let matrix = ResponseMatrix::new(&dict);
    let subset: Vec<usize> = (0..100).collect();
    let metrics = wordle_opt::solver::Metrics::new();
    let equiv_cache: [_; 64] =
        std::array::from_fn(|_| std::sync::RwLock::new(rustc_hash::FxHashMap::default()));

    c.bench_function("solve_n100", |b| {
        b.iter(|| {
            wordle_opt::solver::Solver::solve(
                black_box(&matrix),
                black_box(&subset),
                black_box(&dict),
                black_box(&metrics),
                black_box(&equiv_cache),
            )
        })
    });
}

criterion_group!(benches_solve, criterion_benchmark_solve_n100);
