#[cfg(test)]
mod tests {
    use crate::core::{Word, Response};

    #[test]
    fn test_compute_response() {
        let secret = Word::new("aback");
        let guess = Word::new("abaci");
        let r = Response::compute(&secret, &guess);
        // a=green, b=green, a=green, c=green, i=black
        // greens = 1, black = 0
        // r0=1, r1=1, r2=1, r3=1, r4=0
        // response = 1 + 3 + 9 + 27 + 0 = 40
        assert_eq!(r.0, 40);

        let secret2 = Word::new("abate");
        let guess2 = Word::new("abase");
        let r2 = Response::compute(&secret2, &guess2);
        // a=green, b=green, a=green, s=black, e=green
        // r0=1, r1=1, r2=1, r3=0, r4=1
        // response = 1 + 3 + 9 + 0 + 81 = 94
        assert_eq!(r2.0, 94);
    }
}
