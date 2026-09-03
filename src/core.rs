#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Word(pub [u8; 5]);

impl Word {
    pub fn new(s: &str) -> Self {
        assert_eq!(s.len(), 5);
        let mut bytes = [0; 5];
        bytes.copy_from_slice(s.as_bytes());
        Word(bytes)
    }

    pub fn to_string(&self) -> String {
        String::from_utf8(self.0.to_vec()).unwrap()
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
#[path = "core_test.rs"]
mod tests;
