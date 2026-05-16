use super::MirBuilder;
use crate::parser::{Expr, ExprKind, Param, Stmt, StmtKind, TypeExpr};
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

impl<'a> MirBuilder<'a> {
    pub(super) fn resolve_type_expr(&self, expr: &TypeExpr) -> Type {
        use crate::parser::TypeExprKind;
        match &expr.kind {
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
                "bool" => Type::Bool,
                "None" => Type::Null,
                "Never" => Type::Never,
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
                _ => Type::Struct(name.clone(), Vec::new()),
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

        self.lower_stmt(&specialized_stmt);
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
