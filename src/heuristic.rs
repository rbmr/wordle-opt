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

/// Computes the absolute minimum total cost to solve a subset of size `n`
/// assuming a maximum branching factor `k`.
///
/// This mathematically models the exact capacity of a uniform tree with degree `k-1`
/// (since 1 branch is reserved for the 'WIN' response).
pub fn capacity_bound(n: usize, k: usize) -> u32 {
    if n == 0 { return 0; }
    if n == 1 { return 1; }
    if n == 2 { return 3; }
    
    let mut remaining = n as u32;
    let mut cost = 0;
    let mut depth = 1;
    let mut capacity_at_depth = 1u32;
    
    while remaining > 0 {
        let take = remaining.min(capacity_at_depth);
        cost += take * depth;
        remaining -= take;
        depth += 1;
        capacity_at_depth = capacity_at_depth.saturating_mul(k as u32 - 1);
    }
    cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capacity_bound() {
        assert_eq!(capacity_bound(1, 3), 1);
        assert_eq!(capacity_bound(2, 3), 3); // 1 win at depth 1, 1 at depth 2 (cost 1 + 2)
        assert_eq!(capacity_bound(3, 3), 5); // 1 at depth 1, 2 at depth 2 (cost 1 + 2 + 2)
        assert_eq!(capacity_bound(4, 3), 8); // 1 at depth 1, 2 at depth 2, 1 at depth 3 (cost 1+4+3)
    }
}
