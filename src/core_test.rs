#![allow(clippy::needless_range_loop)]
#[cfg(test)]
mod tests {
    use crate::core::{Response, Word};

    #[test]
    fn test_compute_response_greens() {
        let secret = Word::new("aback");
        let guess = Word::new("abaci");
        let r = Response::compute(&secret, &guess);
        // greens: 1
        assert_eq!(r.0, 1 + 3 + 9 + 27);
    }

    #[test]
    fn test_compute_response_yellows() {
        let secret = Word::new("abcde");
        let guess = Word::new("eabcd");
        let r = Response::compute(&secret, &guess);
        // all yellows: 2
        assert_eq!(r.0, 2 + 6 + 18 + 54 + 162);
    }

    #[test]
    fn test_compute_response_duplicates() {
        // secret has one 'a', guess has two 'a's
        let secret = Word::new("water");
        let guess = Word::new("alpha");
        let r = Response::compute(&secret, &guess);
        // guess[0] 'a' is yellow -> 2
        // guess[1] 'l' is black -> 0
        // guess[2] 'p' is black -> 0
        // guess[3] 'h' is black -> 0
        // guess[4] 'a' is black (only one 'a' in secret) -> 0
        assert_eq!(r.0, 2);

        // secret has two 'a's, guess has two 'a's
        let secret2 = Word::new("abaca");
        let guess2 = Word::new("alpha");
        let r2 = Response::compute(&secret2, &guess2);
        // alpha against abaca
        // 'a' green -> 1
        // 'l' black -> 0
        // 'p' black -> 0
        // 'h' black -> 0
        // 'a' green -> 1
        assert_eq!(r2.0, 1 + 81);

        // Priority of green over yellow
        let secret3 = Word::new("abaca"); // 'a' at 0, 2, 4
        let guess3 = Word::new("arena"); // 'a' at 0, 4, 'e', 'r', 'n'
        let r3 = Response::compute(&secret3, &guess3);
        // arena against abaca
        // a at 0 -> green
        // a at 4 -> green
        // r, e, n -> black
        assert_eq!(r3.0, 1 + 81);
    }

    #[test]
    fn test_compute_response_all_black() {
        let secret = Word::new("apple");
        let guess = Word::new("ghost");
        let r = Response::compute(&secret, &guess);
        // ghost against apple -> all black -> 0
        assert_eq!(r.0, 0);
    }
}
