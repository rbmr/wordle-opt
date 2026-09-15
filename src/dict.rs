#![allow(clippy::needless_range_loop)]
use crate::core::Word;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Represents a loaded Wordle dictionary containing valid guesses and possible secret candidates.
/// Both vectors are deduplicated and sorted to enable deterministic subset caching.
pub struct Dictionary {
    pub guesses: Vec<Word>,
    pub guess_chars: Vec<[u8; 5]>,
    pub candidates: Vec<Word>,
    pub candidate_to_guess: Vec<usize>,
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
                guesses.push(*c);
            }
        }
        guesses.sort();
        guesses.dedup();

        let mut guess_chars = Vec::with_capacity(guesses.len());
        for g in &guesses {
            let mut chars = [0u8; 5];
            for i in 0..5 {
                chars[i] = g.0[i] - b'a';
            }
            guess_chars.push(chars);
        }

        let mut candidate_to_guess = Vec::with_capacity(candidates.len());
        for c in &candidates {
            candidate_to_guess.push(guesses.binary_search(c).unwrap());
        }

        Self {
            guesses,
            guess_chars,
            candidates,
            candidate_to_guess,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_dictionary_loading_and_dedup() {
        let mut guesses_file = NamedTempFile::new().unwrap();
        let mut candidates_file = NamedTempFile::new().unwrap();

        writeln!(guesses_file, "apple\nberry\npeach\nberry").unwrap();
        writeln!(candidates_file, "apple\nmaple\n").unwrap();

        let dict = Dictionary::load(guesses_file.path(), candidates_file.path());

        // Candidates should be exactly apple, maple
        assert_eq!(dict.candidates.len(), 2);
        assert_eq!(dict.candidates[0].to_string(), "apple");
        assert_eq!(dict.candidates[1].to_string(), "maple");

        // Guesses should be apple, berry, peach, AND maple (since candidates must be in guesses),
        // sorted and deduplicated.
        assert_eq!(dict.guesses.len(), 4);
        assert_eq!(dict.guesses[0].to_string(), "apple");
        assert_eq!(dict.guesses[1].to_string(), "berry");
        assert_eq!(dict.guesses[2].to_string(), "maple");
        assert_eq!(dict.guesses[3].to_string(), "peach");

        // Ensure guess_chars was populated correctly
        assert_eq!(dict.guess_chars.len(), 4);
        assert_eq!(dict.guess_chars[0], [0, 15, 15, 11, 4]); // apple
    }
}
