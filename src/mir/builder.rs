use super::ir::*;
use crate::mir::AggregateKind;
use crate::parser::{
    BinOp, CallArg, CompClause, Expr, ExprKind, ForTarget, MatchPattern, Param, ParamKind, Program,
    Stmt, StmtKind,
};
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone)]
struct FnMeta {
    param_names: Vec<String>,
    vararg_idx: Option<usize>,
    kwarg_idx: Option<usize>,
}

// loop targets
struct LoopContext {
    header: BasicBlockId,
    exit: BasicBlockId,
}

pub struct MirBuilder<'a> {
    pub functions: Vec<MirFunction>,
    pub expr_types: &'a HashMap<usize, Type>,
    pub global_types: &'a HashMap<String, Type>,

    current_name: String,
    current_locals: Vec<LocalDecl>,
    current_blocks: Vec<BasicBlock>,
    current_block: Option<BasicBlockId>,
    current_arg_count: usize,
    var_map: Vec<HashMap<String, Local>>,
    loop_stack: Vec<LoopContext>,
    scope_locals: Vec<Vec<Local>>,
    memo_context: Option<(Operand, Operand, BasicBlockId)>, // memo state
    pub globals: HashMap<String, Operand>,
    pub enum_variants: HashMap<String, (String, usize)>,
    current_is_async: bool,
    fn_meta: HashMap<String, FnMeta>,
}

impl<'a> MirBuilder<'a> {
    pub fn new(
        expr_types: &'a HashMap<usize, Type>,
        global_types: &'a HashMap<String, Type>,
    ) -> Self {
        Self {
            functions: Vec::new(),
            expr_types,
            global_types,
            current_name: String::new(),
            current_locals: Vec::new(),
            current_blocks: Vec::new(),
            current_block: None,
            current_arg_count: 0,
            var_map: Vec::new(),
            loop_stack: Vec::new(),
            scope_locals: Vec::new(),
            memo_context: None,
            globals: HashMap::default(),
            enum_variants: HashMap::default(),
            current_is_async: false,
            fn_meta: HashMap::default(),
        }
    }

