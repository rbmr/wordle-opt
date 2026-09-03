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
    matrix: &ResponseMatrix,
    candidates: &[usize],
    guesses: &mut [usize],
) {
    guesses.sort_unstable_by_key(|&g| compute_expected_remaining(matrix, candidates, g));
}
