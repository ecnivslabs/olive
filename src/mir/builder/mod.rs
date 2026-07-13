mod closures;
mod generics;
mod lower_control;
mod lower_expr;
mod lower_pattern;
mod lower_stmt;

use super::ir::*;
use crate::mir::AggregateKind;
use crate::parser::{Expr, Param, ParamKind, Program, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Debug, Clone)]

pub(super) struct FnMeta {
    pub(super) param_names: Vec<String>,
    pub(super) vararg_idx: Option<usize>,
    pub(super) kwarg_idx: Option<usize>,
    pub(super) default_exprs: Vec<Option<crate::parser::Expr>>,
}

pub(super) struct LoopContext {
    pub(super) header: BasicBlockId,
    pub(super) exit: BasicBlockId,
    /// `scope_locals` depth of the loop's own frame(s); `break`/`continue` drop
    /// everything from this depth up, since they skip the loop body's normal
    /// end-of-iteration `leave_scope`.
    pub(super) scope_depth: usize,
    /// Iterator handle to free on every path out of the loop. `Any`-typed, so
    /// scope drops skip it; the exit block and `return` free it explicitly.
    pub(super) cleanup: Option<Local>,
}

/// A lifted nested fn: `mangled` is the global name (`parent$name`),
/// `raw_captures` the free-var list filtered to live locals per site.
#[derive(Clone)]
pub(super) struct NestedFnInfo {
    pub(super) mangled: String,
    pub(super) raw_captures: Vec<String>,
    pub(super) param_tys: Vec<Type>,
}

pub struct MirBuilder<'a> {
    pub functions: Vec<MirFunction>,
    pub expr_types: &'a HashMap<usize, Type>,
    pub expr_kwarg_maps: &'a HashMap<usize, Vec<usize>>,
    pub global_types: &'a HashMap<String, Type>,
    pub struct_fields: HashMap<String, Vec<String>>,
    /// (struct, field) -> field type, used to unbox Python scalars at struct construction sites.
    pub struct_field_types: HashMap<(String, String), Type>,
    /// Per struct, the default-value expression for each field (or `None` when
    /// the field has no default). Used to fill omitted trailing fields at a
    /// construction site.
    pub(super) struct_field_defaults: HashMap<String, Vec<Option<crate::parser::Expr>>>,
    /// Enum variant payload types, needed to encode a correct capture
    /// descriptor when a closure record (E5.2) captures an enum-typed value.
    pub(super) enum_defs: HashMap<String, Vec<(String, Vec<Type>)>>,
    /// Closure calling-convention thunks already emitted (`closures.rs`),
    /// deduped by mangled target name so re-lowering the same escaping
    /// closure (e.g. inside a loop body) doesn't emit it twice.
    pub(super) closure_thunks: HashSet<String>,
    /// Type-parameter substitution active while lowering a monomorphized
    /// function, so types read from the original (generic) `expr_types` are
    /// resolved to the concrete instance. Empty outside monomorphization.
    pub(super) mono_type_map: HashMap<String, Type>,
    pub traits: &'a HashMap<String, crate::semantic::type_checker::TraitDef>,

    pub(super) current_name: String,
    pub(super) current_locals: Vec<LocalDecl>,
    pub(super) current_blocks: Vec<BasicBlock>,
    pub(super) current_block: Option<BasicBlockId>,
    pub(super) current_arg_count: usize,
    pub(super) var_map: Vec<HashMap<String, Local>>,
    /// Nested-fn table per enclosing fn, pushed on body entry, popped on exit.
    pub(super) nested_fns: Vec<HashMap<String, NestedFnInfo>>,
    pub(super) loop_stack: Vec<LoopContext>,
    pub(super) scope_locals: Vec<Vec<Local>>,
    pub(super) memo_context: Option<(Operand, Operand, BasicBlockId)>,
    pub globals: HashMap<String, Operand>,
    pub enum_variants: HashMap<String, (String, usize)>,
    pub(super) current_is_async: bool,
    pub(super) fn_meta: HashMap<String, FnMeta>,
    pub(super) generic_fns: HashMap<String, crate::parser::Stmt>,
    pub(super) defer_stack: Vec<crate::parser::Expr>,
    pub(super) py_exit_stack: Vec<(Local, Span)>,
    pub c_ffi_fns: HashSet<String>,
    pub vtables: HashMap<String, Vec<String>>,
    pub global_vars: Vec<String>,
    /// file_id → source filename, used to locate runtime FFI errors. Empty when
    /// the builder is driven directly (e.g. in unit tests).
    pub file_names: HashMap<usize, String>,
    /// Guards against re-entrant call-site tagging while emitting the
    /// `__olive_py_set_loc` statements themselves.
    pub(super) in_py_loc_emit: bool,
    /// Lambdas bound directly to a name (`let g = lambda: ...`), per
    /// enclosing fn: same shape as `nested_fns`, looked up separately since
    /// the bound name is always a local var (the `nested_fns` lookup
    /// deliberately skips names shadowed by a local).
    pub(super) bound_lambdas: Vec<HashMap<String, NestedFnInfo>>,
}

