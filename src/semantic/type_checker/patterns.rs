use super::super::error::SemanticError;
use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{CompClause, ForTarget, MatchPattern};
use crate::span::Span;

impl TypeChecker {
    pub(super) fn bind_for_target(&mut self, target: &ForTarget, iter_ty: &Type, span: Span) {
        let mut resolved = self.apply_subst(iter_ty.clone());
        // Iterating a borrow (`for x in &xs`) keeps the element types of the
        // underlying collection, so look through the reference.
        while let Type::Ref(inner) | Type::MutRef(inner) = resolved {
            resolved = self.apply_subst(*inner);
        }
        let elem_ty = match resolved {
            Type::List(inner) => *inner,
            Type::Set(inner) => *inner,
            Type::Dict(k, _) => *k,
            Type::Str => Type::Str,
            Type::Tuple(elems) => {
                if elems.is_empty() {
                    Type::Any
                } else {
                    let common = self.fresh_var();
                    for e in &elems {
                        self.unify(&common, e, span);
                    }
                    self.apply_subst(common)
                }
            }
            _ => self.fresh_var(),
        };

        match target {
            ForTarget::Name(name, _) => {
                self.define_type(name, elem_ty, true);
            }
            ForTarget::Tuple(names) => match self.apply_subst(elem_ty) {
                Type::Tuple(elems) if elems.len() == names.len() => {
                    for ((name, _), ty) in names.iter().zip(elems) {
                        self.define_type(name, ty, true);
                    }
                }
                _ => {
                    for (name, _) in names {
                        let var = self.fresh_var();
                        self.define_type(name, var, false);
                    }
                }
            },
        }
    }

    pub(super) fn check_comp_clauses(&mut self, clauses: &[CompClause], span: Span) {
        for clause in clauses {
            let iter_ty = self.check_for_iter(&clause.iter);
            self.bind_for_target(&clause.target, &iter_ty, span);
            if let Some(cond) = &clause.condition {
                let cond_ty = self.check_expr(cond);
                self.expect_truthy(&cond_ty, cond, cond.span);
            }
        }
    }

