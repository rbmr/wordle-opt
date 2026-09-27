#![allow(clippy::needless_range_loop)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Word(pub [u8; 5]);

impl Word {
    pub fn new(s: &str) -> Self {
        assert_eq!(s.len(), 5);
        let mut bytes = [0; 5];
        bytes.copy_from_slice(s.as_bytes());
        Word(bytes)
    }
}

impl std::fmt::Display for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", std::str::from_utf8(&self.0).unwrap())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Response(pub u8);

impl Response {
    pub const WIN: Response = Response::new(1, 1, 1, 1, 1);

    // b = 0, g = 1, y = 2
    pub const fn new(r0: u8, r1: u8, r2: u8, r3: u8, r4: u8) -> Self {
        Response(r0 + r1 * 3 + r2 * 9 + r3 * 27 + r4 * 81)
    }

    pub fn compute(secret: &Word, guess: &Word) -> Self {
        let mut r = [0u8; 5]; // 0 = black
        let mut used = [false; 5];

        // Pass 1: find greens
        for i in 0..5 {
            if guess.0[i] == secret.0[i] {
                r[i] = 1; // green
                used[i] = true;
            }
        }

        // Pass 2: find yellows
        for i in 0..5 {
            if r[i] == 1 {
                continue;
            }
            for j in 0..5 {
                if guess.0[i] == secret.0[j] && !used[j] {
                    r[i] = 2; // yellow
                    used[j] = true;
                    break;
                }
            }
        }

        Response::new(r[0], r[1], r[2], r[3], r[4])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_win() {
        let w = Word::new("crane");
        assert_eq!(Response::compute(&w, &w), Response::WIN);
    }

    #[test]
    fn test_response_all_black() {
        let secret = Word::new("crane");
        let guess = Word::new("stomp");
        let r = Response::compute(&secret, &guess);
        assert_eq!(r, Response::new(0, 0, 0, 0, 0));
    }

    #[test]
    fn test_response_yellow() {
        // secret=abcde, guess=eabcd: all yellows
        let secret = Word::new("abcde");
        let guess = Word::new("eabcd");
        let r = Response::compute(&secret, &guess);
        // e->yellow, a->yellow, b->yellow, c->yellow, d->yellow
        assert_eq!(r, Response::new(2, 2, 2, 2, 2));
    }

    #[test]
    fn test_response_mixed() {
        // crane vs crate: c/r/a match position, e matches position, t doesn't appear.
        let secret = Word::new("crane");
        let guess = Word::new("crate");
        let r = Response::compute(&secret, &guess);
        assert_eq!(r, Response::new(1, 1, 1, 0, 1)); // c G, r G, a G, t B, e G
    }

    #[test]
    fn test_response_duplicate_letter_capped_by_green() {
        // secret has exactly one 'b' (at position 1). guess repeats 'b' at
        // positions 0 and 1: the green match at position 1 must consume the
        // secret's only 'b', so the position-0 'b' has nothing left to match
        // and must be black, not yellow. This is the classic Wordle
        // duplicate-letter rule and the main way a naive per-letter-count
        // implementation goes wrong (double-counting yellows for a letter
        // that only appears once in the secret).
        let secret = Word::new("abcde");
        let guess = Word::new("bbfff");
        let r = Response::compute(&secret, &guess);
        assert_eq!(r, Response::new(0, 1, 0, 0, 0));
    }

    #[test]
    fn test_response_duplicate_letter_only_first_occurrence_yellow() {
        // secret has exactly one 'x' (position 1), never guessed at that
        // position. guess has two 'x's, at positions 0 and 2 - neither
        // green. Only the secret's single 'x' is available to match, so
        // the first guess occurrence (position 0) should come back yellow
        // and the second (position 2) black, not both yellow.
        let secret = Word::new("mxcde");
        let guess = Word::new("xqxrs");
        let r = Response::compute(&secret, &guess);
        assert_eq!(r, Response::new(2, 0, 0, 0, 0));
    }
}

#[cfg(test)]
mod extra_core_tests {
    #![allow(unused_imports)]
    use super::*;

    #[test]
    fn test_all_responses_symmetric() {
        let g = &Word(*b"tests");
        let c = &Word(*b"tests");
        assert_eq!(Response::compute(g, c).0, 121); // 3^5 - 1
    }

    #[test]
    fn test_invalid_characters_do_not_panic() {
        // Technically solver expects a-z, but let's ensure it doesn't crash on uppercase if they slip through
        let g = &Word(*b"HELLO");
        let c = &Word(*b"WORLD");
        let score = Response::compute(g, c).0;
        assert!(score < 243);
    }
}
