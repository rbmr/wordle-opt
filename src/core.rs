#![allow(clippy::needless_range_loop)]
#[derive(Clone, Copy)]
/// Represents a word in the dictionary (up to 15 letters).
pub struct Word {
    pub len: u8,
    pub bytes: [u8; 15],
}

impl Word {
    pub fn new(s: &str) -> Self {
        let b = s.as_bytes();
        assert!(b.len() <= 15, "word {:?} exceeds 15 bytes", s);
        let mut bytes = [0u8; 15];
        bytes[..b.len()].copy_from_slice(b);
        Word {
            len: b.len() as u8,
            bytes,
        }
    }

    #[inline]
    pub fn from_slice(b: &[u8]) -> Self {
        assert!(b.len() <= 15, "byte slice exceeds 15 bytes");
        let mut bytes = [0u8; 15];
        bytes[..b.len()].copy_from_slice(b);
        Word {
            len: b.len() as u8,
            bytes,
        }
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(self.as_bytes()).unwrap()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> From<[u8; N]> for Word {
    #[inline]
    fn from(arr: [u8; N]) -> Self {
        Word::from_slice(&arr)
    }
}

impl<const N: usize> From<&[u8; N]> for Word {
    #[inline]
    fn from(arr: &[u8; N]) -> Self {
        Word::from_slice(arr)
    }
}

impl From<&str> for Word {
    #[inline]
    fn from(s: &str) -> Self {
        Word::new(s)
    }
}

impl std::ops::Deref for Word {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

impl AsRef<[u8]> for Word {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl std::fmt::Display for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::fmt::Debug for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Word({:?})", self.as_str())
    }
}

impl PartialEq for Word {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}
impl Eq for Word {}

impl PartialOrd for Word {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Word {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl std::hash::Hash for Word {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
/// Represents the feedback from a guess against a secret word, encoded as a base-3 integer.
///
/// 0 = Black (letter not in secret)
/// 1 = Green (letter in correct position)
/// 2 = Yellow (letter in wrong position)
pub struct Response(pub u16);

impl Response {
    pub const WIN: Response = Response::new(1, 1, 1, 1, 1);

    pub const fn win_for_len(len: usize) -> Response {
        let mut val = 0u16;
        let mut mul = 1u16;
        let mut i = 0;
        while i < len {
            val += mul;
            mul *= 3;
            i += 1;
        }
        Response(val)
    }

    // b = 0, g = 1, y = 2
    pub const fn new(r0: u8, r1: u8, r2: u8, r3: u8, r4: u8) -> Self {
        Response(
            r0 as u16 + (r1 as u16) * 3 + (r2 as u16) * 9 + (r3 as u16) * 27 + (r4 as u16) * 81,
        )
    }
}

pub trait ResponseElem:
    Copy + Default + Send + Sync + 'static + PartialEq + Eq + PartialOrd + Ord
{
    fn to_usize(self) -> usize;
    fn from_usize(val: usize) -> Self;
}

impl ResponseElem for u8 {
    #[inline(always)]
    fn to_usize(self) -> usize {
        self as usize
    }
    #[inline(always)]
    fn from_usize(val: usize) -> Self {
        val as u8
    }
}

impl ResponseElem for Response {
    #[inline(always)]
    fn to_usize(self) -> usize {
        self.0 as usize
    }
    #[inline(always)]
    fn from_usize(val: usize) -> Self {
        Response(val as u16)
    }
}

impl std::fmt::Display for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut val = self.0;
        let mut s = String::with_capacity(5);
        for _ in 0..5 {
            let c = match val % 3 {
                0 => 'B',
                1 => 'G',
                2 => 'Y',
                _ => unreachable!(),
            };
            s.push(c);
            val /= 3;
        }
        write!(f, "{}", s)
    }
}

impl Response {
    pub fn compute(secret: &Word, guess: &Word) -> Self {
        let s_bytes = secret.as_bytes();
        let g_bytes = guess.as_bytes();
        let n = s_bytes.len();
        debug_assert_eq!(
            n,
            g_bytes.len(),
            "secret and guess must have the same length"
        );

        let mut r = [0u8; 16];
        let mut used = [false; 16];

        // Pass 1: find greens
        for i in 0..n {
            if g_bytes[i] == s_bytes[i] {
                r[i] = 1; // green
                used[i] = true;
            }
        }

        // Pass 2: find yellows
        for i in 0..n {
            if r[i] == 1 {
                continue;
            }
            for j in 0..n {
                if g_bytes[i] == s_bytes[j] && !used[j] {
                    r[i] = 2; // yellow
                    used[j] = true;
                    break;
                }
            }
        }

        let mut val = 0u16;
        let mut mul = 1u16;
        for i in 0..n {
            val += r[i] as u16 * mul;
            mul *= 3;
        }

        Response(val)
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
        let g = &Word::from(*b"tests");
        let c = &Word::from(*b"tests");
        assert_eq!(Response::compute(g, c).0, 121); // 3^5 - 1
    }

    #[test]
    fn test_invalid_characters_do_not_panic() {
        // Technically solver expects a-z, but let's ensure it doesn't crash on uppercase if they slip through
        let g = &Word::from(*b"HELLO");
        let c = &Word::from(*b"WORLD");
        let score = Response::compute(g, c).0;
        assert!(score < 243);
    }

    #[test]
    fn test_response_display() {
        assert_eq!(Response::WIN.to_string(), "GGGGG");
        assert_eq!(Response::new(0, 0, 0, 0, 0).to_string(), "BBBBB");
        assert_eq!(Response::new(0, 1, 2, 0, 1).to_string(), "BGYBG");
    }

    #[test]
    fn test_response_different_lengths() {
        let w3 = Word::new("bad");
        assert_eq!(Response::compute(&w3, &w3), Response::win_for_len(3));
        assert_eq!(Response::win_for_len(3).0, 13); // 1 + 3 + 9

        let w4 = Word::new("able");
        assert_eq!(Response::compute(&w4, &w4), Response::win_for_len(4));
        assert_eq!(Response::win_for_len(4).0, 40); // 1 + 3 + 9 + 27
    }
}