    pub(super) fn check_pattern(&mut self, pattern: &MatchPattern, match_ty: &Type, span: Span) {
        match pattern {
            MatchPattern::Wildcard => {}
            MatchPattern::Identifier(name, _) => {
                self.define_type(name, match_ty.clone(), false);
            }
            MatchPattern::Variant(v_name, inner_patterns) => {
                let resolved_enum = match match_ty {
                    Type::Enum(name, _) => Some(name.clone()),
                    Type::Union(members) => members.iter().find_map(|ty| {
                        if let Type::Enum(en, _) = ty {
                            let mangled = format!("{}::{}", en, v_name);
                            if self.lookup_type(&mangled).is_some() {
                                Some(en.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };

                if let Some(enum_name) = resolved_enum {
                    let variant_mangled = format!("{}::{}", enum_name, v_name);
                    if let Some(Type::Fn(param_types, _, _)) = self.lookup_type(&variant_mangled) {
                        if param_types.len() == inner_patterns.len() {
                            for (p, p_ty) in inner_patterns.iter().zip(param_types) {
                                self.check_pattern(p, &p_ty, span);
                            }
                        } else {
                            self.errors.push(SemanticError::rich(
                                crate::compile::errors::Diagnostic::error(
                                    "E0418",
                                    format!("wrong number of fields for variant `{v_name}`"),
                                    span,
                                )
                                .label(format!(
                                    "expected {} field(s), found {}",
                                    param_types.len(),
                                    inner_patterns.len()
                                ))
                                .help(format!(
                                    "the pattern must bind exactly {} field(s)",
                                    param_types.len()
                                )),
                            ));
                        }
                    } else {
                        let suggestions = self
                            .enum_variants
                            .get(&enum_name)
                            .map(|variants| {
                                super::super::suggest::closest_n(
                                    v_name,
                                    variants.iter().map(String::as_str),
                                    3,
                                )
                                .into_iter()
                                .map(|v| format!("{enum_name}::{v}"))
                                .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        self.errors.push(SemanticError::UndefinedName {
                            name: variant_mangled,
                            span,
                            suggestions,
                            can_autofix: false,
                        });
                    }
                } else {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0419",
                            "this type cannot be destructured by a variant pattern",
                            span,
                        )
                        .label(format!("`{match_ty}` is not an enum or union"))
                        .note("variant patterns only apply to `enum` and union (`A | B`) values"),
                    ));
                }
            }
            MatchPattern::Literal(expr) => {
                let expr_ty = self.check_expr(expr);
                self.unify(match_ty, &expr_ty, span);
            }
            MatchPattern::Tuple(items) => match match_ty {
                Type::Tuple(field_tys) if field_tys.len() == items.len() => {
                    for (item, field_ty) in items.iter().zip(field_tys) {
                        self.check_pattern(item, field_ty, span);
                    }
                }
                Type::Tuple(field_tys) => {
                    self.push_shape_mismatch(
                        span,
                        format!(
                            "expected a {}-element tuple pattern, found {}",
                            field_tys.len(),
                            items.len()
                        ),
                        "the pattern must bind exactly as many elements as the tuple has",
                    );
                }
                _ => {
                    self.push_shape_mismatch(
                        span,
                        format!("`{match_ty}` is not a tuple"),
                        "tuple patterns only apply to tuple-typed values",
                    );
                }
            },
            MatchPattern::StructFields(name, fields, pat_span) => match match_ty {
                Type::Struct(struct_name, ..) if struct_name == name => {
                    for (fname, fpat) in fields {
                        match self
                            .field_types
                            .get(&(struct_name.clone(), fname.clone()))
                            .cloned()
                        {
                            Some(fty) => self.check_pattern(fpat, &fty, *pat_span),
                            None => self.push_shape_mismatch(
                                *pat_span,
                                format!("`{struct_name}` has no field `{fname}`"),
                                "check the field name against the struct definition",
                            ),
                        }
                    }
                }
                Type::Struct(struct_name, ..) => {
                    self.push_shape_mismatch(
                        *pat_span,
                        format!("expected `{struct_name}`, found a pattern for `{name}`"),
                        "match the scrutinee's own struct name",
                    );
                }
                _ => {
                    self.push_shape_mismatch(
                        *pat_span,
                        format!("`{match_ty}` is not a struct"),
                        "struct-field patterns only apply to struct-typed values",
                    );
                }
            },
            MatchPattern::List {
                before,
                rest,
                after,
            } => match match_ty {
                Type::List(elem_ty) => {
                    for p in before.iter().chain(after) {
                        self.check_pattern(p, elem_ty, span);
                    }
                    if let Some((name, _)) = rest {
                        self.define_type(name, Type::List(elem_ty.clone()), false);
                    }
                }
                _ => {
                    self.push_shape_mismatch(
                        span,
                        format!("`{match_ty}` is not a list"),
                        "list patterns only apply to list-typed values",
                    );
                }
            },
            MatchPattern::Range(start, end, _) => {
                let start_ty = self.check_expr(start);
                let end_ty = self.check_expr(end);
                self.unify(match_ty, &start_ty, span);
                self.unify(match_ty, &end_ty, span);
            }
            MatchPattern::Or(alts) => {
                let first_names = pattern_binding_names(&alts[0]);
                if let Some(bad) = alts[1..]
                    .iter()
                    .find(|alt| pattern_binding_names(alt) != first_names)
                {
                    self.errors.push(SemanticError::rich(
                        crate::compile::errors::Diagnostic::error(
                            "E0436",
                            "or-pattern alternatives must bind the same names",
                            span,
                        )
                        .label(format!(
                            "this alternative binds {:?}, another binds {:?}",
                            pattern_binding_names(bad),
                            first_names
                        ))
                        .help("give every alternative the same set of bindings"),
                    ));
                }
                for alt in alts {
                    self.check_pattern(alt, match_ty, span);
                }
            }
        }
    }

    /// A pattern shape that doesn't match its scrutinee's type at all
    /// (wrong arity, wrong struct name, wrong container kind) -- one code
    /// covers every "this pattern cannot apply here" case rather than a
    /// nearly-identical one per pattern form.
    fn push_shape_mismatch(&mut self, span: Span, label: impl Into<String>, help: &str) {
        self.errors.push(SemanticError::rich(
            crate::compile::errors::Diagnostic::error(
                "E0437",
                "pattern does not match the scrutinee's type",
                span,
            )
            .label(label.into())
            .help(help),
        ));
    }
}

/// Every name a pattern binds, for the or-pattern consistency check --
/// purely structural (no type lookups), since only the *names* matter
/// there and this must not touch checker state.
fn pattern_binding_names(pattern: &MatchPattern) -> Vec<String> {
    let mut names = Vec::new();
    collect_binding_names(pattern, &mut names);
    names.sort();
    names
}

fn collect_binding_names(pattern: &MatchPattern, out: &mut Vec<String>) {
    match pattern {
        MatchPattern::Identifier(name, _) => out.push(name.clone()),
        MatchPattern::Variant(_, inner) => {
            for p in inner {
                collect_binding_names(p, out);
            }
        }
        MatchPattern::Tuple(items) | MatchPattern::Or(items) => {
            for p in items {
                collect_binding_names(p, out);
            }
        }
        MatchPattern::StructFields(_, fields, _) => {
            for (_, p) in fields {
                collect_binding_names(p, out);
            }
        }
        MatchPattern::List {
            before,
            rest,
            after,
        } => {
            for p in before.iter().chain(after) {
                collect_binding_names(p, out);
            }
            if let Some((name, _)) = rest {
                out.push(name.clone());
            }
        }
        MatchPattern::Wildcard | MatchPattern::Literal(_) | MatchPattern::Range(..) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::TypeChecker;
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

    #[test]
    fn wildcard_pattern_ok() {
        let tc = typeck(
            "enum E:\n    A\n    B\n\nfn f(x: E):\n    match x:\n        case A:\n            pass\n        case _:\n            pass\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn identifier_pattern_binds() {
        let tc =
            typeck("enum E:\n    A\nlet x = A\nmatch x:\n    case other:\n        let y = other\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn unknown_variant_suggests_nearest() {
        let tc = typeck(
            "enum Color:\n    Red\n    Green\n    Blue\n\nfn f(c: Color):\n    match c:\n        case Gren:\n            pass\n        case _:\n            pass\n",
        );
        let suggestions = tc.errors.iter().find_map(|e| match e {
            crate::semantic::SemanticError::UndefinedName {
                name, suggestions, ..
            } if name == "Color::Gren" => Some(suggestions.clone()),
            _ => None,
        });
        assert_eq!(suggestions, Some(vec!["Color::Green".to_string()]));
    }

    #[test]
    fn unknown_variant_error_points_at_case_arm() {
        let src = "enum Color:\n    Red\n    Green\n\nfn f(c: Color):\n    match c:\n        case Gren:\n            pass\n        case _:\n            pass\n";
        let tc = typeck(src);
        let span = tc.errors.iter().find_map(|e| match e {
            crate::semantic::SemanticError::UndefinedName { name, span, .. }
                if name == "Color::Gren" =>
            {
                Some(*span)
            }
            _ => None,
        });
        // The `case Gren:` arm is on line 7, not the `match c:` line (6).
        assert_eq!(span.map(|s| s.line), Some(7));
    }

    #[test]
    fn variant_pattern_with_data() {
        let tc = typeck(
            "enum Opt:\n    Some(i64)\n    Nil\nlet x = Some(42)\nmatch x:\n    case Some(v):\n        let y = v\n    case Nil:\n        pass\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn nested_variant_pattern() {
        let tc = typeck(
            "enum A:\n    B(i64, str)\nlet x = B(1, \"a\")\nmatch x:\n    case B(a, b):\n        let y = a + 1\n",
        );
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn literal_pattern() {
        let tc =
            typeck("let x = 42\nmatch x:\n    case 0:\n        pass\n    case _:\n        pass\n");
        assert!(tc.errors.is_empty());
    }

    #[test]
    fn non_exhaustive_match_reported() {
        let tc = typeck(
            "enum C:\n    Red\n    Green\n    Blue\nlet x = Red\nmatch x:\n    case Red:\n        pass\n    case Green:\n        pass\n",
        );
        assert!(!tc.errors.is_empty());
    }

    /// E12.3: the E12.1 grammar-only forms now type-check for real, one
    /// case per form (mirrors E12.1's parser round-trip tests and E12.2's
    /// rejected-for-now tests, both now superseded by real acceptance).
    #[test]
    fn tuple_pattern_checks() {
        let tc = typeck(
            "let pair = (1, 2)\nmatch pair:\n    case (0, 0):\n        pass\n    case (a, b):\n        let y = a + b\n",
        );
        assert!(tc.errors.is_empty(), "{:?}", codes(&tc));
    }

    #[test]
    fn tuple_pattern_wrong_arity() {
        let tc = typeck(
            "let pair = (1, 2)\nmatch pair:\n    case (a, b, c):\n        pass\n    case _:\n        pass\n",
        );
        assert!(codes(&tc).contains(&"E0437".to_string()));
    }

    #[test]
    fn struct_named_pattern_checks() {
        let tc = typeck(
            "struct P:\n    x: i64\n    y: i64\nlet p = P(1, 2)\nmatch p:\n    case P(x=0, y=n):\n        let z = n\n    case _:\n        pass\n",
        );
        assert!(tc.errors.is_empty(), "{:?}", codes(&tc));
    }

    #[test]
    fn struct_pattern_unknown_field() {
        let tc = typeck(
            "struct P:\n    x: i64\nlet p = P(1)\nmatch p:\n    case P(z=0):\n        pass\n    case _:\n        pass\n",
        );
        assert!(codes(&tc).contains(&"E0437".to_string()));
    }

    #[test]
    fn list_pattern_checks() {
        let tc = typeck(
            "let xs = [1, 2, 3]\nmatch xs:\n    case []:\n        pass\n    case [first, *rest]:\n        let y = first\n",
        );
        assert!(tc.errors.is_empty(), "{:?}", codes(&tc));
    }

    #[test]
    fn range_pattern_checks() {
        let tc = typeck(
            "let n = 5\nmatch n:\n    case 0..10:\n        pass\n    case _:\n        pass\n",
        );
        assert!(tc.errors.is_empty(), "{:?}", codes(&tc));
    }

    #[test]
    fn range_pattern_wrong_type() {
        let tc = typeck(
            "let s = \"hi\"\nmatch s:\n    case 0..10:\n        pass\n    case _:\n        pass\n",
        );
        assert!(!tc.errors.is_empty());
    }

    #[test]
    fn or_pattern_checks() {
        let tc = typeck(
            "let s = \"GET\"\nmatch s:\n    case \"GET\" | \"HEAD\":\n        pass\n    case _:\n        pass\n",
        );
        assert!(tc.errors.is_empty(), "{:?}", codes(&tc));
    }

    #[test]
    fn or_pattern_inconsistent_bindings() {
        let tc = typeck(
            "enum R:\n    Ok(i64)\n    Err(str)\nlet x = Ok(1)\nmatch x:\n    case Ok(v) | Err(e):\n        pass\n    case _:\n        pass\n",
        );
        assert!(codes(&tc).contains(&"E0436".to_string()));
    }

    #[test]
    fn or_pattern_consistent_bindings() {
        let tc = typeck(
            "enum R:\n    A(i64)\n    B(i64)\nlet x = A(1)\nmatch x:\n    case A(v) | B(v):\n        let y = v\n    case _:\n        pass\n",
        );
        assert!(tc.errors.is_empty(), "{:?}", codes(&tc));
    }

    fn codes(tc: &TypeChecker) -> Vec<String> {
        tc.errors
            .iter()
            .filter_map(|e| e.to_diagnostic().code().map(str::to_string))
            .collect()
    }
}
