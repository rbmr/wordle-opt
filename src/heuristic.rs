use crate::matrix::ResponseMatrix;

pub fn compute_expected_remaining(matrix: &ResponseMatrix, candidates: &[usize], guess: usize) -> u32 {
    let mut counts = [0u32; 243];
    for &c in candidates {
        let r = matrix.get(guess, c);
        counts[r.0 as usize] += 1;
    }
    
    let mut score = 0;
    for &count in &counts {
        score += count * count;
    }
    score
}

pub fn sort_guesses_by_expected_remaining(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
    guesses: &mut [usize],
) {
    guesses.sort_by_cached_key(|&g| compute_expected_remaining(matrix, candidates, g));
}

pub fn compute_max_branching_factor(matrix: &crate::matrix::ResponseMatrix, candidates: &[usize]) -> usize {
    let mut max_k = 0;
    for g in 0..matrix.num_guesses {
        let mut seen = [false; 243];
        let mut k = 0;
        for &c in candidates {
            let r = matrix.get(g, c);
            if !seen[r.0 as usize] {
                seen[r.0 as usize] = true;
                k += 1;
            }
        }
        if k > max_k {
            max_k = k;
        }
    }
    max_k
}
