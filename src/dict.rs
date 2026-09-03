use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use crate::core::Word;

/// Represents a loaded Wordle dictionary containing valid guesses and possible secret candidates.
/// Both vectors are deduplicated and sorted to enable deterministic subset caching.
pub struct Dictionary {
    pub guesses: Vec<Word>,
    pub candidates: Vec<Word>,
}

impl Dictionary {
    pub fn load<P1: AsRef<Path>, P2: AsRef<Path>>(guesses_path: P1, candidates_path: P2) -> Self {
        let mut guesses = Self::read_words(guesses_path);
        let mut candidates = Self::read_words(candidates_path);

        guesses.sort();
        guesses.dedup();
        candidates.sort();
        candidates.dedup();

        // Ensure all candidates are in guesses
        for c in &candidates {
            if !guesses.contains(c) {
                guesses.push(c.clone());
            }
        }
        guesses.sort();
        guesses.dedup();

        Self {
            guesses,
            candidates,
        }
    }

    fn read_words<P: AsRef<Path>>(path: P) -> Vec<Word> {
        let file = File::open(path).expect("failed to open file");
        let reader = BufReader::new(file);
        let mut words = Vec::new();

        for line in reader.lines() {
            let line = line.unwrap();
            let word_str = line.trim().to_lowercase();
            if word_str.len() == 5 {
                words.push(Word::new(&word_str));
            }
        }
        words
    }
}
