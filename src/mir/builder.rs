use super::ir::*;
use crate::span::Span;
use crate::parser::{Expr, ExprKind, Stmt, StmtKind, Program, CallArg, ForTarget};
use crate::semantic::types::Type;
use rustc_hash::FxHashMap as HashMap;

// Break/continue targets for loops.
struct LoopContext {
    header: BasicBlockId,
    exit: BasicBlockId,
}

pub struct MirBuilder<'a> {
    pub functions: Vec<MirFunction>,
    pub expr_types: &'a HashMap<usize, Type>,

    // State for the function currently being built
    current_name: String,
    current_locals: Vec<LocalDecl>,
    current_blocks: Vec<BasicBlock>,
    current_block: Option<BasicBlockId>,
    current_arg_count: usize,
    var_map: Vec<HashMap<String, Local>>,
    loop_stack: Vec<LoopContext>,
    pub class_hierarchy: HashMap<String, Vec<String>>,
}

impl<'a> MirBuilder<'a> {
    pub fn new(expr_types: &'a HashMap<usize, Type>) -> Self {
        Self {
            functions: Vec::new(),
            expr_types,
            current_name: String::new(),
            current_locals: Vec::new(),
            current_blocks: Vec::new(),
            current_block: None,
            current_arg_count: 0,
            var_map: Vec::new(),
            loop_stack: Vec::new(),
            class_hierarchy: HashMap::default(),
        }
    }

