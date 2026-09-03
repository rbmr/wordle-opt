pub mod core;
pub mod dict;
pub mod matrix;

use crate::dict::Dictionary;
use crate::matrix::ResponseMatrix;
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
}