impl<'a> MirBuilder<'a> {
    pub fn new(
        expr_types: &'a HashMap<usize, Type>,
        expr_kwarg_maps: &'a HashMap<usize, Vec<usize>>,
        global_types: &'a HashMap<String, Type>,
        struct_fields: HashMap<String, Vec<String>>,
        traits: &'a HashMap<String, crate::semantic::type_checker::TraitDef>,
        c_ffi_fns: HashSet<String>,
        enum_defs: HashMap<String, Vec<(String, Vec<Type>)>>,
    ) -> Self {
        // Built-in single-variant `Error` enum, mirrored from the type checker so
        // construction and `match` lower the same way user enums do.
        let mut enum_variants: HashMap<String, (String, usize)> = HashMap::default();
        enum_variants.insert("Error::Error".to_string(), ("Error".to_string(), 0));
        enum_variants.insert("Error".to_string(), ("Error".to_string(), 0));
        Self {
            functions: Vec::new(),
            expr_types,
            expr_kwarg_maps,
            global_types,
            struct_fields,
            struct_field_types: HashMap::default(),
            struct_field_defaults: HashMap::default(),
            enum_defs,
            closure_thunks: HashSet::default(),
            mono_type_map: HashMap::default(),
            traits,
            current_name: String::new(),
            current_locals: Vec::new(),
            current_blocks: Vec::new(),
            current_block: None,
            current_arg_count: 0,
            var_map: Vec::new(),
            nested_fns: Vec::new(),
            loop_stack: Vec::new(),
            scope_locals: Vec::new(),
            memo_context: None,
            globals: HashMap::default(),
            enum_variants,
            current_is_async: false,
            fn_meta: HashMap::default(),
            generic_fns: HashMap::default(),
            defer_stack: Vec::new(),
            py_exit_stack: Vec::new(),
            c_ffi_fns,
            vtables: HashMap::default(),
            global_vars: Vec::new(),
            file_names: HashMap::default(),
            in_py_loc_emit: false,
            bound_lambdas: Vec::new(),
        }
    }

    /// True while building `__main__` (top-level statements). A `let` there
    /// needs real global storage; inside a function it must stay a local.
    pub(super) fn at_module_scope(&self) -> bool {
        self.current_name == "__main__"
    }

    /// True for Python runtime ops that surface an exception via `handle_py_error`,
    /// so the Olive call site must be recorded before them.
    pub(super) fn is_raising_py_op(name: &str) -> bool {
        matches!(
            name,
            "__olive_py_call"
                | "__olive_py_call_kw"
                | "__olive_py_getattr"
                | "__olive_py_getitem"
                | "__olive_py_getitem_int"
                | "__olive_py_getslice"
                | "__olive_py_setattr"
                | "__olive_py_setitem"
                | "__olive_py_setitem_int"
                | "__olive_py_bitor"
                | "__olive_py_iter"
                | "__olive_py_import"
                | "__olive_py_run_coroutine"
        )
    }

