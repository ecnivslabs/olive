use super::MirBuilder;
use crate::parser::{Expr, ExprKind, Param, Stmt, StmtKind, TypeExpr};
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

impl<'a> MirBuilder<'a> {
    pub(super) fn resolve_type_expr(&self, expr: &TypeExpr) -> Type {
        use crate::parser::TypeExprKind;
        match &expr.kind {
            TypeExprKind::Qualified(_) => Type::PyObject,
            TypeExprKind::Name(name) => match name.as_str() {
                "int" | "i64" => Type::Int,
                "i32" => Type::I32,
                "i16" => Type::I16,
                "i8" => Type::I8,
                "u64" => Type::U64,
                "u32" => Type::U32,
                "u16" => Type::U16,
                "u8" => Type::U8,
                "float" | "f64" => Type::Float,
                "f32" => Type::F32,
                "str" => Type::Str,
                "bytes" => Type::Bytes,
                "bool" => Type::Bool,
                "None" => Type::Null,
                "Never" => Type::Never,
                "Any" => Type::Any,
                "PyObject" => Type::PyObject,
                _ => {
                    if let Some(Type::Enum(e, args)) = self.global_types.get(name) {
                        Type::Enum(e.clone(), args.clone())
                    } else {
                        Type::Struct(name.clone(), Vec::new())
                    }
                }
            },
            TypeExprKind::Generic(name, args) => match (name.as_str(), args.len()) {
                ("list", 1) => Type::List(Box::new(self.resolve_type_expr(&args[0]))),
                ("set", 1) => Type::Set(Box::new(self.resolve_type_expr(&args[0]))),
                ("dict", 2) => Type::Dict(
                    Box::new(self.resolve_type_expr(&args[0])),
                    Box::new(self.resolve_type_expr(&args[1])),
                ),
                _ => {
                    let resolved_args: Vec<Type> =
                        args.iter().map(|a| self.resolve_type_expr(a)).collect();
                    Type::Struct(name.clone(), resolved_args)
                }
            },
            TypeExprKind::List(inner) => Type::List(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::Dict(k, v) => Type::Dict(
                Box::new(self.resolve_type_expr(k)),
                Box::new(self.resolve_type_expr(v)),
            ),
            TypeExprKind::Tuple(types) => {
                Type::Tuple(types.iter().map(|t| self.resolve_type_expr(t)).collect())
            }
            TypeExprKind::Fn { params, ret } => Type::Fn(
                params.iter().map(|t| self.resolve_type_expr(t)).collect(),
                Box::new(self.resolve_type_expr(ret)),
                Vec::new(),
            ),
            TypeExprKind::Ref(inner) => Type::Ref(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::MutRef(inner) => Type::MutRef(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::Ptr(inner) => Type::Ptr(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::Union(a, b) => {
                let ta = self.resolve_type_expr(a);
                let tb = self.resolve_type_expr(b);
                let mut vars = Vec::new();
                if let Type::Union(mut va) = ta {
                    vars.append(&mut va);
                } else {
                    vars.push(ta);
                }
                if let Type::Union(mut vb) = tb {
                    vars.append(&mut vb);
                } else {
                    vars.push(tb);
                }
                Type::Union(vars)
            }
            TypeExprKind::FixedArray(_, _) => Type::List(Box::new(Type::Int)),
        }
    }

    pub(super) fn monomorphize(&mut self, name: &str, type_args: &[Type]) -> String {
        let generic_stmt = match self.generic_fns.get(name).cloned() {
            Some(s) => s,
            None => return name.to_string(),
        };

        let arg_str = type_args
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join("_")
            .replace("[", "_")
            .replace("]", "_")
            .replace(",", "_")
            .replace(" ", "")
            .replace("->", "_to_")
            .replace("(", "_")
            .replace(")", "_")
            .replace("&", "ref_")
            .replace("*", "ptr_")
            .replace("|", "_or_")
            .replace(":", "_");

        let mut specialized_name = format!("{}_{}", name, arg_str);
        if name.contains("::__init__") {
            let parts: Vec<&str> = name.split("::__init__").collect();
            specialized_name = format!("{}_{}::__init__", parts[0], arg_str);
        }

        if self.functions.iter().any(|f| f.name == specialized_name) {
            return specialized_name;
        }

        let mut specialized_stmt = generic_stmt.clone();
        let mut fn_type_map: Option<HashMap<String, Type>> = None;
        match &mut specialized_stmt.kind {
            StmtKind::Fn {
                name: n,
                type_params: tp,
                params: p,
                return_type: rt,
                body: b,
                ..
            } => {
                let tp_clone = tp.clone();
                *n = specialized_name.clone();
                *tp = Vec::new();

                let mut type_map = HashMap::default();
                for (param_name, arg_ty) in tp_clone.iter().zip(type_args.iter()) {
                    type_map.insert(param_name.clone(), arg_ty.clone());
                }

                self.replace_types_in_fn(p, rt, b, &type_map);
                fn_type_map = Some(type_map);
            }
            StmtKind::Struct {
                name: n,
                type_params: tp,
                fields: f,
                ..
            } => {
                let tp_clone = tp.clone();
                let mut type_map = HashMap::default();
                for (param_name, arg_ty) in tp_clone.iter().zip(type_args.iter()) {
                    type_map.insert(param_name.clone(), arg_ty.clone());
                }

                for field in f {
                    if let Some(ann) = &mut field.type_ann {
                        self.replace_type_expr(ann, &type_map);
                    }
                }

                *n = specialized_name.clone().replace("::__init__", "");
                *tp = Vec::new();
            }
            _ => {}
        }

        // Mirror the generic struct's field layout under the specialized name
        // (`Box_int`); without it, field access falls to the dynamic path and
        // derefs a raw struct pointer as a dict. Register before lowering the body.
        if let Some(base_struct) = name.strip_suffix("::__init__")
            && let Some(spec_struct) = specialized_name.strip_suffix("::__init__")
            && let Some(fields) = self.struct_fields.get(base_struct).cloned()
        {
            self.struct_fields
                .entry(spec_struct.to_string())
                .or_insert(fields);
        }

        // While lowering the specialized body, resolve type parameters read from
        // the original `expr_types` (e.g. a `Box(..)` construct inside the body)
        // to the concrete instance. Saved/restored to support nesting.
        let saved = std::mem::take(&mut self.mono_type_map);
        if let Some(map) = fn_type_map {
            self.mono_type_map = map;
        }
        self.lower_stmt(&specialized_stmt);
        self.mono_type_map = saved;
        specialized_name
    }

    pub(super) fn replace_types_in_fn(
        &self,
        params: &mut [Param],
        ret: &mut Option<TypeExpr>,
        body: &mut [Stmt],
        type_map: &HashMap<String, Type>,
    ) {
        for p in params {
            if let Some(ann) = &mut p.type_ann {
                self.replace_type_expr(ann, type_map);
            }
        }
        if let Some(ann) = ret {
            self.replace_type_expr(ann, type_map);
        }
        for s in body {
            self.replace_types_in_stmt(s, type_map);
        }
    }

    pub(super) fn replace_types_in_stmt(&self, stmt: &mut Stmt, type_map: &HashMap<String, Type>) {
        match &mut stmt.kind {
            StmtKind::Let {
                type_ann, value, ..
            } => {
                if let Some(ann) = type_ann {
                    self.replace_type_expr(ann, type_map);
                }
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::Const {
                type_ann, value, ..
            } => {
                if let Some(ann) = type_ann {
                    self.replace_type_expr(ann, type_map);
                }
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::ExprStmt(e) | StmtKind::Return(Some(e)) => {
                self.replace_types_in_expr(e, type_map)
            }
            StmtKind::Assign { target, value } => {
                self.replace_types_in_expr(target, type_map);
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::AugAssign { target, value, .. } => {
                self.replace_types_in_expr(target, type_map);
                self.replace_types_in_expr(value, type_map);
            }
            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                self.replace_types_in_expr(condition, type_map);
                for s in then_body {
                    self.replace_types_in_stmt(s, type_map);
                }
                for (c, b) in elif_clauses {
                    self.replace_types_in_expr(c, type_map);
                    for s in b {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
            }
            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                self.replace_types_in_expr(condition, type_map);
                for s in body {
                    self.replace_types_in_stmt(s, type_map);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
            }
            StmtKind::For {
                iter,
                body,
                else_body,
                ..
            } => {
                self.replace_types_in_expr(iter, type_map);
                for s in body {
                    self.replace_types_in_stmt(s, type_map);
                }
                if let Some(eb) = else_body {
                    for s in eb {
                        self.replace_types_in_stmt(s, type_map);
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    pub(super) fn replace_types_in_expr(&self, expr: &mut Expr, type_map: &HashMap<String, Type>) {
        match &mut expr.kind {
            ExprKind::BinOp { left, right, .. } => {
                self.replace_types_in_expr(left, type_map);
                self.replace_types_in_expr(right, type_map);
            }
            ExprKind::UnaryOp { operand, .. } => self.replace_types_in_expr(operand, type_map),
            ExprKind::Call { callee, args } => {
                self.replace_types_in_expr(callee, type_map);
                for arg in args {
                    match arg {
                        crate::parser::CallArg::Positional(e)
                        | crate::parser::CallArg::Keyword(_, e)
                        | crate::parser::CallArg::Splat(e)
                        | crate::parser::CallArg::KwSplat(e) => {
                            self.replace_types_in_expr(e, type_map)
                        }
                    }
                }
            }
            ExprKind::Index { obj, index } => {
                self.replace_types_in_expr(obj, type_map);
                self.replace_types_in_expr(index, type_map);
            }
            ExprKind::Attr { obj, .. } => self.replace_types_in_expr(obj, type_map),
            ExprKind::List(elems) | ExprKind::Tuple(elems) | ExprKind::Set(elems) => {
                for e in elems {
                    self.replace_types_in_expr(e, type_map);
                }
            }
            ExprKind::Dict(pairs) => {
                for (k, v) in pairs {
                    self.replace_types_in_expr(k, type_map);
                    self.replace_types_in_expr(v, type_map);
                }
            }
            _ => {}
        }
    }

    pub(super) fn replace_type_expr(&self, ann: &mut TypeExpr, type_map: &HashMap<String, Type>) {
        use crate::parser::TypeExprKind;
        match &mut ann.kind {
            TypeExprKind::Name(name) => {
                if let Some(ty) = type_map.get(name) {
                    ann.kind = self.type_to_type_expr_kind(ty);
                }
            }
            TypeExprKind::Generic(_, args) => {
                for arg in args {
                    self.replace_type_expr(arg, type_map);
                }
            }
            TypeExprKind::List(inner) | TypeExprKind::Ref(inner) | TypeExprKind::MutRef(inner) => {
                self.replace_type_expr(inner, type_map)
            }
            TypeExprKind::Tuple(elems) => {
                for e in elems {
                    self.replace_type_expr(e, type_map);
                }
            }
            TypeExprKind::Fn { params, ret } => {
                for p in params {
                    self.replace_type_expr(p, type_map);
                }
                self.replace_type_expr(ret, type_map);
            }
            _ => {}
        }
    }

    pub(super) fn type_to_type_expr_kind(&self, ty: &Type) -> crate::parser::TypeExprKind {
        use crate::parser::TypeExprKind;
        match ty {
            Type::Int => TypeExprKind::Name("int".to_string()),
            Type::Float => TypeExprKind::Name("float".to_string()),
            Type::Str => TypeExprKind::Name("str".to_string()),
            Type::Bool => TypeExprKind::Name("bool".to_string()),
            Type::Null => TypeExprKind::Name("None".to_string()),
            Type::Any => TypeExprKind::Name("Any".to_string()),
            Type::Never => TypeExprKind::Name("Never".to_string()),
            Type::List(inner) => TypeExprKind::List(Box::new(TypeExpr::new(
                self.type_to_type_expr_kind(inner),
                Span::default(),
            ))),
            Type::Struct(name, args) => {
                let type_args = args
                    .iter()
                    .map(|a| TypeExpr::new(self.type_to_type_expr_kind(a), Span::default()))
                    .collect();
                TypeExprKind::Generic(name.clone(), type_args)
            }
            _ => TypeExprKind::Name("Any".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::MirBuilder;
    use crate::lexer::Lexer;
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};
    use rustc_hash::FxHashSet;

    fn build(src: &str) -> Vec<super::super::super::ir::MirFunction> {
        let tokens = Lexer::new(src, 0).tokenise().unwrap();
        let prog = Parser::new(tokens).parse_program().unwrap();
        let mut r = Resolver::new();
        r.resolve_program(&prog);
        let mut tc = TypeChecker::new();
        tc.check_program(&prog);
        let mut builder = MirBuilder::new(
            &tc.expr_types,
            &tc.expr_kwarg_maps,
            &tc.type_env[0],
            tc.struct_fields.clone(),
            &tc.traits,
            FxHashSet::default(),
        );
        builder.build_program(&prog);
        builder.functions
    }

    #[test]
    fn generic_fn_monomorphized() {
        let fns = build("fn id[T](x: T) -> T:\n    return x\n\nlet r = id(42)\n");
        let id_fns: Vec<_> = fns.iter().filter(|f| f.name.starts_with("id")).collect();
        assert!(!id_fns.is_empty(), "expected at least one monomorphized id");
    }

    #[test]
    fn generic_struct_instantiated() {
        let fns = build("struct Box[T]:\n    val: T\n\nlet b = Box(42)\n");
        let has_box = fns.iter().any(|f| f.name.contains("Box"));
        assert!(has_box, "expected Box init function");
    }

    #[test]
    fn nested_generic_resolved() {
        let fns = build(
            "fn pair[T, U](a: T, b: U) -> (T, U):\n    return (a, b)\n\nlet p = pair(1, \"hi\")\n",
        );
        let pair_fns: Vec<_> = fns.iter().filter(|f| f.name.starts_with("pair")).collect();
        assert!(!pair_fns.is_empty());
    }

    #[test]
    fn resolve_type_expr_basic_types_produce_correct_mir() {
        let fns = build("fn f(x: i64, y: bool) -> i64:\n    return x\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        assert_eq!(f.arg_count, 2);
    }

    #[test]
    fn generic_fn_overloaded_with_different_types() {
        let fns = build("fn id[T](x: T) -> T:\n    return x\n\nlet a = id(1)\nlet b = id(true)\n");
        let id_fns: Vec<_> = fns.iter().filter(|f| f.name.starts_with("id")).collect();
        assert!(
            id_fns.len() >= 2,
            "expected at least 2 monomorphized versions"
        );
    }
}
