pub mod core;
pub mod dict;
pub mod matrix;
pub mod heuristic;
pub mod solver;

use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
use crate::solver::Solver;
use std::time::Instant;

fn main() {
    println!("Loading dictionary...");
    let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
    println!("Loaded {} guesses and {} candidates.", dict.guesses.len(), dict.candidates.len());
    
    println!("Computing response matrix...");
    let start = Instant::now();
    let matrix = ResponseMatrix::new(&dict);
    let duration = start.elapsed();
    println!("Computed matrix of size {}x{} in {:?}", matrix.num_guesses, matrix.num_candidates, duration);

    let subset_size = 100; // Let's try 100 now
    let initial_candidates: Vec<usize> = (0..subset_size).collect();
    println!("Solving for {} candidates...", subset_size);
    let mut solver = Solver::new(&matrix);
    
    let start = Instant::now();
    let cost = solver.solve(&initial_candidates);
    let duration = start.elapsed();
    
    println!("Total cost: {}, Expected guesses: {:.4}", cost, cost as f64 / subset_size as f64);
    println!("Solved in {:?}", duration);
    println!("Cache size: {}", solver.cache.len());
}
