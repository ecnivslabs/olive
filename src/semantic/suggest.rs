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

/// Best suggestion for `target` among `candidates`, or `None` if nothing is
/// close enough. The threshold scales with the typo length: short names tolerate
/// one edit, longer names up to a third of their length.
pub fn closest<'a, I>(target: &str, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let max = (target.chars().count() / 3).clamp(1, 3);
    let mut best: Option<(usize, &str)> = None;
    for cand in candidates {
        if cand == target || cand.starts_with("__olive_") {
            continue;
        }
        let d = edit_distance(target, cand, max);
        if d <= max && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, cand));
            if d == 1 {
                break;
            }
        }
    }
    best.map(|(_, name)| name.to_string())
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
}
