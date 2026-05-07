//! Nearest-name suggestions for unresolved identifiers. Picks the candidate
//! with the smallest Damerau-Levenshtein distance to the typo, within a
//! length-scaled threshold so unrelated names never get suggested.

/// Optimal string alignment distance (Levenshtein plus adjacent transpositions),
/// computed with two rolling rows. Early-exits once the best achievable distance
/// on a row exceeds `max`, so a rejected candidate costs almost nothing.
fn edit_distance(a: &str, b: &str, max: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return max + 1;
    }
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev_prev = vec![0usize; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];

    for i in 1..=a.len() {
        curr[0] = i;
        let mut row_best = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(prev_prev[j - 2] + 1);
            }
            curr[j] = d;
            row_best = row_best.min(d);
        }
        if row_best > max {
            return max + 1;
        }
        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Up to `n` suggestions for `target`, ordered nearest first, together with a
/// flag that is true when the nearest one is an unambiguous winner: either the
/// sole candidate, or strictly closer than the runner-up. The flag gates the
/// machine-applicable autofix. An ambiguous match is shown but never rewritten.
///
/// The threshold scales with the typo length: short names tolerate one edit,
/// longer names up to a third of their length. Ties break lexicographically so
/// the output is deterministic regardless of how `candidates` is iterated.
pub fn ranked<'a, I>(target: &str, candidates: I, n: usize) -> (Vec<String>, bool)
where
    I: IntoIterator<Item = &'a str>,
{
    let max = (target.chars().count() / 3).clamp(1, 3);
    let mut hits: Vec<(usize, &str)> = Vec::new();
    for cand in candidates {
        if cand == target || cand.starts_with("__olive_") {
            continue;
        }
        let d = edit_distance(target, cand, max);
        if d <= max {
            hits.push((d, cand));
        }
    }
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    let unambiguous = match hits.as_slice() {
        [] => false,
        [_] => true,
        [a, b, ..] => a.0 < b.0,
    };
    let names = hits
        .into_iter()
        .take(n)
        .map(|(_, s)| s.to_string())
        .collect();
    (names, unambiguous)
}

/// Up to `n` nearest suggestions for `target`, ordered nearest first. Thin
/// projection of [`ranked`] for callers that only display alternatives.
pub fn closest_n<'a, I>(target: &str, candidates: I, n: usize) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    ranked(target, candidates, n).0
}

/// Best single suggestion for `target`, or `None` if nothing is close enough.
pub fn closest<'a, I>(target: &str, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    closest_n(target, candidates, 1).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_distance() {
        assert_eq!(edit_distance("kitten", "sitting", 10), 3);
        assert_eq!(edit_distance("flaw", "lawn", 10), 2);
        assert_eq!(edit_distance("abc", "abc", 10), 0);
    }

    #[test]
    fn transposition_is_one_edit() {
        assert_eq!(edit_distance("ab", "ba", 10), 1);
        assert_eq!(edit_distance("total", "totla", 10), 1);
    }

    #[test]
    fn early_exit_caps_at_max_plus_one() {
        assert_eq!(edit_distance("abcdef", "uvwxyz", 1), 2);
    }

    #[test]
    fn suggests_near_miss() {
        let names = ["total", "count", "index"];
        assert_eq!(closest("totl", names), Some("total".to_string()));
        assert_eq!(closest("totla", names), Some("total".to_string()));
    }

    #[test]
    fn rejects_far_names() {
        let names = ["total", "count"];
        assert_eq!(closest("xyzzy", names), None);
    }

    #[test]
    fn ignores_compiler_internal_names() {
        let names = ["__olive_panic", "value"];
        assert_eq!(closest("__olive_panci", names), None);
    }

    #[test]
    fn picks_closest_of_several() {
        let names = ["alpha", "beta", "gamma"];
        assert_eq!(closest("alpa", names), Some("alpha".to_string()));
    }

    #[test]
    fn closest_n_orders_by_distance() {
        // input order is reversed from the expected ranking on purpose.
        let names = ["collar", "color"];
        assert_eq!(closest_n("colour", names, 3), vec!["color", "collar"]);
    }

    #[test]
    fn closest_n_caps_at_n() {
        let names = ["numbed", "numbere", "numbers"];
        assert_eq!(closest_n("number", names, 2).len(), 2);
    }

    #[test]
    fn ranked_flags_strict_winner() {
        // `colour` is one edit from `color`, two from `collar`: a clear winner.
        let names = ["color", "collar"];
        let (got, unambiguous) = ranked("colour", names, 3);
        assert_eq!(got, vec!["color", "collar"]);
        assert!(unambiguous);
    }

    #[test]
    fn ranked_flags_tie_as_ambiguous() {
        let names = ["bar", "baz"];
        let (got, unambiguous) = ranked("bat", names, 3);
        assert_eq!(got, vec!["bar", "baz"]);
        assert!(!unambiguous);
    }

    #[test]
    fn ranked_empty_is_ambiguous_false() {
        let names = ["alpha", "beta"];
        let (got, unambiguous) = ranked("xyzzy", names, 3);
        assert!(got.is_empty());
        assert!(!unambiguous);
    }
}
