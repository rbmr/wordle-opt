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

pub const DEFAULT_GUESSES: &str = include_str!("../words/guesses.txt");
pub const DEFAULT_CANDIDATES: &str = include_str!("../words/candidates.txt");

impl Dictionary {
    /// Loads a wordle dictionary from two text files containing words separated by newlines.
    /// Returns a descriptive error if files cannot be opened or have invalid lines.
    pub fn try_load<P1: AsRef<Path>, P2: AsRef<Path>>(
        guesses_path: P1,
        candidates_path: P2,
    ) -> Result<Self, String> {
        let guesses = Self::read_words(guesses_path)?;
        let candidates = Self::read_words(candidates_path)?;
        Ok(Self::from_words(guesses, candidates))
    }

    /// Loads a wordle dictionary from two text files, falling back to the bundled
    /// default dictionary if the default file paths cannot be opened.
    pub fn load<P1: AsRef<Path>, P2: AsRef<Path>>(guesses_path: P1, candidates_path: P2) -> Self {
        let gp = guesses_path.as_ref();
        let cp = candidates_path.as_ref();
        match Self::try_load(gp, cp) {
            Ok(d) => d,
            Err(e) => {
                if gp == Path::new("words/guesses.txt") && cp == Path::new("words/candidates.txt") {
                    Self::load_default()
                } else {
                    panic!("failed to load dictionary: {e}");
                }
            }
        }
    }

    /// Loads the bundled default Wordle dictionary (14,855 guesses, 2,340 candidates)
    /// embedded directly inside the binary, allowing wordle-opt to run from any directory.
    pub fn load_default() -> Self {
        Self::from_strings(DEFAULT_GUESSES.lines(), DEFAULT_CANDIDATES.lines())
    }

    /// Builds a dictionary from string iterators, ignoring empty lines and normalizing to lowercase.
    pub fn from_strings<I1, I2, S1, S2>(guesses: I1, candidates: I2) -> Self
    where
        I1: IntoIterator<Item = S1>,
        I2: IntoIterator<Item = S2>,
        S1: AsRef<str>,
        S2: AsRef<str>,
    {
        let g: Vec<Word> = guesses
            .into_iter()
            .map(|s| s.as_ref().trim().to_lowercase())
            .filter(|s| s.len() == 5)
            .map(|s| Word::new(&s))
            .collect();
        let c: Vec<Word> = candidates
            .into_iter()
            .map(|s| s.as_ref().trim().to_lowercase())
            .filter(|s| s.len() == 5)
            .map(|s| Word::new(&s))
            .collect();
        Self::from_words(g, c)
    }

    /// Builds a dictionary from in-memory word lists, applying the same
    /// normalization as [`Dictionary::load`]: lowercase is assumed, both lists
    /// are sorted and deduplicated, and every candidate is guaranteed to also
    /// be present in the guess list. Used to reconstruct the dictionary
    /// embedded in a readable policy tree.
    pub fn from_words(mut guesses: Vec<Word>, mut candidates: Vec<Word>) -> Self {
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
                chars[i] = if g.0[i].is_ascii_lowercase() {
                    g.0[i] - b'a'
                } else {
                    g.0[i] % 26
                };
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

    /// Reads 5-letter words from a file, returning an error if the file cannot be opened
    /// or contains lines with unexpected word lengths.
    pub fn read_words<P: AsRef<Path>>(path: P) -> Result<Vec<Word>, String> {
        let p = path.as_ref();
        let file = File::open(p).map_err(|e| format!("cannot open {}: {e}", p.display()))?;
        let reader = BufReader::new(file);
        let mut words = Vec::new();

        for (line_no, line) in reader.lines().enumerate() {
            let line = line
                .map_err(|e| format!("cannot read {}: line {}: {e}", p.display(), line_no + 1))?;
            let word_str = line.trim().to_lowercase();
            if word_str.is_empty() {
                continue;
            }
            if word_str.len() != 5 {
                return Err(format!(
                    "inconsistent word length in {}: line {}: expected 5 letters, got {} in {:?}",
                    p.display(),
                    line_no + 1,
                    word_str.len(),
                    word_str
                ));
            }
            words.push(Word::new(&word_str));
        }
        if words.is_empty() {
            return Err(format!("word list in {} is empty", p.display()));
        }
        Ok(words)
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

    #[test]
    fn test_load_default() {
        let dict = Dictionary::load_default();
        assert_eq!(dict.guesses.len(), 14855);
        assert_eq!(dict.candidates.len(), 2340);
        assert_eq!(dict.guess_chars.len(), 14855);
        assert_eq!(dict.candidate_to_guess.len(), 2340);
    }

    #[test]
    fn test_try_load_missing_file() {
        let res = Dictionary::try_load("nonexistent_guesses.txt", "nonexistent_candidates.txt");
        assert!(res.is_err());
        let err = res.err().unwrap();
        assert!(err.contains("cannot open"));
    }

    #[test]
    fn test_try_load_invalid_word_length() {
        let mut guesses_file = NamedTempFile::new().unwrap();
        let mut candidates_file = NamedTempFile::new().unwrap();

        writeln!(guesses_file, "apple\ntoo_long_word\npeach").unwrap();
        writeln!(candidates_file, "apple").unwrap();

        let res = Dictionary::try_load(guesses_file.path(), candidates_file.path());
        assert!(res.is_err());
        let err = res.err().unwrap();
        assert!(err.contains("inconsistent word length"));
        assert!(err.contains("line 2"));
    }

    #[test]
    fn test_from_strings_normalization() {
        let dict =
            Dictionary::from_strings(["APPLE", "berry", "PEACH", "apple"], ["Apple", "maple"]);
        assert_eq!(dict.candidates.len(), 2);
        assert_eq!(dict.guesses.len(), 4);
        assert_eq!(dict.guesses[0].to_string(), "apple");
    }
}
