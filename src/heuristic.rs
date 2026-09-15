#![allow(clippy::needless_range_loop)]
use crate::matrix::ResponseMatrix;

pub fn compute_expected_remaining(
    matrix: &ResponseMatrix,
    candidates: &[usize],
    guess: usize,
) -> u32 {
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

pub fn compute_max_branching_factor(
    matrix: &crate::matrix::ResponseMatrix,
    candidates: &[usize],
) -> usize {
    use rayon::prelude::*;

    (0..matrix.num_guesses)
        .into_par_iter()
        .map(|g| {
            let mut seen = [false; 243];
            let mut k = 0;
            for &c in candidates {
                let r = matrix.get(g, c);
                if !seen[r.0 as usize] {
                    seen[r.0 as usize] = true;
                    k += 1;
                }
            }
            k
        })
        .max()
        .unwrap_or(0)
}

/// Computes the absolute minimum total cost to solve a subset of size `n`
/// assuming a maximum branching factor `k`.
///
/// This mathematically models the exact capacity of a uniform tree with degree `k-1`
/// (since 1 branch is reserved for the 'WIN' response).
pub fn capacity_bound(n: usize, k: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    if n == 1 {
        return 1;
    }
    if n == 2 {
        return 3;
    }

    let mut remaining = n as u32;
    let mut cost = 0;
    let mut depth = 1;
    if k <= 1 {
        panic!("Infinite loop: n={}, k={}", n, k);
    }
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

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn test_capacity_bound_scaling() {
        // Test edge cases
        assert_eq!(capacity_bound(0, 100), 0);
        assert_eq!(capacity_bound(1, 100), 1);
        assert_eq!(capacity_bound(2, 100), 3);

        // Test tight branching (k=3)
        // n=7: 1 at depth 1, 2 at depth 2 (cost 4), 4 at depth 3 (cost 12) -> total 17
        assert_eq!(capacity_bound(7, 3), 1 + 4 + 12);

        // Test wide branching (k=100) for n=20
        // 1 at depth 1, 19 at depth 2 -> 1 + 38 = 39
        assert_eq!(capacity_bound(20, 100), 39);
    }
}

#[cfg(test)]
mod expected_remaining_tests {
    use super::*;
    use crate::dict::Dictionary;

    #[test]
    fn test_compute_expected_remaining() {
        // Create a fake dictionary and matrix for testing expected remaining logic
        let words = vec![
            crate::core::Word::new("abcde"),
            crate::core::Word::new("abcdf"),
            crate::core::Word::new("xyzab"),
        ];
        let dict = Dictionary {
            guesses: words.clone(),
            candidates: words.clone(),
            guess_chars: vec![[0, 1, 2, 3, 4], [0, 1, 2, 3, 5], [23, 24, 25, 0, 1]],
            candidate_to_guess: vec![0, 1, 2],
        };
        let matrix = ResponseMatrix::new(&dict);
        let set = vec![0, 1, 2];

        // Guess 0 (abcde) against candidate 0 (abcde) is WIN
        // Guess 0 against candidate 1 (abcdf) is 4 greens, 1 black
        // Guess 0 against candidate 2 (xyzab) is 2 yellows (ab), 3 blacks
        // So the 3 candidates fall into 3 distinct buckets.
        // sum(1^2 + 1^2 + 1^2) = 3
        let exp0 = compute_expected_remaining(&matrix, &set, 0);
        assert_eq!(exp0, 3);
    }
}

pub fn tight_capacity_bound(n: usize, root_k: usize, max_k: usize) -> u32 {
    if n == 0 {
        return 0;
    }
    if n == 1 {
        return 1;
    }
    if n == 2 {
        return 3;
    }

    let mut remaining = n as u32;
    let mut cost = 0;
    let mut depth = 1;
    if root_k <= 1 || max_k <= 1 {
        // If max_k <= 1 or root_k <= 1 and we have n > 2, it's impossible to solve
        // but we return a large finite cost to let alpha-beta prune it.
        return capacity_bound(n, max_k);
    }

    let take = remaining.min(1);
    cost += take * depth;
    remaining -= take;
    depth += 1;

    let mut capacity_at_depth = (root_k as u32).saturating_sub(1);

    while remaining > 0 {
        let take = remaining.min(capacity_at_depth);
        cost += take * depth;
        remaining -= take;
        depth += 1;
        capacity_at_depth = capacity_at_depth.saturating_mul(max_k as u32 - 1);
    }
    cost
}
