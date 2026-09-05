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
        // secret=aabbc, guess=abcde: a=green@0, b=yellow@1(present@0 but used), wait...
        // Let's use a cleaner case.
        // secret=crane, guess=crate: c=G,r=G,a=G,t=B,e=Y (e in crane, position 4 not match pos 4 which is 'e' in crane wait)
        // crane: c=0,r=1,a=2,n=3,e=4
        // crate: c=0,r=1,a=2,t=3,e=4 -> c=G,r=G,a=G,t=B(no t in crane),e=G
        let secret = Word::new("crane");
        let guess = Word::new("crate");
        let r = Response::compute(&secret, &guess);
        // c G, r G, a G, t B, e G
        assert_eq!(r, Response::new(1, 1, 1, 0, 1));
    }
}
