mod lower_control;
mod lower_expr;
mod lower_stmt;

use super::ir::*;
use crate::mir::AggregateKind;
use crate::parser::{Expr, Param, ParamKind, Program, StmtKind};
use crate::semantic::types::Type;
use crate::span::Span;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone)]
pub(super) struct FnMeta {
    pub(super) param_names: Vec<String>,
    pub(super) vararg_idx: Option<usize>,
    pub(super) kwarg_idx: Option<usize>,
}

pub(super) struct LoopContext {
    pub(super) header: BasicBlockId,
    pub(super) exit: BasicBlockId,
}

pub struct MirBuilder<'a> {
    pub functions: Vec<MirFunction>,
    pub expr_types: &'a HashMap<usize, Type>,
    pub global_types: &'a HashMap<String, Type>,

    pub(super) current_name: String,
    pub(super) current_locals: Vec<LocalDecl>,
    pub(super) current_blocks: Vec<BasicBlock>,
    pub(super) current_block: Option<BasicBlockId>,
    pub(super) current_arg_count: usize,
    pub(super) var_map: Vec<HashMap<String, Local>>,
    pub(super) loop_stack: Vec<LoopContext>,
    pub(super) scope_locals: Vec<Vec<Local>>,
    pub(super) memo_context: Option<(Operand, Operand, BasicBlockId)>,
    pub globals: HashMap<String, Operand>,
    pub enum_variants: HashMap<String, (String, usize)>,
    pub(super) current_is_async: bool,
    pub(super) fn_meta: HashMap<String, FnMeta>,
    pub(super) generic_fns: HashMap<String, crate::parser::Stmt>,
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
            generic_fns: HashMap::default(),
        }
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

    pub(super) fn start_function(&mut self, name: String, arg_count: usize, ret_ty: Type) {
        self.current_name = name;
        self.current_locals.clear();
        self.current_blocks.clear();
        self.var_map.clear();
        self.loop_stack.clear();
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
        self.fn_meta.insert(
            name.to_string(),
            FnMeta {
                param_names,
                vararg_idx,
                kwarg_idx,
            },
        );
    }

    pub(super) fn pack_fn_call_args(
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

        if vararg_idx.is_none() && kwarg_idx.is_none() && arg_kw_names.iter().all(|k| k.is_none()) {
            return arg_ops.to_vec();
        }

        let regular_end = vararg_idx.or(kwarg_idx).unwrap_or(param_names.len());

        let mut positional: Vec<Operand> = Vec::new();
        let mut keyword: Vec<(String, Operand)> = Vec::new();
        for (op, kw) in arg_ops.iter().zip(arg_kw_names.iter()) {
            match kw {
                Some(name) => keyword.push((name.clone(), op.clone())),
                None => positional.push(op.clone()),
            }
        }

        let mut result: Vec<Option<Operand>> = vec![None; param_names.len()];

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

        for (kw_name, kw_op) in &keyword {
            if let Some(pos) = param_names.iter().position(|n| n == kw_name)
                && Some(pos) != vararg_idx
                && Some(pos) != kwarg_idx
                && pos < regular_end
            {
                result[pos] = Some(kw_op.clone());
            }
        }

        if let Some(vi) = vararg_idx {
            let extra: Vec<Operand> = positional[pos_consumed..].to_vec();
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

    pub(super) fn enter_scope(&mut self) {
        self.var_map.push(HashMap::default());
        self.scope_locals.push(Vec::new());
    }

    pub(super) fn leave_scope(&mut self) {
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

    pub(super) fn get_type(&self, expr_id: usize) -> Type {
        self.expr_types.get(&expr_id).cloned().unwrap_or(Type::Any)
    }

    pub(super) fn new_tmp_for_expr(&mut self, expr: &Expr) -> Local {
        let ty = self.get_type(expr.id);
        self.new_local(ty, None, true)
    }

    pub(super) fn new_local(&mut self, ty: Type, name: Option<String>, is_mut: bool) -> Local {
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

    pub(super) fn new_unscoped_local(&mut self, ty: Type) -> Local {
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

    pub(super) fn lookup_var(&self, name: &str) -> Option<Local> {
        for scope in self.var_map.iter().rev() {
            if let Some(&local) = scope.get(name) {
                return Some(local);
            }
        }
        None
    }

    pub(super) fn operand_for_local(&self, local: Local) -> Operand {
        if self.current_locals[local.0].ty.is_move_type() {
            Operand::Move(local)
        } else {
            Operand::Copy(local)
        }
    }

    pub(super) fn is_terminated(&self) -> bool {
        self.current_block
            .and_then(|bb| self.current_blocks.get(bb.0))
            .is_none_or(|b| b.terminator.is_some())
    }
}
