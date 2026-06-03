//! E12.2: extends E0414 past the pre-existing enum/union-variant coverage
//! check (`expr.rs`'s `Match` arm) to the E12.1 pattern forms. The existing
//! checker's shape is a flat variant-name set, which cannot express
//! recursion into fields the way a proper decision-tree usefulness
//! algorithm (Maranget's, as Rust uses) would. Rather than build that in
//! full here, int/range and list-length coverage are checked exactly (via
//! interval-union arithmetic, the one place where "exactly" is cheap and
//! the payoff is real: a match over a bounded range genuinely can be
//! exhaustive without a wildcard). Tuple/struct-field coverage uses a
//! conservative, sound approximation instead of full cross-product
//! specialization: it accepts only when some single arm's fields are, on
//! their own, already fully wildcard/recursively-exhaustive. That rejects
//! a few patterns a complete algorithm would accept (e.g. `(True, _)` and
//! `(False, _)` jointly covering `(bool, bool)` without either alone doing
//! so), but never accepts a genuinely incomplete match -- soundness over
//! completeness, and cheap to state precisely in one place instead of
//! scattered across every call site.

use super::TypeChecker;
use crate::parser::ast::{Expr, ExprKind, MatchPattern, UnaryOp};
use crate::semantic::types::Type;

/// What's left uncovered, in a form the caller can put straight into a
/// diagnostic label.
pub(super) struct Gap {
    pub label: String,
    pub help: String,
}

fn literal_int_value(e: &Expr) -> Option<i64> {
    match &e.kind {
        ExprKind::Integer(n) => Some(*n),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => literal_int_value(operand).map(|n| -n),
        _ => None,
    }
}

/// Merges (possibly overlapping/adjacent) inclusive intervals and reports
/// the first gap in `[full_lo, full_hi]` left uncovered, if any.
fn first_gap(mut intervals: Vec<(i64, i64)>, full_lo: i64, full_hi: i64) -> Option<(i64, i64)> {
    intervals.sort_by_key(|iv| iv.0);
    let mut cursor = full_lo;
    for &(lo, hi) in &intervals {
        if lo > cursor {
            return Some((cursor, lo - 1));
        }
        if hi >= cursor {
            // `hi == i64::MAX` means nothing is left to cover.
            cursor = hi.checked_add(1)?;
        }
        if cursor > full_hi {
            return None;
        }
    }
    (cursor <= full_hi).then_some((cursor, full_hi))
}

fn int_interval(pattern: &MatchPattern) -> Option<(i64, i64)> {
    match pattern {
        MatchPattern::Literal(e) => literal_int_value(e).map(|v| (v, v)),
        MatchPattern::Range(start, end, inclusive) => {
            let lo = literal_int_value(start)?;
            let hi = literal_int_value(end)?;
            Some((lo, if *inclusive { hi } else { hi - 1 }))
        }
        _ => None,
    }
}

/// A list pattern's length coverage: `(min_len, max_len)`, `i64::MAX`
/// standing in for "or more" when a `*rest` slot is present.
fn list_length_interval(pattern: &MatchPattern) -> Option<(i64, i64)> {
    match pattern {
        MatchPattern::List {
            before,
            rest,
            after,
        } => {
            let fixed = (before.len() + after.len()) as i64;
            Some((fixed, if rest.is_some() { i64::MAX } else { fixed }))
        }
        _ => None,
    }
}

/// Every name a pattern binds, treated for this check as "matches
/// anything at this position" -- an `Identifier`/`Wildcard` sub-pattern
/// contributes nothing further to prove.
fn is_trivially_exhaustive(pattern: &MatchPattern) -> bool {
    matches!(
        pattern,
        MatchPattern::Wildcard | MatchPattern::Identifier(..)
    )
}

