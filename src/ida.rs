use crate::matrix::ResponseMatrix;
use crate::dict::Dictionary;
use crate::solver::Metrics;
use std::sync::atomic::Ordering;

pub fn solve_ida_star(
    matrix: &ResponseMatrix,
    initial_candidates: &[usize],
    dict: &Dictionary,
    metrics: &Metrics,
) -> u32 {
    let mut threshold = if initial_candidates.len() <= 2 {
        (initial_candidates.len() * (initial_candidates.len() + 1) / 2) as u32
    } else {
        initial_candidates.len() as u32
    };
    
    loop {
        let cost = search(matrix, initial_candidates, dict, metrics, threshold);
        if cost <= threshold {
            return cost;
        }
        threshold = cost;
    }
}

fn search(
    matrix: &ResponseMatrix,
    set: &[usize],
    dict: &Dictionary,
    metrics: &Metrics,
    threshold: u32,
) -> u32 {
    metrics.states_evaluated.fetch_add(1, Ordering::Relaxed);
    if set.len() <= 2 {
        return (set.len() * (set.len() + 1) / 2) as u32;
    }
    
    let mut min_cost = u32::MAX;
    
    for g in 0..dict.guesses.len() {
        let mut cost = set.len() as u32;
        let mut counts = [0u16; 243];
        let mut num_non_empty = 0;
        
        for &c in set {
            let r = matrix.get(g, c).0 as usize;
            if counts[r] == 0 {
                num_non_empty += 1;
            }
            counts[r] += 1;
        }
        
        if num_non_empty == 1 && !set.contains(&g) {
            continue;
        }
        
        for r_idx in 0..243 {
            if counts[r_idx] == 0 || r_idx == crate::core::Response::WIN.0 as usize {
                continue;
            }
            let mut subset = Vec::with_capacity(counts[r_idx] as usize);
            for &c in set {
                if matrix.get(g, c).0 as usize == r_idx {
                    subset.push(c);
                }
            }
            
            let sub_cost = search(matrix, &subset, dict, metrics, threshold.saturating_sub(cost));
            cost = cost.saturating_add(sub_cost);
            if cost > threshold {
                break;
            }
        }
        
        if cost < min_cost {
            min_cost = cost;
        }
        if min_cost <= threshold {
            return min_cost;
        }
    }
    
    min_cost
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::Dictionary;

    #[test]
    fn test_ida_star_base_case() {
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        // A single candidate should cost 1
        let cost = solve_ida_star(&matrix, &[0], &dict, &metrics);
        assert_eq!(cost, 1);
        
        // Two candidates should cost 3
        let cost = solve_ida_star(&matrix, &[0, 1], &dict, &metrics);
        assert_eq!(cost, 3);
    }
}

    #[test]
    fn test_ida_star_deeper_case() {
        let dict = Dictionary::load("words/guesses.txt", "words/candidates.txt");
        let matrix = ResponseMatrix::new(&dict);
        let metrics = Metrics::new();
        // A slightly larger test set for IDA*
        let set: Vec<usize> = (0..5).collect();
        let cost = solve_ida_star(&matrix, &set, &dict, &metrics);
        // Cost should be correctly bounded.
        assert!(cost > 0 && cost <= 100);
    }
