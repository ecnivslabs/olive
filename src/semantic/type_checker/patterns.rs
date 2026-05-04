use super::super::error::SemanticError;
use super::super::types::Type;
use super::TypeChecker;
use crate::parser::{CompClause, ForTarget, MatchPattern};
use crate::span::Span;

impl TypeChecker {
    pub(super) fn bind_for_target(&mut self, target: &ForTarget, iter_ty: &Type, span: Span) {
        let resolved = self.apply_subst(iter_ty.clone());
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
            let iter_ty = self.check_expr(&clause.iter);
            self.bind_for_target(&clause.target, &iter_ty, span);
            if let Some(cond) = &clause.condition {
                self.check_expr(cond);
            }
        }
    }

    pub(super) fn check_pattern(&mut self, pattern: &MatchPattern, match_ty: &Type, span: Span) {
        match pattern {
            MatchPattern::Wildcard => {}
            MatchPattern::Identifier(name) => {
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
                            self.errors.push(SemanticError::Custom {
                                msg: format!(
                                    "expected {} arguments for variant {}, found {}",
                                    param_types.len(),
                                    v_name,
                                    inner_patterns.len()
                                ),
                                span,
                            });
                        }
                    } else {
                        self.errors.push(SemanticError::UndefinedName {
                            name: variant_mangled,
                            span,
                        });
                    }
                } else {
                    self.errors.push(SemanticError::Custom {
                        msg: format!("expected Enum or Union type, found {}", match_ty),
                        span,
                    });
                }
            }
            MatchPattern::Literal(expr) => {
                let expr_ty = self.check_expr(expr);
                self.unify(match_ty, &expr_ty, span);
            }
        }
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
    fn variant_pattern_with_data() {
        let tc = typeck(
            "enum Opt:\n    Some(i64)\n    None\nlet x = Some(42)\nmatch x:\n    case Some(v):\n        let y = v\n    case None:\n        pass\n",
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
}