    pub fn build_program(&mut self, program: &Program) {
        self.start_function("__main__".to_string(), 0);

        for stmt in &program.stmts {
            match &stmt.kind {
                StmtKind::Fn { .. } => self.lower_fn_def(stmt),
                _ => self.lower_stmt(stmt),
            }
        }

        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Return, Span::default());
        }
        self.finish_function();
    }

    fn start_function(&mut self, name: String, arg_count: usize) {
        self.current_name = name;
        self.current_locals.clear();
        self.current_blocks.clear();
        self.var_map.clear();
        self.loop_stack.clear();
        self.current_arg_count = arg_count;
        self.enter_scope();

        // _0 is always the return value local.
        self.new_local(Type::Any, Some("_return".to_string()), true);

        let start_bb = self.new_block();
        self.current_block = Some(start_bb);
    }

    fn finish_function(&mut self) {
        self.leave_scope();
        let func = MirFunction {
            name: self.current_name.clone(),
            locals: std::mem::take(&mut self.current_locals),
            basic_blocks: std::mem::take(&mut self.current_blocks),
            arg_count: self.current_arg_count,
        };
        // If a function with this name already exists (e.g. from an import or redefinition),
        // remove it so that the new definition takes precedence.
        self.functions.retain(|f| f.name != func.name);
        self.functions.push(func);
    }

    fn enter_scope(&mut self) {
        self.var_map.push(HashMap::default());
    }

    fn leave_scope(&mut self) {
        if let Some(scope) = self.var_map.pop() {
            for (_, local) in scope {
                let ty = self.current_locals[local.0].ty.clone();
                if ty.is_move_type() {
                    self.push_statement(StatementKind::Drop(local), Span::default());
                }
                self.push_statement(StatementKind::StorageDead(local), Span::default());
            }
        }
    }

    fn get_type(&self, expr_id: usize) -> Type {
        self.expr_types.get(&expr_id).cloned().unwrap_or(Type::Any)
    }

    fn new_tmp_for_expr(&mut self, expr: &Expr) -> Local {
        let ty = self.get_type(expr.id);
        self.new_local(ty, None, true) // Temporaries are mutable
    }

    fn new_local(&mut self, ty: Type, name: Option<String>, is_mut: bool) -> Local {
        let id = self.current_locals.len();
        self.current_locals.push(LocalDecl {
            ty,
            name,
            span: Span::default(),
            is_mut,
        });
        Local(id)
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
            && block.terminator.is_none() {
                block.terminator = Some(Terminator { kind, span });
            }
    }

    fn push_statement(&mut self, kind: StatementKind, span: Span) {
        if let Some(bb) = self.current_block {
            self.current_blocks[bb.0].statements.push(Statement { kind, span });
        }
    }

    fn declare_var(&mut self, name: String, ty: Type, is_mut: bool) -> Local {
        let local = self.new_local(ty, Some(name.clone()), is_mut);
        self.push_statement(StatementKind::StorageLive(local), Span::default());
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

    // Check if the current block has a terminator.
    fn is_terminated(&self) -> bool {
        self.current_block
            .and_then(|bb| self.current_blocks.get(bb.0))
            .is_none_or(|b| b.terminator.is_some())
    }

    fn lower_stmt(&mut self, stmt: &Stmt) {
        if self.is_terminated() { return; }

        match &stmt.kind {
            StmtKind::Let { name, value, is_mut, .. } => {
                let rval = self.lower_expr(value);
                let ty = self.get_type(value.id);
                let local = self.declare_var(name.clone(), ty, *is_mut);
                self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), stmt.span);
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
                    crate::parser::AugOp::FloorDiv => crate::parser::BinOp::FloorDiv,
                    crate::parser::AugOp::Mod => crate::parser::BinOp::Mod,
                    crate::parser::AugOp::Pow => crate::parser::BinOp::Pow,
                };
                let lhs_op = self.lower_expr(target);
                let rhs_op = self.lower_expr(value);
                let tmp = self.new_local(Type::Any, None, true);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::BinaryOp(bin_op, lhs_op, rhs_op),
                ), stmt.span);
                // Write result back to target.
                if let ExprKind::Identifier(name) = &target.kind
                    && let Some(local) = self.lookup_var(name) {
                        self.push_statement(StatementKind::Assign(local, Rvalue::Use(Operand::Copy(tmp))), stmt.span);
                    }
            }

            StmtKind::Return(Some(expr)) => {
                let rval = self.lower_expr(expr);
                self.push_statement(StatementKind::Assign(Local(0), Rvalue::Use(rval)), stmt.span);
                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Return, Span::default());
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::Return(None) => {
                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Return, Span::default());
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::If { condition, then_body, elif_clauses, else_body } => {
                self.lower_if(condition, then_body, elif_clauses, else_body);
            }

            StmtKind::While { condition, body, else_body } => {
                self.lower_while(condition, body, else_body);
            }

            StmtKind::For { target, iter, body, else_body } => {
                self.lower_for(target, iter, body, else_body);
            }

            StmtKind::Break => {
                if let Some(ctx) = self.loop_stack.last() {
                    let exit = ctx.exit;
                    if let Some(bb) = self.current_block {
                        self.terminate_block(bb, TerminatorKind::Goto { target: exit }, Span::default());
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Continue => {
                if let Some(ctx) = self.loop_stack.last() {
                    let header = ctx.header;
                    if let Some(bb) = self.current_block {
                        self.terminate_block(bb, TerminatorKind::Goto { target: header }, Span::default());
                    }
                    self.current_block = Some(self.new_block());
                }
            }

            StmtKind::Fn { .. } => {
                self.lower_fn_def(stmt);
            }

            StmtKind::Raise(Some(expr)) => {
                self.lower_expr(expr);
                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::Unreachable, Span::default());
                }
                self.current_block = Some(self.new_block());
            }

            StmtKind::Assert { test, msg } => {
                let test_op = self.lower_expr(test);
                if let Some(m) = msg {
                    self.lower_expr(m);
                }
                // Assert is lowered as: if not test, unreachable (will be panic in codegen).
                let pass_bb = self.new_block();
                let fail_bb = self.new_block();
                if let Some(bb) = self.current_block {
                    self.terminate_block(bb, TerminatorKind::SwitchInt {
                        discr: test_op,
                        targets: vec![(1, pass_bb)],
                        otherwise: fail_bb,
                    }, test.span);
                }
                self.terminate_block(fail_bb, TerminatorKind::Unreachable, Span::default());
                self.current_block = Some(pass_bb);
            }

            StmtKind::Class { name, bases, body } => {
                let mut base_names = Vec::new();
                for base in bases {
                    if let ExprKind::Identifier(base_name) = &base.kind {
                        base_names.push(base_name.clone());
                    }
                }
                self.class_hierarchy.insert(name.clone(), base_names);

                // Lower a class by lowering all its methods as top-level functions with mangled names.
                for stmt in body {
                    if let StmtKind::Fn { name: fn_name, .. } = &stmt.kind {
                        let mangled_name = format!("{}::{}", name, fn_name);
                        let mut class_stmt = stmt.clone();
                        if let StmtKind::Fn { name: ref mut n, .. } = class_stmt.kind {
                            *n = mangled_name;
                        }
                        self.lower_fn_def(&class_stmt);
                    }
                }
            }

            // Pass, Import, FromImport, Try — no MIR effect for now.
            StmtKind::Pass | StmtKind::Raise(None)
            | StmtKind::Import(_) | StmtKind::FromImport { .. }
            | StmtKind::Try { .. } => {}
        }
    }

    fn lower_assign(&mut self, target: &Expr, value: &Expr) {
        let rval = self.lower_expr(value);
        match &target.kind {
            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    self.push_statement(StatementKind::Assign(local, Rvalue::Use(rval)), target.span);
                }
            }
            ExprKind::Attr { obj, attr } => {
                let obj_op = self.lower_expr(obj);
                self.push_statement(StatementKind::SetAttr(obj_op, attr.clone(), rval), target.span);
            }
            ExprKind::Index { obj, index } => {
                let obj_op = self.lower_expr(obj);
                let idx_op = self.lower_expr(index);
                self.push_statement(StatementKind::SetIndex(obj_op, idx_op, rval), target.span);
            }
            ExprKind::Tuple(elems) => {
                // Tuple unpacking assignment: store the RHS, then destructure.
                let rhs_local = self.new_tmp_for_expr(value);
                self.push_statement(StatementKind::Assign(rhs_local, Rvalue::Use(rval)), value.span);
                for (i, elem) in elems.iter().enumerate() {
                    let idx_op = Operand::Constant(Constant::Int(i as i64));
                    let elem_tmp = self.new_tmp_for_expr(elem);
                    self.push_statement(StatementKind::Assign(
                        elem_tmp,
                        Rvalue::GetIndex(Operand::Copy(rhs_local), idx_op),
                    ), elem.span);
                    if let ExprKind::Identifier(name) = &elem.kind
                        && let Some(local) = self.lookup_var(name) {
                            self.push_statement(StatementKind::Assign(
                                local,
                                Rvalue::Use(Operand::Copy(elem_tmp)),
                            ), elem.span);
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
        if let StmtKind::Fn { name, params, body, .. } = &stmt.kind {
            // Save builder state.
            let saved_name = std::mem::take(&mut self.current_name);
            let saved_locals = std::mem::take(&mut self.current_locals);
            let saved_blocks = std::mem::take(&mut self.current_blocks);
            let saved_block = self.current_block.take();
            let saved_var_map = std::mem::take(&mut self.var_map);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);
            let saved_arg_count = self.current_arg_count;

            self.start_function(name.clone(), params.len());

            // Declare parameters as locals.
            for param in params {
                self.declare_var(param.name.clone(), Type::Any, param.is_mut);
            }

            for s in body {
                self.lower_stmt(s);
            }

            if let Some(bb) = self.current_block {
                self.terminate_block(bb, TerminatorKind::Return, Span::default());
            }
            self.finish_function();

            // Restore builder state.
            self.current_name = saved_name;
            self.current_locals = saved_locals;
            self.current_blocks = saved_blocks;
            self.current_block = saved_block;
            self.var_map = saved_var_map;
            self.loop_stack = saved_loop_stack;
            self.current_arg_count = saved_arg_count;

            // Functions are currently resolved statically by name during codegen,
            // so we don't declare them as local variables containing function pointers yet.
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
            self.terminate_block(bb, TerminatorKind::SwitchInt {
                discr: cond_op,
                targets: vec![(1, then_bb)],
                otherwise: next_bb,
            }, condition.span);
        }

        self.current_block = Some(then_bb);
        self.enter_scope();
        for s in then_body { self.lower_stmt(s); }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, Span::default());
        }

        let mut current_next = next_bb;
        for (elif_cond, elif_body) in elif_clauses {
            self.current_block = Some(current_next);
            let elif_op = self.lower_expr(elif_cond);
            let elif_then = self.new_block();
            let elif_next = self.new_block();

            self.terminate_block(current_next, TerminatorKind::SwitchInt {
                discr: elif_op,
                targets: vec![(1, elif_then)],
                otherwise: elif_next,
            }, elif_cond.span);

            self.current_block = Some(elif_then);
            self.enter_scope();
            for s in elif_body { self.lower_stmt(s); }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, Span::default());
            }
            current_next = elif_next;
        }

        if let Some(body) = else_body {
            self.current_block = Some(current_next);
            self.enter_scope();
            for s in body { self.lower_stmt(s); }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(bb, TerminatorKind::Goto { target: merge_bb }, Span::default());
            }
        } else if current_next != merge_bb {
            self.terminate_block(current_next, TerminatorKind::Goto { target: merge_bb }, Span::default());
        }

        self.current_block = Some(merge_bb);
    }

    fn lower_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        else_body: &Option<Vec<Stmt>>,
    ) {
        let header_bb = self.new_block();
        let body_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: header_bb }, Span::default());
        }

        self.current_block = Some(header_bb);
        let cond_op = self.lower_expr(condition);

        let else_bb = if else_body.is_some() { self.new_block() } else { exit_bb };

        self.terminate_block(header_bb, TerminatorKind::SwitchInt {
            discr: cond_op,
            targets: vec![(1, body_bb)],
            otherwise: else_bb,
        }, condition.span);

        self.loop_stack.push(LoopContext { header: header_bb, exit: exit_bb });
        self.current_block = Some(body_bb);
        self.enter_scope();
        for s in body { self.lower_stmt(s); }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: header_bb }, Span::default());
        }
        self.loop_stack.pop();

        if let Some(eb) = else_body {
            self.current_block = Some(else_bb);
            self.enter_scope();
            for s in eb { self.lower_stmt(s); }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(bb, TerminatorKind::Goto { target: exit_bb }, Span::default());
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
        // For loops are lowered as a while loop over an iterator.
        // For now, we represent the iter as a local and synthesize the control flow.
        let iter_op = self.lower_expr(iter);
        let iter_local = self.new_local(Type::Any, Some("_iter".to_string()), true);
        self.push_statement(StatementKind::Assign(iter_local, Rvalue::Use(iter_op)), iter.span);

        let header_bb = self.new_block();
        let body_bb = self.new_block();
        let exit_bb = self.new_block();

        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: header_bb }, Span::default());
        }

        // Header: check if iterator has next.
        self.current_block = Some(header_bb);
        let has_next = self.new_local(Type::Bool, None, true);
        self.push_statement(StatementKind::Assign(
            has_next,
            Rvalue::Use(Operand::Copy(iter_local)),
        ), iter.span);

        let else_bb = if else_body.is_some() { self.new_block() } else { exit_bb };
        self.terminate_block(header_bb, TerminatorKind::SwitchInt {
            discr: Operand::Copy(has_next),
            targets: vec![(1, body_bb)],
            otherwise: else_bb,
        }, iter.span);

        // Body.
        self.loop_stack.push(LoopContext { header: header_bb, exit: exit_bb });
        self.current_block = Some(body_bb);
        self.enter_scope();

        // Bind the loop variable.
        match target {
            ForTarget::Name(name, _) => {
                self.declare_var(name.clone(), Type::Any, true); // loop var is mutable by default? or immutable? 
                                                                 // in python it's mutable. in olive let's make it mutable.
            }
            ForTarget::Tuple(names, _) => {
                for (name, _) in names {
                    self.declare_var(name.clone(), Type::Any, true);
                }
            }
        }

        for s in body { self.lower_stmt(s); }
        self.leave_scope();
        if let Some(bb) = self.current_block {
            self.terminate_block(bb, TerminatorKind::Goto { target: header_bb }, Span::default());
        }
        self.loop_stack.pop();

        if let Some(eb) = else_body {
            self.current_block = Some(else_bb);
            self.enter_scope();
            for s in eb { self.lower_stmt(s); }
            self.leave_scope();
            if let Some(bb) = self.current_block {
                self.terminate_block(bb, TerminatorKind::Goto { target: exit_bb }, Span::default());
            }
        }

        self.current_block = Some(exit_bb);
    }

    fn lower_expr(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            ExprKind::Integer(i) => Operand::Constant(Constant::Int(*i)),
            ExprKind::Float(f) => Operand::Constant(Constant::Float(*f)),
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
                        self.push_statement(StatementKind::Assign(
                            tmp,
                            Rvalue::Call {
                                func: Operand::Constant(Constant::Function("str".to_string())),
                                args: vec![op],
                            },
                        ), e.span);
                        Operand::Copy(tmp)
                    };
                    
                    if let Some(res) = current_res {
                        let tmp = self.new_local(Type::Str, None, true);
                        self.push_statement(StatementKind::Assign(
                            tmp,
                            Rvalue::BinaryOp(crate::parser::BinOp::Add, res, str_op),
                        ), expr.span);
                        current_res = Some(Operand::Copy(tmp));
                    } else {
                        current_res = Some(str_op);
                    }
                }
                
                current_res.unwrap()
            }
            ExprKind::Bool(b) => Operand::Constant(Constant::Bool(*b)),
            ExprKind::Null => Operand::Constant(Constant::Null),

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
                Operand::Copy(tmp)
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
                Operand::Copy(tmp)
            }

            ExprKind::Identifier(name) => {
                if let Some(local) = self.lookup_var(name) {
                    let ty = self.current_locals[local.0].ty.clone();
                    if ty.is_move_type() {
                        Operand::Move(local)
                    } else {
                        Operand::Copy(local)
                    }
                } else {
                    // Fallback: assume it's a function identifier
                    Operand::Constant(Constant::Function(name.clone()))
                }
            }

            ExprKind::BinOp { left, op, right } => {
                let l = self.lower_expr(left);
                let r = self.lower_expr(right);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::BinaryOp(op.clone(), l, r),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::UnaryOp { op, operand } => {
                let o = self.lower_expr(operand);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::UnaryOp(op.clone(), o),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::Call { callee, args } => {
                let mut arg_ops = Vec::new();
                for arg in args {
                    match arg {
                        CallArg::Positional(e) | CallArg::Keyword(_, e)
                        | CallArg::Splat(e) | CallArg::KwSplat(e) => {
                            arg_ops.push(self.lower_expr(e));
                        }
                    }
                }
                
                // Special case for built-in 'type()' function
                if let ExprKind::Identifier(name) = &callee.kind
                    && name == "type" && !args.is_empty() {
                        let arg_expr = match &args[0] {
                            CallArg::Positional(e) | CallArg::Keyword(_, e)
                            | CallArg::Splat(e) | CallArg::KwSplat(e) => e,
                        };
                        let arg_ty = self.get_type(arg_expr.id);
                        let type_str = format!("<class '{}'>", arg_ty);
                        return Operand::Constant(Constant::Str(type_str));
                }

                // If the callee is an attribute access, it's a method call.
                if let ExprKind::Attr { obj, attr } = &callee.kind {
                    let obj_op = self.lower_expr(obj);
                    let tmp = self.new_tmp_for_expr(expr);
                    
                    // Prepend 'self' (the object) to the arguments.
                    let mut method_args = vec![obj_op];
                    method_args.extend(arg_ops);

                    // Special case for built-in .copy() method
                    if attr == "copy" {
                        self.push_statement(StatementKind::Assign(
                            tmp,
                            Rvalue::Call { 
                                func: Operand::Constant(Constant::Function("__olive_copy".to_string())), 
                                args: method_args 
                            },
                        ), expr.span);
                        return Operand::Copy(tmp);
                    }

                    // Get the class type to resolve the method name.
                    let obj_ty = self.get_type(obj.id);
                    let mut method_name = attr.clone();

                    if let Type::Class(class_name) = obj_ty {
                        // Search in class and then in bases (static dispatch).
                        let mut queue = vec![class_name];
                        let mut seen = std::collections::HashSet::new();
                        while let Some(current) = queue.pop() {
                            if !seen.insert(current.clone()) { continue; }
                            
                            let mangled = format!("{}::{}", current, attr);
                            // We don't have a list of all functions here, so we assume it exists if it's a class.
                            // Real implementation would verify existence.
                            method_name = mangled;
                            break; 
                        }
                    }

                    self.push_statement(StatementKind::Assign(
                        tmp,
                        Rvalue::Call { 
                            func: Operand::Constant(Constant::Function(method_name)), 
                            args: method_args 
                        },
                    ), expr.span);
                    return Operand::Copy(tmp);
                }

                // If the callee is a Class, this is a constructor call.
                let callee_ty = self.get_type(callee.id);
                if let Type::Class(class_name) = callee_ty {
                    let obj_tmp = self.new_tmp_for_expr(expr);
                    self.push_statement(StatementKind::Assign(
                        obj_tmp,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function("__olive_obj_new".to_string())),
                            args: vec![],
                        },
                    ), expr.span);
                    
                    let init_name = format!("{}::__init__", class_name);
                    let mut init_args = vec![Operand::Copy(obj_tmp)];
                    init_args.extend(arg_ops);
                    
                    let init_res = self.new_tmp_for_expr(expr);
                    self.push_statement(StatementKind::Assign(
                        init_res,
                        Rvalue::Call {
                            func: Operand::Constant(Constant::Function(init_name)),
                            args: init_args,
                        },
                    ), expr.span);
                    
                    return Operand::Copy(obj_tmp);
                }

                let func = self.lower_expr(callee);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::Call { func, args: arg_ops },
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::List(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::Aggregate(AggregateKind::List, ops),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::Tuple(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::Aggregate(AggregateKind::Tuple, ops),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::Set(elems) => {
                let ops: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::Aggregate(AggregateKind::Set, ops),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::Dict(pairs) => {
                let mut ops = Vec::new();
                for (k, v) in pairs {
                    ops.push(self.lower_expr(k));
                    ops.push(self.lower_expr(v));
                }
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::Aggregate(AggregateKind::Dict, ops),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::Attr { obj, attr } => {
                let o = self.lower_expr(obj);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::GetAttr(o, attr.clone()),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::Index { obj, index } => {
                let o = self.lower_expr(obj);
                let i = self.lower_expr(index);
                let tmp = self.new_tmp_for_expr(expr);
                self.push_statement(StatementKind::Assign(
                    tmp,
                    Rvalue::GetIndex(o, i),
                ), expr.span);
                Operand::Copy(tmp)
            }

            ExprKind::Walrus { name, value } => {
                let val = self.lower_expr(value);
                let ty = self.get_type(value.id);
                let local = self.declare_var(name.clone(), ty, false); // Walrus is usually immutable? 
                                                                      // Actually walrus in python is just an assignment.
                                                                      // Let's make it immutable like 'let'.
                self.push_statement(StatementKind::Assign(local, Rvalue::Use(val)), expr.span);
                Operand::Copy(local)
            }

            // Comprehensions: lower as the expression body for now.
            // Full lowering would expand into loops; this is the correct type-level representation.
            ExprKind::ListComp { elt, .. } | ExprKind::SetComp { elt, .. } => {
                self.lower_expr(elt)
            }
            ExprKind::DictComp { key, value, .. } => {
                self.lower_expr(key);
                self.lower_expr(value)
            }
        }
    }
}
