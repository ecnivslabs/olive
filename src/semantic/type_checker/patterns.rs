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
                    let common = Type::new_var();
                    for e in &elems {
                        self.unify(&common, e, span);
                    }
                    self.apply_subst(common)
                }
            }
            _ => Type::new_var(),
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
                        self.define_type(name, Type::new_var(), false);
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