    pub fn build_program(&mut self, program: &Program) {
        // Pre-scan: register param metadata for all functions before lowering
        for stmt in &program.stmts {
            match &stmt.kind {
                StmtKind::Fn { name, params, .. } => {
                    self.register_fn_meta(name, params);
                }
                StmtKind::Impl {
                    type_name, body, ..
                } => {
                    for s in body {
                        if let StmtKind::Fn {
                            name: fn_name,
                            params,
                            ..
                        } = &s.kind
                        {
                            let mangled = format!("{}::{}", type_name, fn_name);
                            self.register_fn_meta(&mangled, params);
                        }
                    }
                }
                _ => {}
            }
        }
        self.start_function("__main__".to_string(), 0, Type::Any);

        for stmt in &program.stmts {
            match &stmt.kind {
                StmtKind::Fn { .. } | StmtKind::Impl { .. } => self.lower_fn_def_or_impl(stmt),
                StmtKind::Trait { .. } => {}
                _ => self.lower_stmt(stmt),
            }
        }

        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Return, Span::default());
        }
        self.finish_function();
    }

    fn start_function(&mut self, name: String, arg_count: usize, ret_ty: Type) {
        self.current_name = name;
        self.current_locals.clear();
        self.current_blocks.clear();
        self.var_map.clear();
        self.loop_stack.clear();
        self.current_arg_count = arg_count;
        self.enter_scope();

        let start_bb = self.new_block();
        self.current_block = Some(start_bb);

        // _0 is return
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

    fn finish_function(&mut self) {
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
        // remove existing to let new one take precedence
        self.functions.retain(|f| f.name != func.name);
        self.functions.push(func);
    }

    fn register_fn_meta(&mut self, name: &str, params: &[Param]) {
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
        self.fn_meta.insert(
            name.to_string(),
            FnMeta {
                param_names,
                vararg_idx,
                kwarg_idx,
            },
        );
    }

    // Pack and reorder call arguments for functions with vararg/kwarg/keyword params.
    // arg_ops: lowered operands in order. arg_kw_names: Some(name) if keyword arg.
    fn pack_fn_call_args(
        &mut self,
        fn_name: &str,
        arg_ops: &[Operand],
        arg_kw_names: &[Option<String>],
        span: Span,
    ) -> Vec<Operand> {
        let meta = match self.fn_meta.get(fn_name).cloned() {
            Some(m) => m,
            None => return arg_ops.to_vec(),
        };

        let param_names = &meta.param_names;
        let vararg_idx = meta.vararg_idx;
        let kwarg_idx = meta.kwarg_idx;

        // If no vararg/kwarg and no keyword args used, just return as-is
        if vararg_idx.is_none() && kwarg_idx.is_none() && arg_kw_names.iter().all(|k| k.is_none()) {
            return arg_ops.to_vec();
        }

        let regular_end = vararg_idx.or(kwarg_idx).unwrap_or(param_names.len());

        // Separate positional and keyword (name, op) pairs
        let mut positional: Vec<Operand> = Vec::new();
        let mut keyword: Vec<(String, Operand)> = Vec::new();
        for (op, kw) in arg_ops.iter().zip(arg_kw_names.iter()) {
            match kw {
                Some(name) => keyword.push((name.clone(), op.clone())),
                None => positional.push(op.clone()),
            }
        }

        let mut result: Vec<Option<Operand>> = vec![None; param_names.len()];

        // Place positional args into regular slots
        let mut pos_consumed = 0;
        for (i, slot) in result.iter_mut().enumerate().take(regular_end) {
            if Some(i) == vararg_idx || Some(i) == kwarg_idx {
                continue;
            }
            if pos_consumed < positional.len() {
                *slot = Some(positional[pos_consumed].clone());
                pos_consumed += 1;
            }
        }

        // Place keyword args by name match into regular slots
        for (kw_name, kw_op) in &keyword {
            if let Some(pos) = param_names.iter().position(|n| n == kw_name)
                && Some(pos) != vararg_idx
                && Some(pos) != kwarg_idx
                && pos < regular_end
            {
                result[pos] = Some(kw_op.clone());
            }
        }

        // Pack extra positional args into vararg list
        if let Some(vi) = vararg_idx {
            let extra: Vec<Operand> = positional[pos_consumed..].to_vec();
            let list_tmp = self.new_local(Type::List(Box::new(Type::Any)), None, false);
            self.push_statement(
                StatementKind::Assign(list_tmp, Rvalue::Aggregate(AggregateKind::List, extra)),
                span,
            );
            result[vi] = Some(self.operand_for_local(list_tmp));
        }

        // Pack unmatched keyword args into kwarg dict
        if let Some(ki) = kwarg_idx {
            let extra_kw: Vec<Operand> = keyword
                .iter()
                .filter(|(kw_name, _)| {
                    param_names
                        .iter()
                        .position(|n| n == kw_name)
                        .map(|p| p == ki || p >= regular_end)
                        .unwrap_or(true)
                })
                .flat_map(|(kw_name, kw_op)| {
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

        result
            .into_iter()
            .map(|op| op.unwrap_or(Operand::Constant(Constant::Int(0))))
            .collect()
    }

    fn enter_scope(&mut self) {
        self.var_map.push(HashMap::default());
        self.scope_locals.push(Vec::new());
    }

    fn leave_scope(&mut self) {
        if let Some(locals) = self.scope_locals.pop() {
            for local in locals.into_iter().rev() {
                let ty = self.current_locals[local.0].ty.clone();
                if ty.is_move_type() {
                    self.push_statement(StatementKind::Drop(local), Span::default());
                }
                self.push_statement(StatementKind::StorageDead(local), Span::default());
            }
        }
        self.var_map.pop();
    }

    fn get_type(&self, expr_id: usize) -> Type {
        self.expr_types.get(&expr_id).cloned().unwrap_or(Type::Any)
    }

    fn new_tmp_for_expr(&mut self, expr: &Expr) -> Local {
        let ty = self.get_type(expr.id);
        self.new_local(ty, None, true)
    }

    fn new_local(&mut self, ty: Type, name: Option<String>, is_mut: bool) -> Local {
        let id = self.current_locals.len();
        let local = Local(id);
        self.current_locals.push(LocalDecl {
            ty,
            name,
            span: Span::default(),
            is_mut,
        });
        self.push_statement(StatementKind::StorageLive(local), Span::default());
        if let Some(scope) = self.scope_locals.last_mut() {
            scope.push(local);
        }
        local
    }

    // Create a local not tracked by scope — ownership transferred to caller's variable.
    fn new_unscoped_local(&mut self, ty: Type) -> Local {
        let id = self.current_locals.len();
        let local = Local(id);
        self.current_locals.push(LocalDecl {
            ty,
            name: None,
            span: Span::default(),
            is_mut: true,
        });
        self.push_statement(StatementKind::StorageLive(local), Span::default());
        local
    }

    fn new_block(&mut self) -> BasicBlockId {
        let id = self.current_blocks.len();
        self.current_blocks.push(BasicBlock {
            statements: Vec::new(),
            terminator: None,
        });
        BasicBlockId(id)
    }

    fn terminate_block(&mut self, bb: BasicBlockId, kind: TerminatorKind, span: Span) {
        if let Some(block) = self.current_blocks.get_mut(bb.0)
            && block.terminator.is_none()
        {
            block.terminator = Some(Terminator { kind, span });
        }
    }

    fn push_statement(&mut self, kind: StatementKind, span: Span) {
        if let Some(bb) = self.current_block {
            self.current_blocks[bb.0]
                .statements
                .push(Statement { kind, span });
        }
    }

    fn declare_var(&mut self, name: String, ty: Type, is_mut: bool) -> Local {
        let local = self.new_local(ty, Some(name.clone()), is_mut);
        self.var_map.last_mut().unwrap().insert(name, local);
        local
    }

    fn lookup_var(&self, name: &str) -> Option<Local> {
        for scope in self.var_map.iter().rev() {
            if let Some(&local) = scope.get(name) {
                return Some(local);
            }
        }
        None
    }

    fn operand_for_local(&self, local: Local) -> Operand {
        if self.current_locals[local.0].ty.is_move_type() {
            Operand::Move(local)
        } else {
            Operand::Copy(local)
        }
    }

    fn is_terminated(&self) -> bool {
        self.current_block
            .and_then(|bb| self.current_blocks.get(bb.0))
            .is_none_or(|b| b.terminator.is_some())
    }

    fn lower_stmt(&mut self, stmt: &Stmt) {
        if self.is_terminated() {
            return;
        }

        match &stmt.kind {
            StmtKind::Let {
                name,
                value,
                is_mut,
                ..
            } => {
                let rval = self.lower_expr(value);
                let ty = self.get_type(value.id);
                let local = self.declare_var(name.clone(), ty, *is_mut);
                self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), stmt.span);
            }

            StmtKind::Const { name, value, .. } => {
                let rval = self.lower_expr(value);
                // inline literals
                if let Operand::Constant(_) = &rval {
                    self.globals.insert(name.clone(), rval);
                } else {
                    // fallback to local
                    let ty = self.get_type(value.id);
                    let local = self.declare_var(name.clone(), ty, false);
                    self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), stmt.span);
                }
            }

            StmtKind::ExprStmt(expr) => {
                let rval = self.lower_expr(expr);
                let tmp = self.new_local(Type::Any, None, true);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(rval)), expr.span);
            }

            StmtKind::Assign { target, value } => {
                self.lower_assign(target, value);
            }

            StmtKind::AugAssign { target, op, value } => {
                let bin_op = match op {
                    crate::parser::AugOp::Add => crate::parser::BinOp::Add,
                    crate::parser::AugOp::Sub => crate::parser::BinOp::Sub,
                    crate::parser::AugOp::Mul => crate::parser::BinOp::Mul,
                    crate::parser::AugOp::Div => crate::parser::BinOp::Div,
                    crate::parser::AugOp::Mod => crate::parser::BinOp::Mod,
                    crate::parser::AugOp::Pow => crate::parser::BinOp::Pow,
                    crate::parser::AugOp::Shl => crate::parser::BinOp::Shl,
                    crate::parser::AugOp::Shr => crate::parser::BinOp::Shr,
                };
                let lhs_op = self.lower_expr(target);
                let rhs_op = self.lower_expr(value);
                let tmp = self.new_local(Type::Any, None, true);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::BinaryOp(bin_op, lhs_op, rhs_op)),
                    stmt.span,
                );

                if let ExprKind::Identifier(name) = &target.kind
                    && let Some(local) = self.lookup_var(name)
                {
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(Operand::Copy(tmp))),
                        stmt.span,
                    );
                }
            }

            StmtKind::Return(Some(expr)) => {
                let rval = self.lower_expr(expr);
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(rval)),
                    stmt.span,
                );
                if let Some(bb) = self.current_block {
                    if let Some((_, _, exit_bb)) = self.memo_context {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit_bb },
                            stmt.span,
                        );
                    } else {
                        self.terminate_block(bb, TerminatorKind::Return, stmt.span);
                    }
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::Return(None) => {
                if let Some(bb) = self.current_block {
                    if let Some((_, _, exit_bb)) = self.memo_context {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit_bb },
                            stmt.span,
                        );
                    } else {
                        self.terminate_block(bb, TerminatorKind::Return, stmt.span);
                    }
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::If {
                condition,
                then_body,
                elif_clauses,
                else_body,
            } => {
                self.lower_if(condition, then_body, elif_clauses, else_body);
            }

            StmtKind::While {
                condition,
                body,
                else_body,
            } => {
                self.lower_while(condition, body, else_body);
            }

            StmtKind::For {
                target,
                iter,
                body,
                else_body,
            } => {
                self.lower_for(target, iter, body, else_body);
            }

            StmtKind::Break => {
                if let Some(ctx) = self.loop_stack.last() {
                    let exit = ctx.exit;
                    if let Some(bb) = self.current_block {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: exit },
                            Span::default(),
                        );
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Continue => {
                if let Some(ctx) = self.loop_stack.last() {
                    let header = ctx.header;
                    if let Some(bb) = self.current_block {
                        self.terminate_block(
                            bb,
                            TerminatorKind::Goto { target: header },
                            Span::default(),
                        );
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Fn { .. } => {
                self.lower_fn_def(stmt);
            }

            StmtKind::Trait { .. } => {}

            StmtKind::Impl {
                type_name, body, ..
            } => {
                for s in body {
                    if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                        let mangled_name = format!("{}::{}", type_name, fn_name);
                        let mut impl_stmt = s.clone();
                        if let StmtKind::Fn {
                            name: ref mut n, ..
                        } = impl_stmt.kind
                        {
                            *n = mangled_name;
                        }
                        self.lower_fn_def(&impl_stmt);
                    }
                }
            }

            StmtKind::Assert { test, msg } => {
                let test_op = self.lower_expr(test);
                if let Some(m) = msg {
                    self.lower_expr(m);
                }
                let pass_bb = self.new_block();
                let fail_bb = self.new_block();
                if let Some(bb) = self.current_block {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: test_op,
                            targets: vec![(1, pass_bb)],
                            otherwise: fail_bb,
                        },
                        test.span,
                    );
                }
                self.terminate_block(fail_bb, TerminatorKind::Unreachable, Span::default());
                self.current_block = Some(pass_bb);
            }

            StmtKind::Struct { name, fields, .. } => {
                // synthesize __init__
                if !fields.is_empty() {
                    let init_name = format!("{}::__init__", name);
                    let n_params = fields.len() + 1; // self + each field

                    // save state
                    let saved_name = std::mem::take(&mut self.current_name);
                    let saved_locals = std::mem::take(&mut self.current_locals);
                    let saved_blocks = std::mem::take(&mut self.current_blocks);
                    let saved_block = self.current_block.take();
                    let saved_var_map = std::mem::take(&mut self.var_map);
                    let saved_loop_stack = std::mem::take(&mut self.loop_stack);
                    let saved_arg_count = self.current_arg_count;

                    self.start_function(init_name, n_params, Type::Null);

                    // params
                    // Use Type::Any so self is not dropped at scope exit (caller owns the struct)
                    let self_local = self.new_local(Type::Any, Some("self".to_string()), false);
                    let mut field_locals = Vec::new();
                    for field in fields {
                        let field_ty = field
                            .type_ann
                            .as_ref()
                            .map(|ann| self.resolve_type_expr(ann))
                            .unwrap_or(Type::Any);
                        let fl = self.new_local(field_ty, Some(field.name.clone()), false);
                        field_locals.push((field.name.clone(), fl));
                    }

                    // emit attrs
                    for (field_name, fl) in &field_locals {
                        self.push_statement(
                            StatementKind::SetAttr(
                                Operand::Copy(self_local),
                                field_name.clone(),
                                Operand::Copy(*fl),
                            ),
                            Span::default(),
                        );
                    }

                    if let Some(bb) = self.current_block {
                        self.terminate_block(bb, TerminatorKind::Return, Span::default());
                    }

                    self.finish_function();

                    // restore state
                    self.current_name = saved_name;
                    self.current_locals = saved_locals;
                    self.current_blocks = saved_blocks;
                    self.current_block = saved_block;
                    self.var_map = saved_var_map;
                    self.loop_stack = saved_loop_stack;
                    self.current_arg_count = saved_arg_count;
                }
            }

            StmtKind::Pass | StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}
            StmtKind::Enum { name, variants, .. } => {
                for (i, variant) in variants.iter().enumerate() {
                    let mangled = format!("{}::{}", name, variant.name);
                    self.enum_variants.insert(mangled, (name.clone(), i));
                }
            }
        }
    }

    fn lower_assign(&mut self, target: &Expr, value: &Expr) {
        let rval = self.lower_expr(value);
        match &target.kind {
            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(rval)),
                        target.span,
                    );
                }
            }
            ExprKind::Attr { obj, attr } => {
                let obj_op = self.lower_expr_as_copy(obj);
                self.push_statement(
                    StatementKind::SetAttr(obj_op, attr.clone(), rval),
                    target.span,
                );
            }
            ExprKind::Index { obj, index } => {
                let obj_op = self.lower_expr_as_copy(obj);
                let idx_op = self.lower_expr(index);
                self.push_statement(StatementKind::SetIndex(obj_op, idx_op, rval), target.span);
            }
            ExprKind::Tuple(elems) => {
                // tuple unpacking
                let rhs_local = self.new_tmp_for_expr(value);
                self.push_statement(
                    StatementKind::Assign(rhs_local, Rvalue::Use(rval)),
                    value.span,
                );
                for (i, elem) in elems.iter().enumerate() {
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_tmp = self.new_tmp_for_expr(elem);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(rhs_local), idx_op),
                        ),
                        elem.span,
                    );
                    if let ExprKind::Identifier(name) = &elem.kind
                        && let Some(local) = self.lookup_var(name)
                    {
                        self.push_statement(
                            StatementKind::Assign(local, Rvalue::Use(Operand::Copy(elem_tmp))),
                            elem.span,
                        );
                    }
                }
            }
            _ => {
                let tmp = self.new_tmp_for_expr(target);
                self.push_statement(StatementKind::Assign(tmp, Rvalue::Use(rval)), target.span);
            }
        }
    }

    fn lower_fn_def(&mut self, stmt: &Stmt) {
        if let StmtKind::Fn {
            name,
            params,
            body,
            decorators,
            return_type,
            is_async,
            ..
        } = &stmt.kind
        {
            // Register metadata for nested function definitions too
            if !self.fn_meta.contains_key(name) {
                self.register_fn_meta(name, params);
            }

            let is_memo = decorators
                .iter()
                .any(|d| d.name == "memo" && !d.is_directive);

            // save state
            let saved_name = std::mem::take(&mut self.current_name);
            let saved_locals = std::mem::take(&mut self.current_locals);
            let saved_blocks = std::mem::take(&mut self.current_blocks);
            let saved_block = self.current_block.take();
            let saved_var_map = std::mem::take(&mut self.var_map);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);
            let saved_arg_count = self.current_arg_count;
            let saved_is_async = self.current_is_async;
            self.current_is_async = *is_async;

            let ret_ty = return_type
                .as_ref()
                .map(|ann| self.resolve_type_expr(ann))
                .unwrap_or(Type::Any);

            self.start_function(name.clone(), params.len(), ret_ty);

            // params as locals
            let mut param_locals = Vec::new();
            for param in params {
                let ty = param
                    .type_ann
                    .as_ref()
                    .map(|ann| self.resolve_type_expr(ann))
                    .unwrap_or(Type::Any);
                let local = self.declare_var(param.name.clone(), ty, param.is_mut);
                param_locals.push(local);
            }

            if is_memo {
                // memoization
                // cache
                let cache_tmp = self.new_local(Type::Any, Some("cache".to_string()), false);
                let fn_name_const = Operand::Constant(Constant::Str(name.clone()));

                let is_tuple_val = if param_locals.len() > 1 { 1 } else { 0 };
                // get/create cache
                self.push_statement(
                    StatementKind::Assign(
                        cache_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_memo_get".to_string(),
                            )),
                            args: vec![
                                fn_name_const,
                                Operand::Constant(Constant::Int(is_tuple_val)),
                            ],
                        },
                    ),
                    stmt.span,
                );

                // check cache
                let key = if param_locals.len() == 1 {
                    Operand::Copy(param_locals[0])
                } else {
                    // pack args
                    let tuple_tmp = self.new_local(Type::Any, None, false);
                    let ops = param_locals.iter().map(|l| Operand::Copy(*l)).collect();
                    self.push_statement(
                        StatementKind::Assign(
                            tuple_tmp,
                            Rvalue::Aggregate(AggregateKind::Tuple, ops),
                        ),
                        stmt.span,
                    );
                    Operand::Copy(tuple_tmp)
                };

                let (has_fn, get_fn, set_fn) = if param_locals.len() == 1 {
                    (
                        "__olive_cache_has",
                        "__olive_cache_get",
                        "__olive_cache_set",
                    )
                } else {
                    (
                        "__olive_cache_has_tuple",
                        "__olive_cache_get_tuple",
                        "__olive_cache_set_tuple",
                    )
                };

                let cond_tmp = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        cond_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(has_fn.to_string())),
                            args: vec![Operand::Copy(cache_tmp), key.clone()],
                        },
                    ),
                    stmt.span,
                );

                let body_bb = self.new_block();
                let return_bb = self.new_block();
                let exit_bb = self.new_block();

                self.memo_context = Some((Operand::Copy(cache_tmp), key.clone(), exit_bb));

                let cur_bb = self.current_block.unwrap();
                self.terminate_block(
                    cur_bb,
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(cond_tmp),
                        targets: vec![(1, return_bb)],
                        otherwise: body_bb,
                    },
                    stmt.span,
                );

                // cache hit
                self.current_block = Some(return_bb);
                let hit_tmp = self.new_local(Type::Any, Some("cache_hit".to_string()), false);
                self.push_statement(
                    StatementKind::Assign(
                        hit_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(get_fn.to_string())),
                            args: vec![Operand::Copy(cache_tmp), key.clone()],
                        },
                    ),
                    stmt.span,
                );
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(Operand::Copy(hit_tmp))),
                    stmt.span,
                );
                self.terminate_block(return_bb, TerminatorKind::Return, stmt.span);

                // body
                self.current_block = Some(body_bb);
                for s in body {
                    self.lower_stmt(s);
                }

                // fallthrough
                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Goto { target: exit_bb }, stmt.span);
                }

                // cache miss store
                self.current_block = Some(exit_bb);
                let (cache_val, key_val, _) = self.memo_context.as_ref().unwrap().clone();
                let res_local = Local(0); // return value
                let dummy = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        dummy,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(set_fn.to_string())),
                            args: vec![cache_val, key_val, Operand::Copy(res_local)],
                        },
                    ),
                    stmt.span,
                );
                self.terminate_block(exit_bb, TerminatorKind::Return, stmt.span);

                self.memo_context = None;
            } else {
                for (i, s) in body.iter().enumerate() {
                    if i == body.len() - 1
                        && let StmtKind::ExprStmt(e) = &s.kind
                    {
                        let rval = self.lower_expr(e);
                        self.push_statement(
                            StatementKind::Assign(Local(0), Rvalue::Use(rval)),
                            e.span,
                        );
                        if let Some(bb) = self.current_block {
                            self.terminate_block(bb, TerminatorKind::Return, e.span);
                        }
                        self.current_block = Some(self.new_block());
                        continue;
                    }
                    self.lower_stmt(s);
                }

                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Return, Span::default());
                }
            }

            self.finish_function();

            // restore state
            self.current_name = saved_name;
            self.current_locals = saved_locals;
            self.current_blocks = saved_blocks;
            self.current_block = saved_block;
            self.var_map = saved_var_map;
            self.loop_stack = saved_loop_stack;
            self.current_arg_count = saved_arg_count;
            self.current_is_async = saved_is_async;
        }
    }

    fn lower_fn_def_or_impl(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Fn { .. } => self.lower_fn_def(stmt),
            StmtKind::Impl {
                type_name, body, ..
            } => {
                let type_name = type_name.clone();
                let body = body.clone();
                for s in &body {
                    if let StmtKind::Fn { name: fn_name, .. } = &s.kind {
                        let mangled = format!("{}::{}", type_name, fn_name);
                        let mut impl_stmt = s.clone();
                        if let StmtKind::Fn {
                            name: ref mut n, ..
                        } = impl_stmt.kind
                        {
                            *n = mangled;
                        }
                        self.lower_fn_def(&impl_stmt);
                    }
                }
            }
            _ => {}
        }
    }

    fn resolve_type_expr(&self, expr: &crate::parser::TypeExpr) -> Type {
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
                "Any" => Type::Any,
                "Never" => Type::Never,
                _ => {
                    if let Some(Type::Enum(e)) = self.global_types.get(name) {
                        Type::Enum(e.clone())
                    } else {
                        Type::Struct(name.clone())
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
                _ => Type::Struct(name.clone()),
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
            ),
            TypeExprKind::Ref(inner) => Type::Ref(Box::new(self.resolve_type_expr(inner))),
            TypeExprKind::MutRef(inner) => Type::MutRef(Box::new(self.resolve_type_expr(inner))),
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
        }
    }

    fn lower_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        elif_clauses: &[(Expr, Vec<Stmt>)],
        else_body: &Option<Vec<Stmt>>,
    ) {
        let cond_op = self.lower_expr(condition);
        let then_bb = self.new_block();
        let merge_bb = self.new_block();

        let next_bb = if !elif_clauses.is_empty() || else_body.is_some() {
            self.new_block()
        } else {
            merge_bb
        };

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::SwitchInt {
                    discr: cond_op,
                    targets: vec![(1, then_bb)],
                    otherwise: next_bb,
                },
                condition.span,
            );
        }

        self.current_block = Some(then_bb);
        self.enter_scope();
        for s in then_body {
            self.lower_stmt(s);
        }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: merge_bb },
                Span::default(),
            );
        }

        let mut current_next = next_bb;
        for (elif_cond, elif_body) in elif_clauses {
            self.current_block = Some(current_next);
            let elif_op = self.lower_expr(elif_cond);
            let elif_then = self.new_block();
            let elif_next = self.new_block();

            self.terminate_block(
                current_next,
                TerminatorKind::SwitchInt {
                    discr: elif_op,
                    targets: vec![(1, elif_then)],
                    otherwise: elif_next,
                },
                elif_cond.span,
            );

            self.current_block = Some(elif_then);
            self.enter_scope();
            for s in elif_body {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: merge_bb },
                    Span::default(),
                );
            }
            current_next = elif_next;
        }

        if let Some(body) = else_body {
            self.current_block = Some(current_next);
            self.enter_scope();
            for s in body {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: merge_bb },
                    Span::default(),
                );
            }
        } else if current_next != merge_bb {
            self.terminate_block(
                current_next,
                TerminatorKind::Goto { target: merge_bb },
                Span::default(),
            );
        }

        self.current_block = Some(merge_bb);
    }

    fn lower_while(&mut self, condition: &Expr, body: &[Stmt], else_body: &Option<Vec<Stmt>>) {
        let header_bb = self.new_block();
        let body_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }

        self.current_block = Some(header_bb);
        let cond_op = self.lower_expr(condition);

        let else_bb = if else_body.is_some() {
            self.new_block()
        } else {
            exit_bb
        };

        self.terminate_block(
            header_bb,
            TerminatorKind::SwitchInt {
                discr: cond_op,
                targets: vec![(1, body_bb)],
                otherwise: else_bb,
            },
            condition.span,
        );

        self.loop_stack.push(LoopContext {
            header: header_bb,
            exit: exit_bb,
        });
        self.current_block = Some(body_bb);
        self.enter_scope();
        for s in body {
            self.lower_stmt(s);
        }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }
        self.loop_stack.pop();

        if let Some(eb) = else_body {
            self.current_block = Some(else_bb);
            self.enter_scope();
            for s in eb {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: exit_bb },
                    Span::default(),
                );
            }
        }

        self.current_block = Some(exit_bb);
    }

    fn lower_for(
        &mut self,
        target: &ForTarget,
        iter: &Expr,
        body: &[Stmt],
        else_body: &Option<Vec<Stmt>>,
    ) {
        // lowered as while loop over iterator
        let iter_expr_op = self.lower_expr(iter);
        let iter_local = self.new_local(Type::Any, Some("_iter_obj".to_string()), true);

        // __olive_iter()
        self.push_statement(
            StatementKind::Assign(
                iter_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_iter".to_string())),
                    args: vec![iter_expr_op],
                },
            ),
            iter.span,
        );

        let header_bb = self.new_block();
        let body_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }

        // check if next exists: __olive_has_next()
        self.current_block = Some(header_bb);
        let has_next = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                has_next,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_has_next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            iter.span,
        );

        let else_bb = if else_body.is_some() {
            self.new_block()
        } else {
            exit_bb
        };
        self.terminate_block(
            header_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(has_next),
                targets: vec![(1, body_bb)],
                otherwise: else_bb,
            },
            iter.span,
        );

        // Body.
        self.loop_stack.push(LoopContext {
            header: header_bb,
            exit: exit_bb,
        });
        self.current_block = Some(body_bb);
        self.enter_scope();

        // Get next value: __olive_next()
        let next_val = self.new_local(Type::Any, None, false);
        self.push_statement(
            StatementKind::Assign(
                next_val,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("__olive_next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            iter.span,
        );

        // Bind the loop variable.
        match target {
            ForTarget::Name(name, _) => {
                let local = self.declare_var(name.clone(), Type::Any, true);
                self.push_statement(
                    StatementKind::Assign(local, Rvalue::Use(Operand::Copy(next_val))),
                    iter.span,
                );
            }
            ForTarget::Tuple(names) => {
                for (i, (name, _)) in names.iter().enumerate() {
                    let local = self.declare_var(name.clone(), Type::Any, true);
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_tmp = self.new_local(Type::Any, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            elem_tmp,
                            Rvalue::GetIndex(Operand::Copy(next_val), idx_op),
                        ),
                        iter.span,
                    );
                    self.push_statement(
                        StatementKind::Assign(local, Rvalue::Use(Operand::Copy(elem_tmp))),
                        iter.span,
                    );
                }
            }
        }

        for s in body {
            self.lower_stmt(s);
        }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(
                bb,
                TerminatorKind::Goto { target: header_bb },
                Span::default(),
            );
        }
        self.loop_stack.pop();

        if let Some(eb) = else_body {
            self.current_block = Some(else_bb);
            self.enter_scope();
            for s in eb {
                self.lower_stmt(s);
            }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(
                    bb,
                    TerminatorKind::Goto { target: exit_bb },
                    Span::default(),
                );
            }
        }

        self.current_block = Some(exit_bb);
    }

    fn lower_expr(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            ExprKind::Integer(i) => Operand::Constant(Constant::Int(*i)),
            ExprKind::Float(f) => Operand::Constant(Constant::Float((*f).to_bits())),
            ExprKind::Str(s) => Operand::Constant(Constant::Str(s.clone())),
            ExprKind::FStr(exprs) => {
                if exprs.is_empty() {
                    return Operand::Constant(Constant::Str("".to_string()));
                }

                let mut current_res: Option<Operand> = None;

                for e in exprs {
                    let op = self.lower_expr(e);
                    let ty = self.get_type(e.id);

                    let str_op = if ty == Type::Str {
                        op
                    } else {
                        let tmp = self.new_local(Type::Str, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function("str".to_string())),
                                    args: vec![op],
                                },
                            ),
                            e.span,
                        );
                        self.operand_for_local(tmp)
                    };

                    if let Some(res) = current_res {
                        let tmp = self.new_local(Type::Str, None, true);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::BinaryOp(crate::parser::BinOp::Add, res, str_op),
                            ),
                            expr.span,
                        );
                        current_res = Some(Operand::Copy(tmp));
                    } else {
                        current_res = Some(str_op);
                    }
                }

                current_res.unwrap()
            }
            ExprKind::Bool(b) => Operand::Constant(Constant::Bool(*b)),

            ExprKind::Try(inner) => {
                let inner_op = self.lower_expr(inner);
                let tag_tmp = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(tag_tmp, Rvalue::GetTag(inner_op.clone())),
                    expr.span,
                );

                let success_bb = self.new_block();
                let error_bb = self.new_block();

                if let Some(bb) = self.current_block {
                    self.terminate_block(
                        bb,
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(tag_tmp),
                            targets: vec![(0, success_bb)],
                            otherwise: error_bb,
                        },
                        expr.span,
                    );
                }

                // Error branch: return the union directly
                self.current_block = Some(error_bb);
                self.push_statement(
                    StatementKind::Assign(Local(0), Rvalue::Use(inner_op.clone())),
                    expr.span,
                );
                self.terminate_block(error_bb, TerminatorKind::Return, expr.span);

                // Success branch: extract payload
                self.current_block = Some(success_bb);
                let payload_tmp = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        payload_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_enum_get".to_string(),
                            )),
                            args: vec![inner_op, Operand::Constant(Constant::Int(0))],
                        },
                    ),
                    expr.span,
                );

                Operand::Copy(payload_tmp)
            }

            // await: lower the future operand then call __olive_await to resolve it.
            // Full state-machine transform is deferred; this emits a runtime poll call.
            ExprKind::Await(inner) => {
                let inner_op = self.lower_expr(inner);
                let result_ty = self.get_type(expr.id);
                let tmp = self.new_local(result_ty, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_await".to_string(),
                            )),
                            args: vec![inner_op],
                        },
                    ),
                    expr.span,
                );
                Operand::Copy(tmp)
            }

            // async: block — wrap body in a closure-like future object.
            ExprKind::AsyncBlock(body) => {
                let tmp = self.new_local(Type::Any, None, false);
                // Lower body stmts; last value becomes the future payload.
                self.enter_scope();
                let mut last_op = Operand::Constant(Constant::None);
                for s in body {
                    self.lower_stmt(s);
                    if let StmtKind::ExprStmt(e) = &s.kind {
                        last_op = self.lower_expr(e);
                    }
                }
                self.leave_scope();
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(
                                "__olive_make_future".to_string(),
                            )),
                            args: vec![last_op],
                        },
                    ),
                    expr.span,
                );
                Operand::Copy(tmp)
            }

            ExprKind::Borrow(inner) => {
                let tmp = self.new_tmp_for_expr(expr);
                let rval = if let ExprKind::Identifier(name) = &inner.kind {
                    if let Some(local) = self.lookup_var(name) {
                        Rvalue::Ref(local)
                    } else {
                        let op = self.lower_expr(inner);
                        Rvalue::Use(op)
                    }
                } else {
                    let op = self.lower_expr(inner);
                    Rvalue::Use(op)
                };
                self.push_statement(StatementKind::Assign(tmp, rval), expr.span);
                self.operand_for_local(tmp)
            }

            ExprKind::MutBorrow(inner) => {
                let tmp = self.new_tmp_for_expr(expr);
                let rval = if let ExprKind::Identifier(name) = &inner.kind {
                    if let Some(local) = self.lookup_var(name) {
                        Rvalue::MutRef(local)
                    } else {
                        let op = self.lower_expr(inner);
                        Rvalue::Use(op)
                    }
                } else {
                    let op = self.lower_expr(inner);
                    Rvalue::Use(op)
                };
                self.push_statement(StatementKind::Assign(tmp, rval), expr.span);
                self.operand_for_local(tmp)
            }

            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    Operand::Copy(local)
                } else if let Some(global_op) = self.globals.get(name) {
                    global_op.clone()
                } else {
                    // Fallback: assume it's a function identifier
                    Operand::Constant(Constant::Function(name.clone()))
                }
            }

            ExprKind::BinOp { left, op, right } => {
                let l = self.lower_expr(left);
                let r = self.lower_expr(right);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::BinaryOp(op.clone(), l, r)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::UnaryOp { op, operand } => {
                let o = self.lower_expr(operand);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::UnaryOp(op.clone(), o)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Call { callee, args } => {
                let mut arg_ops = Vec::new();
                let mut arg_kw_names: Vec<Option<String>> = Vec::new();
                for arg in args {
                    match arg {
                        CallArg::Positional(e) | CallArg::Splat(e) | CallArg::KwSplat(e) => {
                            arg_ops.push(self.lower_expr(e));
                            arg_kw_names.push(None);
                        }
                        CallArg::Keyword(name, e) => {
                            arg_ops.push(self.lower_expr(e));
                            arg_kw_names.push(Some(name.clone()));
                        }
                    }
                }

                // Special case for built-in 'type()' function
                if let ExprKind::Identifier(name) = &callee.kind
                    && name == "type"
                    && !args.is_empty()
                {
                    let arg_expr = match &args[0] {
                        CallArg::Positional(e)
                        | CallArg::Keyword(_, e)
                        | CallArg::Splat(e)
                        | CallArg::KwSplat(e) => e,
                    };
                    let arg_ty = self.get_type(arg_expr.id);
                    let type_str = format!("<struct '{}'>", arg_ty);
                    return Operand::Constant(Constant::Str(type_str));
                }

                if let ExprKind::Identifier(name) = &callee.kind
                    && name == "len"
                    && !args.is_empty()
                {
                    let arg_expr = match &args[0] {
                        CallArg::Positional(e)
                        | CallArg::Keyword(_, e)
                        | CallArg::Splat(e)
                        | CallArg::KwSplat(e) => e,
                    };
                    let arg_ty = self.get_type(arg_expr.id);
                    let mut current_arg_ty = arg_ty;
                    while let Type::Ref(inner) | Type::MutRef(inner) = current_arg_ty {
                        current_arg_ty = *inner;
                    }

                    if current_arg_ty == Type::Str {
                        let arg_op = self.lower_expr(arg_expr);
                        let tmp = self.new_local(Type::Int, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_str_len".to_string(),
                                    )),
                                    args: vec![arg_op],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    } else if matches!(
                        current_arg_ty,
                        Type::List(_)
                            | Type::Tuple(_)
                            | Type::Set(_)
                            | Type::Dict(_, _)
                            | Type::Any
                    ) {
                        let arg_op = self.lower_expr(arg_expr);
                        let tmp = self.new_local(Type::Int, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_list_len".to_string(),
                                    )),
                                    args: vec![arg_op],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }
                }
                if let ExprKind::Identifier(name) = &callee.kind {
                    if let Some((enum_name, tag)) = self.enum_variants.get(name).cloned() {
                        let type_id = Self::enum_type_id(&enum_name);
                        let tmp = self.new_tmp_for_expr(expr);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Aggregate(
                                    AggregateKind::EnumVariant(type_id, tag),
                                    arg_ops,
                                ),
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }

                    if name == "list_new" && !args.is_empty() {
                        let arg_expr = match &args[0] {
                            CallArg::Positional(e)
                            | CallArg::Keyword(_, e)
                            | CallArg::Splat(e)
                            | CallArg::KwSplat(e) => e,
                        };
                        let arg_op = self.lower_expr(arg_expr);
                        let tmp = self.new_local(Type::List(Box::new(Type::Any)), None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_list_new".to_string(),
                                    )),
                                    args: vec![arg_op],
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }
                }

                // If the callee is an attribute access, it's a method call.
                if let ExprKind::Attr { obj, attr } = &callee.kind {
                    if let ExprKind::Identifier(name) = &obj.kind {
                        let obj_ty = self.get_type(obj.id);
                        // Only treat as module/enum-namespace if not a struct/self variable
                        let is_struct_var = matches!(obj_ty, Type::Struct(_) | Type::Any)
                            && self.lookup_var(name).is_some();
                        if !is_struct_var {
                            // Check if it's a module function call (math.sqrt())
                            let mangled = format!("{}::{}", name, attr);

                            // Check if it's an enum variant constructor
                            let variant_info = self.enum_variants.get(&mangled).cloned();
                            if let Some((enum_name, tag)) = variant_info {
                                let type_id = Self::enum_type_id(&enum_name);
                                let tmp = self.new_tmp_for_expr(expr);
                                self.push_statement(
                                    StatementKind::Assign(
                                        tmp,
                                        Rvalue::Aggregate(
                                            AggregateKind::EnumVariant(type_id, tag),
                                            arg_ops,
                                        ),
                                    ),
                                    expr.span,
                                );
                                return self.operand_for_local(tmp);
                            }

                            // If it's a namespaced function, lower it as a direct call
                            let callee_op = Operand::Constant(Constant::Function(mangled));
                            let tmp = self.new_tmp_for_expr(expr);
                            self.push_statement(
                                StatementKind::Assign(
                                    tmp,
                                    Rvalue::Call {
                                        func: callee_op,
                                        args: arg_ops,
                                    },
                                ),
                                expr.span,
                            );
                            return self.operand_for_local(tmp);
                        }
                    }

                    let obj_op = self.lower_expr_as_copy(obj);
                    let tmp = self.new_tmp_for_expr(expr);

                    // Prepend 'self' (the object) to the arguments.
                    let mut method_args = vec![obj_op];
                    method_args.extend(arg_ops);

                    // Special case for built-in .copy() method
                    if attr == "copy" {
                        self.push_statement(
                            StatementKind::Assign(
                                tmp,
                                Rvalue::Call {
                                    func: Operand::Constant(Constant::Function(
                                        "__olive_copy".to_string(),
                                    )),
                                    args: method_args,
                                },
                            ),
                            expr.span,
                        );
                        return self.operand_for_local(tmp);
                    }

                    let obj_ty = self.get_type(obj.id);
                    let mut method_name = attr.clone();

                    if let Type::Struct(struct_name) = obj_ty {
                        method_name = format!("{}::{}", struct_name, attr);
                    }

                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(method_name)),
                                args: method_args,
                            },
                        ),
                        expr.span,
                    );
                    return self.operand_for_local(tmp);
                }

                // If the callee is a Struct, this is a constructor call.
                let callee_ty = self.get_type(callee.id);
                if let Type::Struct(struct_name) = callee_ty {
                    // Unscoped: ownership transfers to the let-binding's local, avoid double-drop.
                    let obj_tmp = self.new_unscoped_local(self.get_type(expr.id));
                    self.push_statement(
                        StatementKind::Assign(
                            obj_tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_obj_new".to_string(),
                                )),
                                args: vec![],
                            },
                        ),
                        expr.span,
                    );

                    let init_name = format!("{}::__init__", struct_name);
                    let mut init_args = vec![Operand::Copy(obj_tmp)];
                    init_args.extend(arg_ops);

                    let init_res = self.new_tmp_for_expr(expr);
                    self.push_statement(
                        StatementKind::Assign(
                            init_res,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(init_name)),
                                args: init_args,
                            },
                        ),
                        expr.span,
                    );

                    return Operand::Copy(obj_tmp);
                }

                let func = self.lower_expr(callee);
                let tmp = self.new_tmp_for_expr(expr);
                let final_args = if let ExprKind::Identifier(fn_name) = &callee.kind {
                    self.pack_fn_call_args(fn_name, &arg_ops, &arg_kw_names, expr.span)
                } else {
                    arg_ops
                };
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func,
                            args: final_args,
                        },
                    ),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::List(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::List, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Tuple(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Tuple, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Set(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Set, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Dict(pairs) => {
                let mut ops = Vec::new();
                for (k, v) in pairs {
                    ops.push(self.lower_expr(k));
                    ops.push(self.lower_expr(v));
                }
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::Aggregate(AggregateKind::Dict, ops)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Attr { obj, attr } => {
                if let ExprKind::Identifier(name) = &obj.kind {
                    let obj_ty = self.get_type(obj.id);
                    // Only treat as module attr if obj is not a struct/self variable
                    let is_struct_or_self = matches!(obj_ty, Type::Struct(_) | Type::Any)
                        && self.lookup_var(name).is_some();
                    if !is_struct_or_self {
                        // Check if it's a module attribute (math.PI)
                        let mangled = format!("{}::{}", name, attr);
                        if let Some(local) = self.lookup_var(&mangled) {
                            let ty = self.current_locals[local.0].ty.clone();
                            return if ty.is_move_type() {
                                Operand::Move(local)
                            } else {
                                Operand::Copy(local)
                            };
                        }
                        if let Some(global_op) = self.globals.get(&mangled) {
                            return global_op.clone();
                        }
                        // Fallback: assume it's a function (math.sqrt)
                        return Operand::Constant(Constant::Function(mangled));
                    }
                }
                let o = self.lower_expr_as_copy(obj);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::GetAttr(o, attr.clone())),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::Index { obj, index } => {
                let obj_ty = self.get_type(obj.id);
                let mut current_obj_ty = obj_ty;
                while let Type::Ref(inner) | Type::MutRef(inner) = current_obj_ty {
                    current_obj_ty = *inner;
                }

                if current_obj_ty == Type::Str {
                    let o = self.lower_expr_as_copy(obj);
                    let i = self.lower_expr(index);
                    let tmp = self.new_local(Type::Any, None, false);
                    self.push_statement(
                        StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function(
                                    "__olive_str_get".to_string(),
                                )),
                                args: vec![o, i],
                            },
                        ),
                        expr.span,
                    );
                    return self.operand_for_local(tmp);
                }
                let o = self.lower_expr_as_copy(obj);
                let i = self.lower_expr(index);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(
                    StatementKind::Assign(tmp, Rvalue::GetIndex(o, i)),
                    expr.span,
                );
                self.operand_for_local(tmp)
            }

            ExprKind::ListComp { elt, clauses } => {
                let ty = self.get_type(expr.id);
                self.lower_comprehension(
                    None,
                    Some(elt),
                    clauses,
                    AggregateKind::List,
                    expr.span,
                    ty,
                )
            }
            ExprKind::SetComp { elt, clauses } => {
                let ty = self.get_type(expr.id);
                self.lower_comprehension(
                    None,
                    Some(elt),
                    clauses,
                    AggregateKind::Set,
                    expr.span,
                    ty,
                )
            }
            ExprKind::DictComp {
                key,
                value,
                clauses,
            } => {
                let ty = self.get_type(expr.id);
                self.lower_comprehension(
                    Some((key, value)),
                    None,
                    clauses,
                    AggregateKind::Dict,
                    expr.span,
                    ty,
                )
            }
            ExprKind::Match {
                expr: match_expr,
                cases,
            } => {
                let discr_op = self.lower_expr(match_expr);
                let discr_local = match discr_op {
                    Operand::Copy(l) | Operand::Move(l) => l,
                    _ => {
                        let tmp = self.new_local(self.get_type(match_expr.id), None, false);
                        self.push_statement(
                            StatementKind::Assign(tmp, Rvalue::Use(discr_op)),
                            match_expr.span,
                        );
                        tmp
                    }
                };

                let exit_bb = self.new_block();
                let result_ty = self.get_type(expr.id);
                let result_tmp = self.new_local(result_ty, None, false);

                for case in cases {
                    let success_bb = self.new_block();
                    let failure_bb = self.new_block();

                    let match_ty = self.get_type(match_expr.id);
                    self.lower_pattern(
                        &case.pattern,
                        discr_local,
                        &match_ty,
                        success_bb,
                        failure_bb,
                        expr.span,
                    );

                    self.current_block = Some(success_bb);
                    self.enter_scope();

                    let mut last_op = Operand::Constant(Constant::None);
                    if case.body.is_empty() {
                        self.push_statement(
                            StatementKind::Assign(result_tmp, Rvalue::Use(last_op)),
                            expr.span,
                        );
                    } else {
                        for (i, stmt) in case.body.iter().enumerate() {
                            if i == case.body.len() - 1 {
                                if let StmtKind::ExprStmt(e) = &stmt.kind {
                                    last_op = self.lower_expr(e);
                                } else {
                                    self.lower_stmt(stmt);
                                }
                                self.push_statement(
                                    StatementKind::Assign(result_tmp, Rvalue::Use(last_op.clone())),
                                    stmt.span,
                                );
                            } else {
                                self.lower_stmt(stmt);
                            }
                        }
                    }

                    self.terminate_block(
                        self.current_block.unwrap(),
                        TerminatorKind::Goto { target: exit_bb },
                        expr.span,
                    );
                    self.leave_scope();

                    self.current_block = Some(failure_bb);
                }

                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::Goto { target: exit_bb },
                    expr.span,
                );
                self.current_block = Some(exit_bb);
                Operand::Copy(result_tmp)
            }
        }
    }

    fn enum_type_id(enum_name: &str) -> i64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        enum_name.hash(&mut h);
        // Mask sign bit so the value fits both i64 and Cranelift's u128 switch entry.
        (h.finish() & 0x7FFF_FFFF_FFFF_FFFF) as i64
    }

    fn lower_pattern(
        &mut self,
        pattern: &MatchPattern,
        discr: Local,
        match_ty: &Type,
        success_bb: BasicBlockId,
        failure_bb: BasicBlockId,
        expr_span: Span,
    ) {
        match pattern {
            MatchPattern::Wildcard => {
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::Goto { target: success_bb },
                    expr_span,
                );
            }
            MatchPattern::Identifier(name) => {
                let binding_local = self.declare_var(name.clone(), match_ty.clone(), true);
                self.push_statement(
                    StatementKind::Assign(binding_local, Rvalue::Use(Operand::Copy(discr))),
                    expr_span,
                );
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::Goto { target: success_bb },
                    expr_span,
                );
            }
            MatchPattern::Variant(v_name, inner_patterns) => {
                // Resolve which enum owns this variant and what tag it has.
                // For Union types, also record the type_id so we can discriminate
                // between member enums at runtime.
                let resolved = match match_ty {
                    Type::Enum(enum_name) => {
                        let mangled = format!("{}::{}", enum_name, v_name);
                        self.enum_variants.get(&mangled).map(|(_, tag)| {
                            (
                                enum_name.clone(),
                                Self::enum_type_id(enum_name),
                                *tag as i64,
                            )
                        })
                    }
                    Type::Union(members) => members.iter().find_map(|ty| {
                        if let Type::Enum(en) = ty {
                            let mangled = format!("{}::{}", en, v_name);
                            self.enum_variants
                                .get(&mangled)
                                .map(|(_, tag)| (en.clone(), Self::enum_type_id(en), *tag as i64))
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };

                let (enum_name, type_id, tag_id) =
                    resolved.unwrap_or_else(|| (String::new(), 0, 0));

                // For union types, gate on type_id before checking the variant tag.
                let tag_check_start_bb = if matches!(match_ty, Type::Union(_)) {
                    let type_id_tmp = self.new_local(Type::Int, None, false);
                    self.push_statement(
                        StatementKind::Assign(type_id_tmp, Rvalue::GetTypeId(Operand::Copy(discr))),
                        expr_span,
                    );
                    let type_match_bb = self.new_block();
                    self.terminate_block(
                        self.current_block.unwrap(),
                        TerminatorKind::SwitchInt {
                            discr: Operand::Copy(type_id_tmp),
                            targets: vec![(type_id, type_match_bb)],
                            otherwise: failure_bb,
                        },
                        expr_span,
                    );
                    self.current_block = Some(type_match_bb);
                    type_match_bb
                } else {
                    self.current_block.unwrap()
                };

                // get tag
                let tag_tmp = self.new_local(Type::Int, None, false);
                self.push_statement(
                    StatementKind::Assign(tag_tmp, Rvalue::GetTag(Operand::Copy(discr))),
                    expr_span,
                );

                // switch on tag
                let variant_match_bb = self.new_block();
                self.terminate_block(
                    self.current_block.unwrap_or(tag_check_start_bb),
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(tag_tmp),
                        targets: vec![(tag_id, variant_match_bb)],
                        otherwise: failure_bb,
                    },
                    expr_span,
                );

                self.current_block = Some(variant_match_bb);

                // handle inner patterns
                if inner_patterns.is_empty() {
                    self.terminate_block(
                        variant_match_bb,
                        TerminatorKind::Goto { target: success_bb },
                        expr_span,
                    );
                } else {
                    let mangled = format!("{}::{}", enum_name, v_name);
                    let param_types = self
                        .global_types
                        .get(&mangled)
                        .and_then(|ty| {
                            if let Type::Fn(pts, _) = ty {
                                Some(pts.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| vec![Type::Any; inner_patterns.len()]);

                    let mut current_bb = variant_match_bb;
                    for (i, (p, p_ty)) in inner_patterns.iter().zip(param_types.iter()).enumerate()
                    {
                        self.current_block = Some(current_bb);
                        let val_tmp = self.new_local(p_ty.clone() as Type, None, false);
                        self.push_statement(
                            StatementKind::Assign(
                                val_tmp,
                                Rvalue::GetIndex(
                                    Operand::Copy(discr),
                                    Operand::Constant(Constant::Int(i as i64)),
                                ),
                            ),
                            expr_span,
                        );

                        let next_bb = if i == inner_patterns.len() - 1 {
                            success_bb
                        } else {
                            self.new_block()
                        };

                        self.lower_pattern(p, val_tmp, p_ty, next_bb, failure_bb, expr_span);
                        current_bb = next_bb;
                    }
                }
            }
            MatchPattern::Literal(lit_expr) => {
                let lit_op = self.lower_expr(lit_expr);
                let is_eq = self.new_local(Type::Bool, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        is_eq,
                        Rvalue::BinaryOp(BinOp::Eq, Operand::Copy(discr), lit_op),
                    ),
                    expr_span,
                );
                self.terminate_block(
                    self.current_block.unwrap(),
                    TerminatorKind::SwitchInt {
                        discr: Operand::Copy(is_eq),
                        targets: vec![(1, success_bb)],
                        otherwise: failure_bb,
                    },
                    expr_span,
                );
            }
        }
    }
    fn lower_expr_as_copy(&mut self, expr: &Expr) -> Operand {
        let op = self.lower_expr(expr);
        match op {
            Operand::Move(l) => Operand::Copy(l),
            _ => op,
        }
    }

    fn bind_for_target(&mut self, target: &ForTarget, val: Local, span: Span) {
        match target {
            ForTarget::Name(name, _) => {
                let local = self.declare_var(name.clone(), Type::Any, true);
                self.push_statement(
                    StatementKind::Assign(local, Rvalue::Use(Operand::Copy(val))),
                    span,
                );
            }
            ForTarget::Tuple(names) => {
                for (i, (name, _)) in names.iter().enumerate() {
                    let local = self.declare_var(name.clone(), Type::Any, true);
                    self.push_statement(
                        StatementKind::Assign(
                            local,
                            Rvalue::GetIndex(
                                Operand::Copy(val),
                                Operand::Constant(Constant::Int(i as i64)),
                            ),
                        ),
                        span,
                    );
                }
            }
        }
    }

    fn lower_comprehension(
        &mut self,
        elt: Option<(&Expr, &Expr)>,
        single_elt: Option<&Expr>,
        clauses: &[CompClause],
        aggregate_kind: AggregateKind,
        span: Span,
        result_ty: Type,
    ) -> Operand {
        let result_local = self.new_local(result_ty, None, true);
        self.push_statement(StatementKind::StorageLive(result_local), span);
        self.push_statement(
            StatementKind::Assign(
                result_local,
                Rvalue::Aggregate(aggregate_kind.clone(), vec![]),
            ),
            span,
        );

        self.lower_comp_clause(
            elt,
            single_elt,
            clauses,
            0,
            result_local,
            aggregate_kind,
            span,
        );

        Operand::Move(result_local)
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_comp_clause(
        &mut self,
        elt: Option<(&Expr, &Expr)>,
        single_elt: Option<&Expr>,
        clauses: &[CompClause],
        clause_idx: usize,
        result_local: Local,
        aggregate_kind: AggregateKind,
        span: Span,
    ) {
        if clause_idx == clauses.len() {
            if let Some((k_expr, v_expr)) = elt {
                let k = self.lower_expr(k_expr);
                let v = self.lower_expr(v_expr);
                let set_id = Operand::Constant(Constant::Function("__olive_obj_set".to_string()));
                let tmp = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: set_id,
                            args: vec![Operand::Copy(result_local), k, v],
                        },
                    ),
                    span,
                );
            } else if let Some(e_expr) = single_elt {
                let val = self.lower_expr(e_expr);
                let func_name = match aggregate_kind {
                    AggregateKind::Set => "__olive_set_add",
                    _ => "__olive_list_append",
                };
                let tmp = self.new_local(Type::Any, None, false);
                self.push_statement(
                    StatementKind::Assign(
                        tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(func_name.to_string())),
                            args: vec![Operand::Copy(result_local), val],
                        },
                    ),
                    span,
                );
            }
            return;
        }

        let clause = &clauses[clause_idx];
        let iter_op = self.lower_expr(&clause.iter);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let next_clause_bb = self.new_block();
        let exit_bb = self.new_block();

        // Initialize iterator
        let iter_local = self.new_local(Type::Any, None, true);
        self.push_statement(
            StatementKind::Assign(
                iter_local,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("iter".to_string())),
                    args: vec![iter_op],
                },
            ),
            span,
        );

        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: cond_bb },
            span,
        );

        self.current_block = Some(cond_bb);
        let has_next = self.new_local(Type::Bool, None, false);
        self.push_statement(
            StatementKind::Assign(
                has_next,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("has_next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            span,
        );
        self.terminate_block(
            cond_bb,
            TerminatorKind::SwitchInt {
                discr: Operand::Copy(has_next),
                targets: vec![(1, body_bb)],
                otherwise: exit_bb,
            },
            span,
        );

        self.current_block = Some(body_bb);
        let next_val = self.new_local(Type::Any, None, true);
        self.push_statement(
            StatementKind::Assign(
                next_val,
                Rvalue::Call {
                    func: Operand::Constant(Constant::Function("next".to_string())),
                    args: vec![Operand::Copy(iter_local)],
                },
            ),
            span,
        );

        self.bind_for_target(&clause.target, next_val, span);

        if let Some(cond_expr) = &clause.condition {
            let cond_val = self.lower_expr(cond_expr);
            self.terminate_block(
                self.current_block.unwrap(),
                TerminatorKind::SwitchInt {
                    discr: cond_val,
                    targets: vec![(1, next_clause_bb)],
                    otherwise: cond_bb,
                },
                span,
            );
        } else {
            self.terminate_block(
                self.current_block.unwrap(),
                TerminatorKind::Goto {
                    target: next_clause_bb,
                },
                span,
            );
        }

        self.current_block = Some(next_clause_bb);
        self.lower_comp_clause(
            elt,
            single_elt,
            clauses,
            clause_idx + 1,
            result_local,
            aggregate_kind,
            span,
        );
        self.terminate_block(
            self.current_block.unwrap(),
            TerminatorKind::Goto { target: cond_bb },
            span,
        );

        self.current_block = Some(exit_bb);
    }
}