    pub fn build_program(&mut self, program: &Program) {
        for stmt in &program.stmts {
            match &stmt.kind {
                StmtKind::Fn { name, params, .. } => {
                    self.register_fn_meta(name, params);
                }
                StmtKind::Impl {
                    type_name, body, ..
                } => {
                    let type_base = Self::type_expr_base_name(type_name);
                    for s in body {
                        if let StmtKind::Fn {
                            name: fn_name,
                            params,
                            ..
                        } = &s.kind
                        {
                            let mangled = format!("{}::{}", type_base, fn_name);
                            self.register_fn_meta(&mangled, params);
                        }
                    }
                }
                StmtKind::NativeImport { alias, consts, .. } => {
                    for c in consts {
                        let mangled = format!("{}::{}", alias, c.name);
                        self.globals
                            .insert(mangled, Operand::Constant(Constant::Int(c.value)));
                    }
                }
                StmtKind::Struct { name, fields, .. }
                    if fields.iter().any(|f| f.default.is_some()) =>
                {
                    self.struct_field_defaults.insert(
                        name.clone(),
                        fields.iter().map(|f| f.default.clone()).collect(),
                    );
                }
                _ => {}
            }
        }
        self.start_function("__main__".to_string(), 0, Type::Int);

        for stmt in &program.stmts {
            match &stmt.kind {
                StmtKind::Fn { .. } | StmtKind::Impl { .. } => self.lower_fn_def_or_impl(stmt),
                StmtKind::Trait { .. } => {}
                _ => self.lower_stmt(stmt),
            }
        }

        let has_main_fn = program
            .stmts
            .iter()
            .any(|s| matches!(&s.kind, StmtKind::Fn { name, .. } if name == "main"));

        let already_calls_main = program.stmts.iter().any(|s| {
            if let StmtKind::ExprStmt(expr) = &s.kind
                && let crate::parser::ExprKind::Call { callee, .. } = &expr.kind
                && let crate::parser::ExprKind::Identifier(name) = &callee.kind
            {
                return name == "main";
            }
            false
        });

        if has_main_fn && !already_calls_main {
            let main_call_expr = crate::parser::Expr::new(
                crate::parser::ExprKind::Call {
                    callee: Box::new(crate::parser::Expr::new(
                        crate::parser::ExprKind::Identifier("main".to_string()),
                        Span::default(),
                    )),
                    args: vec![],
                },
                Span::default(),
            );

            // `main`'s own return type, resolved the same way a regular
            // function's is. Only an integer-like type is forwarded as the
            // process exit code (the only sensible reading of a return value
            // there); anything else keeps the prior behavior of calling
            // `main` for its side effects and exiting 0.
            let main_ret_ty = program
                .stmts
                .iter()
                .find_map(|s| match &s.kind {
                    StmtKind::Fn {
                        name,
                        return_type,
                        is_async,
                        ..
                    } if name == "main" => Some(match return_type {
                        Some(ann) => self.resolve_type_expr(ann),
                        None => self.inferred_return_type("main", *is_async),
                    }),
                    _ => None,
                })
                .unwrap_or(Type::Any);

            if Self::is_exit_code_type(&main_ret_ty) {
                let rval = self.lower_expr(&main_call_expr);
                let coerced = self.coerce(rval, &main_ret_ty, &Type::Int, Span::default());
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(coerced)),
                    Span::default(),
                );
            } else {
                let main_call_stmt = crate::parser::Stmt::new(
                    crate::parser::StmtKind::ExprStmt(main_call_expr),
                    Span::default(),
                );
                self.lower_stmt(&main_call_stmt);
            }
        }

        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Return, Span::default());
        }
        self.finish_function();
    }

    /// Whether a type is a sensible process exit code: a plain integer.
    fn is_exit_code_type(ty: &Type) -> bool {
        matches!(
            ty,
            Type::Int
                | Type::I8
                | Type::I16
                | Type::I32
                | Type::U8
                | Type::U16
                | Type::U32
                | Type::U64
                | Type::Usize
        )
    }

    /// After building all user code, monomorphize `__drop__` for each concrete
    /// instantiation of generic structs that define a drop hook.
    pub fn monomorphize_drop_fns(&mut self) {
        let generic_drop_keys: Vec<String> = self
            .generic_fns
            .keys()
            .filter(|k| k.ends_with("::__drop__"))
            .cloned()
            .collect();

        if generic_drop_keys.is_empty() {
            return;
        }

        let mut work: Vec<(String, Vec<Type>)> = Vec::new();
        for drop_key in &generic_drop_keys {
            let struct_name = drop_key.strip_suffix("::__drop__").unwrap().to_string();
            for func in &self.functions {
                for local in &func.locals {
                    if let Type::Struct(name, args, _) = &local.ty
                        && *name == struct_name
                        && !args.is_empty()
                        && !work.iter().any(|(n, a)| n == &struct_name && a == args)
                    {
                        work.push((struct_name.clone(), args.clone()));
                    }
                }
            }
        }

        for (struct_name, type_args) in &work {
            let drop_key = format!("{}::__drop__", struct_name);
            if self.generic_fns.contains_key(&drop_key) {
                self.monomorphize(&drop_key, type_args);
            }
        }
    }

    pub(super) fn start_function(&mut self, name: String, arg_count: usize, ret_ty: Type) {
        self.current_name = name;
        self.current_locals.clear();
        self.current_blocks.clear();
        self.var_map.clear();
        self.loop_stack.clear();
        self.defer_stack.clear();
        self.current_arg_count = arg_count;
        self.enter_scope();

        let start_bb = self.new_block();
        self.current_block = Some(start_bb);

        let default_val = match ret_ty {
            Type::Float => Operand::Constant(Constant::Float(0.0f64.to_bits())),
            Type::Bool => Operand::Constant(Constant::Bool(false)),
            _ => Operand::Constant(Constant::Int(0)),
        };
        let ret = self.new_local(ret_ty, Some("_return".to_string()), true);
        self.push_statement(
            StatementKind::Assign(ret, Rvalue::Use(default_val)),
            Span::default(),
        );
    }

    pub(super) fn finish_function(&mut self) {
        self.leave_scope();
        let meta = self.fn_meta.get(&self.current_name).cloned();
        let func = MirFunction {
            name: self.current_name.clone(),
            locals: std::mem::take(&mut self.current_locals),
            basic_blocks: std::mem::take(&mut self.current_blocks),
            arg_count: self.current_arg_count,
            vararg_idx: meta.as_ref().and_then(|m| m.vararg_idx),
            kwarg_idx: meta.as_ref().and_then(|m| m.kwarg_idx),
            param_names: meta.map(|m| m.param_names).unwrap_or_default(),
            is_async: self.current_is_async,
        };
        self.functions.retain(|f| f.name != func.name);
        self.functions.push(func);
    }

    pub(super) fn register_fn_meta(&mut self, name: &str, params: &[Param]) {
        let mut vararg_idx = None;
        let mut kwarg_idx = None;
        let param_names = params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                match p.kind {
                    ParamKind::VarArg => vararg_idx = Some(i),
                    ParamKind::KwArg => kwarg_idx = Some(i),
                    ParamKind::Regular => {}
                }
                p.name.clone()
            })
            .collect();
        let default_exprs = params.iter().map(|p| p.default.clone()).collect();
        self.fn_meta.insert(
            name.to_string(),
            FnMeta {
                param_names,
                vararg_idx,
                kwarg_idx,
                default_exprs,
            },
        );
    }

    pub(super) fn pack_fn_call_args(
        &mut self,
        fn_name: &str,
        arg_ops: &[Operand],
        arg_tys: &[Type],
        param_tys: &[Type],
        arg_kw_names: &[Option<String>],
        span: Span,
    ) -> Vec<Operand> {
        let meta = match self.fn_meta.get(fn_name).cloned() {
            Some(m) => m,
            None => {
                let mut res = Vec::new();
                for i in 0..arg_ops.len() {
                    let p_ty = param_tys.get(i).unwrap_or(&Type::Any);
                    res.push(self.coerce(arg_ops[i].clone(), &arg_tys[i], p_ty, span));
                }
                return res;
            }
        };

        let param_names = &meta.param_names;
        let vararg_idx = meta.vararg_idx;
        let kwarg_idx = meta.kwarg_idx;

        if vararg_idx.is_none()
            && kwarg_idx.is_none()
            && arg_kw_names.iter().all(|k| k.is_none())
            && arg_ops.len() == param_names.len()
        {
            let mut res = Vec::new();
            for i in 0..arg_ops.len() {
                let p_ty = param_tys.get(i).unwrap_or(&Type::Any);
                res.push(self.coerce(arg_ops[i].clone(), &arg_tys[i], p_ty, span));
            }
            return res;
        }

        let regular_end = vararg_idx.or(kwarg_idx).unwrap_or(param_names.len());

        let mut positional: Vec<(Operand, Type)> = Vec::new();
        let mut keyword: Vec<(String, Operand, Type)> = Vec::new();
        for (i, (op, kw)) in arg_ops.iter().zip(arg_kw_names.iter()).enumerate() {
            let ty = arg_tys[i].clone();
            match kw {
                Some(name) => keyword.push((name.clone(), op.clone(), ty)),
                None => positional.push((op.clone(), ty)),
            }
        }

        let mut result: Vec<Option<Operand>> = vec![None; param_names.len()];

        let mut pos_consumed = 0;
        for (i, slot) in result.iter_mut().enumerate().take(regular_end) {
            if Some(i) == vararg_idx || Some(i) == kwarg_idx {
                continue;
            }
            if pos_consumed < positional.len() {
                let p_ty = param_tys.get(i).unwrap_or(&Type::Any);
                *slot = Some(self.coerce(
                    positional[pos_consumed].0.clone(),
                    &positional[pos_consumed].1,
                    p_ty,
                    span,
                ));
                pos_consumed += 1;
            }
        }

        for (kw_name, kw_op, kw_ty) in &keyword {
            if let Some(pos) = param_names.iter().position(|n| n == kw_name)
                && Some(pos) != vararg_idx
                && Some(pos) != kwarg_idx
                && pos < regular_end
            {
                let p_ty = param_tys.get(pos).unwrap_or(&Type::Any);
                result[pos] = Some(self.coerce(kw_op.clone(), kw_ty, p_ty, span));
            }
        }

        if let Some(vi) = vararg_idx {
            let extra: Vec<Operand> = positional[pos_consumed..]
                .iter()
                .map(|(op, _)| op.clone())
                .collect();
            let list_tmp = self.new_local(Type::List(Box::new(Type::Any)), None, false);
            self.push_statement(
                StatementKind::Assign(list_tmp, Rvalue::Aggregate(AggregateKind::List, extra)),
                span,
            );
            result[vi] = Some(self.operand_for_local(list_tmp));
        }

        if let Some(ki) = kwarg_idx {
            let extra_kw: Vec<Operand> = keyword
                .iter()
                .filter(|(kw_name, _, _)| {
                    param_names
                        .iter()
                        .position(|n| n == kw_name)
                        .map(|p| p == ki || p >= regular_end)
                        .unwrap_or(true)
                })
                .flat_map(|(kw_name, kw_op, _)| {
                    [
                        Operand::Constant(Constant::Str(kw_name.clone())),
                        kw_op.clone(),
                    ]
                })
                .collect();
            let dict_tmp = self.new_local(
                Type::Dict(Box::new(Type::Str), Box::new(Type::Any)),
                None,
                false,
            );
            self.push_statement(
                StatementKind::Assign(dict_tmp, Rvalue::Aggregate(AggregateKind::Dict, extra_kw)),
                span,
            );
            result[ki] = Some(self.operand_for_local(dict_tmp));
        }

        let mut final_result = Vec::new();
        for (i, op_opt) in result.into_iter().enumerate() {
            if let Some(op) = op_opt {
                final_result.push(op);
            } else if i < meta.default_exprs.len() && meta.default_exprs[i].is_some() {
                let default_expr = meta.default_exprs[i].as_ref().unwrap();
                let op = self.lower_expr_as_copy(default_expr);
                let default_ty = self.get_type(default_expr.id);
                let p_ty = param_tys.get(i).unwrap_or(&Type::Any);
                final_result.push(self.coerce(op, &default_ty, p_ty, span));
            } else {
                final_result.push(Operand::Constant(Constant::Int(0)));
            }
        }
        final_result
    }

    pub(super) fn enter_scope(&mut self) {
        self.var_map.push(HashMap::default());
        self.scope_locals.push(Vec::new());
    }

    pub(super) fn leave_scope(&mut self) {
        if let Some(locals) = self.scope_locals.pop() {
            for local in locals.into_iter().rev() {
                let decl = &self.current_locals[local.0];
                if decl.ty.is_move_type() && decl.is_owning {
                    self.push_statement(StatementKind::Drop(local), Span::default());
                }
                self.push_statement(StatementKind::StorageDead(local), Span::default());
            }
        }
        self.var_map.pop();
    }

    /// Drops every owning local in the open scope frames from `from_depth` up,
    /// without popping them, for a control-transfer statement (`return`,
    /// `break`, `continue`) that jumps out of those scopes early. Without this,
    /// only the eventual *normal* `leave_scope` would drop them -- but that
    /// call lands in the block created after the jump, which has no
    /// predecessor and is deleted as dead code by the optimizer, silently
    /// leaking every local the jump skipped past.
    ///
    /// `exclude` is the local (if any) whose value is being carried out by the
    /// jump itself (e.g. `return that_local`); its ownership has already
    /// transferred to `_return`; dropping it here would free the value out
    /// from under the caller.
    pub(super) fn emit_open_scope_drops(&mut self, from_depth: usize, exclude: Option<Local>) {
        // Local(0) is always `_return` (registered in scope by `start_function`
        // before the body is lowered); it holds the value being carried out and
        // must never be dropped here regardless of `exclude`.
        let to_drop: Vec<Local> = self.scope_locals[from_depth..]
            .iter()
            .rev()
            .flat_map(|frame| frame.iter().rev().copied())
            .filter(|&local| {
                local != Local(0) && Some(local) != exclude && {
                    let decl = &self.current_locals[local.0];
                    decl.ty.is_move_type() && decl.is_owning
                }
            })
            .collect();
        for local in to_drop {
            self.push_statement(StatementKind::Drop(local), Span::default());
        }
    }

    /// Frees a loop's iterator handle. The runtime call is idempotent (guarded
    /// by the live-object registry), so converging paths may each emit one.
    pub(super) fn emit_iter_free(&mut self, iter_local: Local) {
        let sink = self.new_unscoped_local(Type::Int);
        self.push_statement(
            StatementKind::Assign(
                sink,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_free_iter".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            Span::default(),
        );
    }

    /// Frees the iterators of every loop still open at a `return`, which jumps
    /// past each loop's exit block where they are normally freed.
    pub(super) fn emit_open_loop_iter_frees(&mut self) {
        let iters: Vec<Local> = self.loop_stack.iter().filter_map(|c| c.cleanup).collect();
        for iter_local in iters {
            self.emit_iter_free(iter_local);
        }
    }

    pub(super) fn get_type(&self, expr_id: usize) -> Type {
        let ty = self.expr_types.get(&expr_id).cloned().unwrap_or(Type::Any);
        let ty = if self.mono_type_map.is_empty() {
            ty
        } else {
            self.subst_mono_type(&ty)
        };
        match ty {
            Type::PyNamed(_, _) => Type::PyObject,
            ty => ty,
        }
    }

    /// Applies the active monomorphization substitution to a type. A type
    /// parameter shows up either as `Param(n)` or, once resolved through the
    /// type checker, as a zero-arg `Struct(n)`; both map to the concrete type.
    pub(super) fn subst_mono_type(&self, ty: &Type) -> Type {
        match ty {
            Type::Param(n) => self
                .mono_type_map
                .get(n)
                .cloned()
                .unwrap_or_else(|| ty.clone()),
            Type::Struct(n, args, _is_ffi)
                if args.is_empty() && self.mono_type_map.contains_key(n) =>
            {
                self.mono_type_map[n].clone()
            }
            Type::Struct(n, args, is_ffi) => Type::Struct(
                n.clone(),
                args.iter().map(|a| self.subst_mono_type(a)).collect(),
                *is_ffi,
            ),
            Type::Enum(n, args) => Type::Enum(
                n.clone(),
                args.iter().map(|a| self.subst_mono_type(a)).collect(),
            ),
            Type::List(t) => Type::List(Box::new(self.subst_mono_type(t))),
            Type::Set(t) => Type::Set(Box::new(self.subst_mono_type(t))),
            Type::Dict(k, v) => Type::Dict(
                Box::new(self.subst_mono_type(k)),
                Box::new(self.subst_mono_type(v)),
            ),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.subst_mono_type(t)).collect()),
            Type::Ref(t) => Type::Ref(Box::new(self.subst_mono_type(t))),
            Type::MutRef(t) => Type::MutRef(Box::new(self.subst_mono_type(t))),
            Type::Ptr(t) => Type::Ptr(Box::new(self.subst_mono_type(t))),
            _ => ty.clone(),
        }
    }

    pub(super) fn new_tmp_for_expr(&mut self, expr: &Expr) -> Local {
        let ty = self.get_type(expr.id);
        self.new_local(ty, None, true)
    }

    pub(super) fn new_tmp_for_expr_with_owning(&mut self, expr: &Expr, is_owning: bool) -> Local {
        let ty = self.get_type(expr.id);
        self.new_local_with_owning(ty, None, true, is_owning)
    }

    pub(super) fn new_local(&mut self, ty: Type, name: Option<String>, is_mut: bool) -> Local {
        self.new_local_with_owning(ty, name, is_mut, true)
    }

    pub(super) fn new_local_with_owning(
        &mut self,
        ty: Type,
        name: Option<String>,
        is_mut: bool,
        is_owning: bool,
    ) -> Local {
        let id = self.current_locals.len();
        let local = Local(id);
        self.current_locals.push(LocalDecl {
            ty,
            name,
            span: Span::default(),
            is_mut,
            is_owning,
        });
        self.push_statement(StatementKind::StorageLive(local), Span::default());
        if let Some(scope) = self.scope_locals.last_mut() {
            scope.push(local);
        }
        local
    }

    pub(super) fn new_unscoped_local(&mut self, ty: Type) -> Local {
        self.new_unscoped_local_with_owning(ty, true)
    }

    pub(super) fn new_unscoped_local_with_owning(&mut self, ty: Type, is_owning: bool) -> Local {
        let id = self.current_locals.len();
        let local = Local(id);
        self.current_locals.push(LocalDecl {
            ty,
            name: None,
            span: Span::default(),
            is_mut: true,
            is_owning,
        });
        self.push_statement(StatementKind::StorageLive(local), Span::default());
        local
    }

    pub(super) fn new_block(&mut self) -> BasicBlockId {
        let id = self.current_blocks.len();
        self.current_blocks.push(BasicBlock {
            statements: Vec::new(),
            terminator: None,
        });
        BasicBlockId(id)
    }

    pub(super) fn terminate_block(&mut self, bb: BasicBlockId, kind: TerminatorKind, span: Span) {
        if let Some(block) = self.current_blocks.get_mut(bb.0)
            && block.terminator.is_none()
        {
            block.terminator = Some(Terminator { kind, span });
        }
    }

    pub(super) fn push_statement(&mut self, kind: StatementKind, span: Span) {
        if !self.in_py_loc_emit
            && let StatementKind::Assign(_, Rvalue::Call { func, .. }) = &kind
            && let Operand::Constant(Constant::Function(name)) = func
            && Self::is_raising_py_op(name)
        {
            self.in_py_loc_emit = true;
            self.emit_py_set_loc(span);
            self.in_py_loc_emit = false;
        }
        if let Some(bb) = self.current_block {
            self.current_blocks[bb.0]
                .statements
                .push(Statement { kind, span });
        }
    }

    pub(super) fn declare_var(&mut self, name: String, ty: Type, is_mut: bool) -> Local {
        let local = self.new_local(ty, Some(name.clone()), is_mut);
        self.var_map.last_mut().unwrap().insert(name, local);
        local
    }

    /// Declares a variable that does not own its value. Used for bindings that
    /// are views into a value owned elsewhere (e.g. a tuple-destructure target
    /// pointing into the iterated element), so the value is not freed twice.
    pub(super) fn declare_var_view(&mut self, name: String, ty: Type, is_mut: bool) -> Local {
        let local = self.new_local_with_owning(ty, Some(name.clone()), is_mut, false);
        self.var_map.last_mut().unwrap().insert(name, local);
        local
    }

    /// Lowers `expr` and hands back a local holding it, materializing a
    /// fresh local first if lowering produced a bare constant.
    pub(super) fn local_of_expr(&mut self, expr: &Expr) -> (Local, Type) {
        let op = self.lower_expr(expr);
        let ty = self.get_type(expr.id);
        let local = match op {
            Operand::Copy(l) | Operand::Move(l) => l,
            Operand::Constant(_) => {
                let tmp = self.new_local(ty.clone(), None, false);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(op)), expr.span);
                tmp
            }
        };
        (local, ty)
    }

    /// Borrows `expr` for iteration instead of copying it: emits the exact
    /// MIR shape of an explicit `&expr` (`Rvalue::Ref`, never owning, so no
    /// `Drop` and the borrow checker enforces exclusivity against the
    /// source for the loop's duration). Already-`Ref`/`MutRef` typed
    /// iterables are reused as-is rather than double-wrapped.
    pub(super) fn borrow_iterable(&mut self, expr: &Expr) -> (Local, Type) {
        let (src_local, ty) = self.local_of_expr(expr);
        if matches!(ty, Type::Ref(_) | Type::MutRef(_)) {
            return (src_local, ty);
        }
        let ref_ty = Type::Ref(Box::new(ty));
        let ref_local = self.new_local_with_owning(ref_ty.clone(), None, false, false);
        self.push_statement(
            StatementKind::Assign(ref_local, Rvalue::Ref(src_local)),
            expr.span,
        );
        (ref_local, ref_ty)
    }

    pub(super) fn lookup_var(&self, name: &str) -> Option<Local> {
        for scope in self.var_map.iter().rev() {
            if let Some(&local) = scope.get(name) {
                return Some(local);
            }
        }
        None
    }

    /// Every use is a borrow; ownership transfers are inferred later from
    /// liveness (ownership pass + MoveElision), never at the use site.
    pub(super) fn operand_for_local(&self, local: Local) -> Operand {
        Operand::Copy(local)
    }

    pub(super) fn is_terminated(&self) -> bool {
        self.current_block
            .and_then(|bb| self.current_blocks.get(bb.0))
            .is_none_or(|b| b.terminator.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::mir::ir::TerminatorKind;
    use crate::parser::Parser;
    use crate::semantic::{Resolver, TypeChecker};

    fn build(src: &str) -> (Vec<MirFunction>, rustc_hash::FxHashMap<String, Vec<String>>) {
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
            HashSet::default(),
            tc.enum_defs.clone(),
        );
        builder.build_program(&prog);
        (builder.functions, builder.struct_fields)
    }

    #[test]
    fn simple_let_produces_assign_stmt() {
        let (fns, _) = build("let x = 42\n");
        let main = fns.iter().find(|f| f.name == "__main__").unwrap();
        let has_assign = main.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(s.kind, StatementKind::Assign(_, Rvalue::Use(_))))
        });
        assert!(has_assign);
    }

    #[test]
    fn function_emitted_as_mir_function() {
        let (fns, _) = build("fn add(a: i64, b: i64) -> i64:\n    return a + b\n");
        assert!(fns.iter().any(|f| f.name == "add"));
    }

    #[test]
    fn function_has_correct_arg_count() {
        let (fns, _) = build("fn add(a: i64, b: i64) -> i64:\n    return a + b\n");
        let f = fns.iter().find(|f| f.name == "add").unwrap();
        assert_eq!(f.arg_count, 2);
    }

    #[test]
    fn function_basic_block_has_terminator() {
        let (fns, _) = build("fn foo() -> i64:\n    return 1\n");
        let f = fns.iter().find(|f| f.name == "foo").unwrap();
        assert!(f.basic_blocks.iter().all(|bb| bb.terminator.is_some()));
    }

    #[test]
    fn if_statement_creates_multiple_blocks() {
        let (fns, _) =
            build("fn foo(x: i64) -> i64:\n    if x > 0:\n        return 1\n    return 0\n");
        let f = fns.iter().find(|f| f.name == "foo").unwrap();
        assert!(f.basic_blocks.len() >= 2);
    }

    #[test]
    fn while_loop_creates_backedge() {
        let (fns, _) = build(
            "fn count(n: i64) -> i64:\n    let i = 0\n    while i < n:\n        i = i + 1\n    return i\n",
        );
        let f = fns.iter().find(|f| f.name == "count").unwrap();
        let has_goto = f.basic_blocks.iter().any(|bb| {
            bb.terminator
                .as_ref()
                .is_some_and(|t| matches!(t.kind, TerminatorKind::Goto { .. }))
        });
        assert!(has_goto);
    }

    #[test]
    fn struct_fields_registered() {
        let (_, struct_fields) = build("struct Vec2:\n    x: i64\n    y: i64\n");
        assert!(struct_fields.contains_key("Vec2"));
        let fields = &struct_fields["Vec2"];
        assert!(fields.contains(&"x".to_string()));
        assert!(fields.contains(&"y".to_string()));
    }

    #[test]
    fn generic_fn_monomorphized_on_call() {
        let (fns, _) = build("fn id[T](x: T) -> T:\n    return x\n\nlet r = id(5)\n");
        assert!(fns.iter().any(|f| f.name.starts_with("id")));
    }

    #[test]
    fn constant_folding_reduces_ops() {
        let (mut fns, _) = build("fn f() -> i64:\n    return 2 + 3\n");
        let opt = crate::mir::Optimizer::new();
        let (_diags, _copy_sites) = opt.run(&mut fns);
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_const5 = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                matches!(
                    &s.kind,
                    StatementKind::Assign(
                        _,
                        Rvalue::Use(crate::mir::Operand::Constant(crate::mir::Constant::Int(5)))
                    )
                )
            })
        });
        assert!(has_const5, "const fold should produce Int(5) from 2+3");
    }

    #[test]
    fn binary_op_produces_binop_rvalue() {
        let (fns, _) = build("fn f() -> i64:\n    return 1 + 2\n");
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_binop = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(s.kind, StatementKind::Assign(_, Rvalue::BinaryOp(_, _, _))))
        });
        assert!(has_binop);
    }

    #[test]
    fn nested_if_creates_multiple_blocks() {
        let (fns, _) = build(
            "fn sign(x: i64) -> i64:\n    if x > 0:\n        return 1\n    else:\n        if x < 0:\n            return -1\n    return 0\n",
        );
        let f = fns.iter().find(|f| f.name == "sign").unwrap();
        assert!(f.basic_blocks.len() >= 3);
    }

    #[test]
    fn for_loop_emits_iter_calls() {
        let (fns, _) = build(
            "fn sum_list(xs: [i64]) -> i64:\n    let s = 0\n    for x in xs:\n        s = s + x\n    return s\n",
        );
        let f = fns.iter().find(|f| f.name == "sum_list").unwrap();
        let has_call = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(s.kind, StatementKind::Assign(_, Rvalue::Call { .. })))
        });
        assert!(has_call);
    }

    #[test]
    fn enum_variant_produces_aggregate() {
        let (fns, _) = build("enum Opt:\n    Some(i64)\n    Nil\n\nlet v = Some(42)\n");
        let main = fns.iter().find(|f| f.name == "__main__").unwrap();
        let has_aggregate = main.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(s.kind, StatementKind::Assign(_, Rvalue::Aggregate(_, _))))
        });
        assert!(has_aggregate);
    }

    #[test]
    fn list_literal_produces_aggregate() {
        let (fns, _) = build("let xs = [1, 2, 3]\n");
        let main = fns.iter().find(|f| f.name == "__main__").unwrap();
        let has_list = main.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(s.kind, StatementKind::Assign(_, Rvalue::Aggregate(_, _))))
        });
        assert!(has_list);
    }

    #[test]
    fn recursive_function_has_self_call() {
        let (fns, _) = build(
            "fn fact(n: i64) -> i64:\n    if n <= 1:\n        return 1\n    return n * fact(n - 1)\n",
        );
        let f = fns.iter().find(|f| f.name == "fact").unwrap();
        let has_self_call = f.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                if let StatementKind::Assign(_, Rvalue::Call { func, .. }) = &s.kind {
                    matches!(func, crate::mir::Operand::Constant(crate::mir::Constant::Function(name)) if name == "fact")
                } else {
                    false
                }
            })
        });
        assert!(has_self_call);
    }

    #[test]
    fn struct_field_access_produces_getattr() {
        let (fns, _) = build(
            "struct Pt:\n    x: i64\n    y: i64\n\nfn get_x(p: Pt) -> i64:\n    return p.x\n",
        );
        let f = fns.iter().find(|f| f.name == "get_x").unwrap();
        let has_getattr = f.basic_blocks.iter().any(|bb| {
            bb.statements
                .iter()
                .any(|s| matches!(s.kind, StatementKind::Assign(_, Rvalue::GetAttr(_, _))))
        });
        assert!(has_getattr);
    }

    #[test]
    fn multiple_generic_instantiations_distinct() {
        let (fns, _) =
            build("fn id[T](x: T) -> T:\n    return x\n\nlet a = id(1)\nlet b = id(\"hi\")\n");
        let id_fns: Vec<_> = fns.iter().filter(|f| f.name.starts_with("id")).collect();
        assert!(
            id_fns.len() >= 2,
            "should produce two monomorphized id variants"
        );
    }

    #[test]
    fn match_produces_switch_int_terminator() {
        let (fns, _) = build(
            "enum Color:\n    Red\n    Green\n    Blue\n\nfn f(c: Color) -> i64:\n    match c:\n        case Red:\n            return 0\n        case _:\n            return 1\n",
        );
        let f = fns.iter().find(|f| f.name == "f").unwrap();
        let has_switch = f.basic_blocks.iter().any(|bb| {
            bb.terminator
                .as_ref()
                .is_some_and(|t| matches!(t.kind, crate::mir::ir::TerminatorKind::SwitchInt { .. }))
        });
        assert!(has_switch);
    }

    #[test]
    fn auto_main_appends_call() {
        let (fns, _) = build("fn main():\n    let x = 1\n");
        let main = fns.iter().find(|f| f.name == "__main__").unwrap();
        let has_main_call = main.basic_blocks.iter().any(|bb| {
            bb.statements.iter().any(|s| {
                if let StatementKind::Assign(_, Rvalue::Call { func, .. }) = &s.kind {
                    matches!(func, crate::mir::Operand::Constant(crate::mir::Constant::Function(name)) if name == "main")
                } else {
                    false
                }
            })
        });
        assert!(has_main_call);
    }
}