impl TypeChecker {
    /// `None` when exhaustive (or when `ty`/`patterns` aren't a shape this
    /// pass covers -- the caller's pre-existing enum/union logic still
    /// applies unconditionally). `patterns` already excludes guarded arms;
    /// a guard may not fire even when its pattern matches, so a guarded
    /// arm never contributes to exhaustiveness.
    pub(super) fn check_new_form_exhaustiveness(
        &self,
        ty: &Type,
        patterns: &[&MatchPattern],
    ) -> Option<Gap> {
        match ty {
            Type::Int
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::Usize => {
                let intervals: Vec<(i64, i64)> =
                    patterns.iter().filter_map(|p| int_interval(p)).collect();
                let (lo, hi) = first_gap(intervals, i64::MIN, i64::MAX)?;
                let range = if lo == hi {
                    lo.to_string()
                } else {
                    format!("{lo}..={hi}")
                };
                Some(Gap {
                    label: format!("{range} is not covered"),
                    help: "add a range/literal arm covering it, or a wildcard `case _:` arm"
                        .to_string(),
                })
            }
            Type::List(_) => {
                let intervals: Vec<(i64, i64)> = patterns
                    .iter()
                    .filter_map(|p| list_length_interval(p))
                    .collect();
                if intervals.is_empty() {
                    return None;
                }
                let (lo, hi) = first_gap(intervals, 0, i64::MAX)?;
                let label = if hi == i64::MAX {
                    format!("lists of length {lo} or more are not covered")
                } else if lo == hi {
                    format!("lists of length {lo} are not covered")
                } else {
                    format!("lists of length {lo}..={hi} are not covered")
                };
                Some(Gap {
                    label,
                    help: "add a `[*rest]`-shaped arm, or a wildcard `case _:` arm".to_string(),
                })
            }
            Type::Tuple(field_tys) => {
                let has_tuple_pattern =
                    patterns.iter().any(|p| matches!(p, MatchPattern::Tuple(_)));
                if !has_tuple_pattern {
                    return None;
                }
                let covers = patterns.iter().any(|p| match p {
                    MatchPattern::Tuple(items) if items.len() == field_tys.len() => items
                        .iter()
                        .zip(field_tys)
                        .all(|(item, field_ty)| self.field_fully_covered(item, field_ty)),
                    _ => is_trivially_exhaustive(p),
                });
                if covers {
                    None
                } else {
                    Some(Gap {
                        label: "not every field combination is covered".to_string(),
                        help: "add a wildcard `case _:` arm, or bind every field in one arm"
                            .to_string(),
                    })
                }
            }
            Type::Struct(struct_name, _, _) => {
                let has_struct_pattern = patterns.iter().any(
                    |p| matches!(p, MatchPattern::StructFields(name, ..) if name == struct_name),
                );
                if !has_struct_pattern {
                    return None;
                }
                let covers = patterns.iter().any(|p| match p {
                    MatchPattern::StructFields(name, fields, _) if name == struct_name => {
                        fields.iter().all(|(fname, fpat)| {
                            self.field_types
                                .get(&(struct_name.clone(), fname.clone()))
                                .is_some_and(|fty| self.field_fully_covered(fpat, fty))
                        })
                    }
                    _ => is_trivially_exhaustive(p),
                });
                if covers {
                    None
                } else {
                    Some(Gap {
                        label: "not every field combination is covered".to_string(),
                        help: "add a wildcard `case _:` arm, or bind every field in one arm"
                            .to_string(),
                    })
                }
            }
            _ => None,
        }
    }

    /// Whether a single sub-pattern, alone, already proves its field fully
    /// covered -- the "diagonal" check `check_new_form_exhaustiveness`
    /// uses for tuple/struct fields instead of real cross-arm
    /// specialization (see the module doc for why).
    fn field_fully_covered(&self, pattern: &MatchPattern, field_ty: &Type) -> bool {
        if is_trivially_exhaustive(pattern) {
            return true;
        }
        self.check_new_form_exhaustiveness(field_ty, &[pattern])
            .is_none()
            && !matches!(pattern, MatchPattern::Literal(_) | MatchPattern::Range(..))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semantic::Resolver;

    fn typeck(src: &str) -> TypeChecker {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        tc
    }

    fn has_e0414(tc: &TypeChecker) -> bool {
        tc.errors
            .iter()
            .any(|e| e.to_diagnostic().code() == Some("E0414"))
    }

    #[test]
    fn int_interval_exclusive_range() {
        let iv = int_interval(&MatchPattern::Range(
            Expr::new(ExprKind::Integer(0), Default::default()),
            Expr::new(ExprKind::Integer(10), Default::default()),
            false,
        ));
        assert_eq!(iv, Some((0, 9)));
    }

    #[test]
    fn int_interval_inclusive_range() {
        let iv = int_interval(&MatchPattern::Range(
            Expr::new(ExprKind::Integer(200), Default::default()),
            Expr::new(ExprKind::Integer(299), Default::default()),
            true,
        ));
        assert_eq!(iv, Some((200, 299)));
    }

    #[test]
    fn first_gap_detects_missing_low_end() {
        let gap = first_gap(vec![(0, 10)], i64::MIN, i64::MAX);
        assert_eq!(gap, Some((i64::MIN, -1)));
    }

    #[test]
    fn first_gap_none_when_fully_covered() {
        let gap = first_gap(vec![(0, 5), (6, 10)], 0, 10);
        assert_eq!(gap, None);
    }

    #[test]
    fn first_gap_detects_middle_hole() {
        let gap = first_gap(vec![(0, 5), (8, 10)], 0, 10);
        assert_eq!(gap, Some((6, 7)));
    }

    #[test]
    fn identifier_catchall_satisfies_enum_exhaustiveness() {
        // Regression test for the bug this phase found: a bare binding arm
        // (not `_`) is a catch-all too.
        let tc = typeck(
            "enum E:\n    A\n    B\n    C\n\nfn f(x: E) -> str:\n    match x:\n        A:\n            return \"a\"\n        n:\n            return \"other\"\n",
        );
        assert!(
            !has_e0414(&tc),
            "errors: {:?}",
            tc.errors
                .iter()
                .map(|e| e.to_diagnostic().code().map(str::to_string))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn list_length_gap_below_rest_threshold() {
        let intervals = vec![
            list_length_interval(&MatchPattern::List {
                before: vec![MatchPattern::Wildcard],
                rest: Some(("rest".to_string(), Default::default())),
                after: vec![],
            })
            .unwrap(),
        ];
        // `[first, *rest]` only covers length >= 1; length 0 is a gap.
        assert_eq!(first_gap(intervals, 0, i64::MAX), Some((0, 0)));
    }

    #[test]
    fn bare_rest_pattern_covers_every_length() {
        let intervals = vec![
            list_length_interval(&MatchPattern::List {
                before: vec![],
                rest: Some(("rest".to_string(), Default::default())),
                after: vec![],
            })
            .unwrap(),
        ];
        assert_eq!(first_gap(intervals, 0, i64::MAX), None);
    }
}
