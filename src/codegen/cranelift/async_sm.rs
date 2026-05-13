use super::imports::cl_type;
use super::{CraneliftCodegen, KIND_SM_FUTURE, POLL_PENDING, SmAwaitPoint};
use crate::mir::{Constant, Local, MirFunction, Operand, StatementKind, TerminatorKind};
use cranelift::prelude::*;
use cranelift_module::Module;
use rustc_hash::FxHashMap as HashMap;

impl<'a> CraneliftCodegen<'a> {
    pub(super) fn analyze_async_sm(func: &MirFunction) -> Option<Vec<SmAwaitPoint>> {
        let n_bbs = func.basic_blocks.len();
        let mut visited = vec![false; n_bbs];

        let mut queue = std::collections::VecDeque::new();
        queue.push_back(0usize);
        let mut order = Vec::new();
        while let Some(bb_idx) = queue.pop_front() {
            if visited[bb_idx] {
                continue;
            }
            visited[bb_idx] = true;
            order.push(bb_idx);
            let bb = &func.basic_blocks[bb_idx];
            if let Some(term) = &bb.terminator {
                match &term.kind {
                    TerminatorKind::Goto { target } => queue.push_back(target.0),
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets {
                            queue.push_back(t.0);
                        }
                        queue.push_back(otherwise.0);
                    }
                    _ => {}
                }
            }
        }
        let mut points = Vec::new();
        for bb_idx in order {
            let bb = &func.basic_blocks[bb_idx];
            for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
                if let StatementKind::Assign(
                    result_local,
                    crate::mir::Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        args,
                    },
                ) = &stmt.kind
                    && name == "__olive_await"
                    && let Some(sub_op) = args.first()
                {
                    let sub_local = match sub_op {
                        Operand::Copy(l) | Operand::Move(l) => *l,
                        _ => return None,
                    };
                    points.push(SmAwaitPoint {
                        bb_idx,
                        stmt_idx,
                        result_local: *result_local,
                        sub_future_local: sub_local,
                    });
                }
            }
        }
        if points.is_empty() {
            None
        } else {
            Some(points)
        }
    }

    pub(super) fn translate_async_sm_poll(
        &mut self,
        func: &MirFunction,
        await_points: &[SmAwaitPoint],
    ) {
        let poll_name = format!("{}__sm_poll", func.name);
        let poll_id = *self.func_ids.get(&poll_name).unwrap();
        let num_locals = func.locals.len();
        let n_awaits = await_points.len();
        let n_bbs = func.basic_blocks.len();
        let mf = MemFlags::trusted();

        let frame_off = |local: Local| -> i32 { ((local.0 + 2) * 8) as i32 };

        let mut ctx = self.module.make_context();
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.returns.push(AbiParam::new(types::I64));
        let mut bctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        let mut vars: HashMap<Local, Variable> = HashMap::default();
        for (i, decl) in func.locals.iter().enumerate() {
            vars.insert(Local(i), builder.declare_var(cl_type(&decl.ty)));
        }
        let frame_var = builder.declare_var(types::I64);

        let mut bb_awaits: Vec<Vec<usize>> = vec![Vec::new(); n_bbs];
        for (idx, ap) in await_points.iter().enumerate() {
            bb_awaits[ap.bb_idx].push(idx);
        }
        let n_segs: Vec<usize> = (0..n_bbs).map(|i| bb_awaits[i].len() + 1).collect();

        let entry_blk = builder.create_block();
        let dispatch_blk = builder.create_block();
        let done_blk = builder.create_block();
        let state_blks: Vec<Block> = (0..=n_awaits).map(|_| builder.create_block()).collect();
        let seg_blks: Vec<Vec<Block>> = (0..n_bbs)
            .map(|i| (0..n_segs[i]).map(|_| builder.create_block()).collect())
            .collect();

        builder.switch_to_block(entry_blk);
        builder.seal_block(entry_blk);
        builder.append_block_params_for_function_params(entry_blk);
        let frame_ptr = builder.block_params(entry_blk)[0];
        for (i, decl) in func.locals.iter().enumerate() {
            let ty = cl_type(&decl.ty);
            let z = if ty == types::F64 {
                builder.ins().f64const(0.0)
            } else {
                builder.ins().iconst(types::I64, 0)
            };
            builder.def_var(vars[&Local(i)], z);
        }
        builder.def_var(frame_var, frame_ptr);
        builder.switch_to_block(dispatch_blk);

        let frame_d = builder.use_var(frame_var);
        let state_val = builder.ins().load(types::I64, mf, frame_d, 0);
        let mut sw = cranelift::frontend::Switch::new();
        for (k, &blk) in state_blks.iter().enumerate() {
            sw.set_entry(k as u128, blk);
        }
        sw.emit(&mut builder, state_val, done_blk);
        for blk in &state_blks {
            builder.seal_block(*blk);
        }
        builder.seal_block(done_blk);

        builder.switch_to_block(state_blks[0]);
        {
            let frame_s = builder.use_var(frame_var);
            for i in 1..=func.arg_count {
                let local = Local(i);
                let ty = cl_type(&func.locals[i].ty);
                let val = builder.ins().load(ty, mf, frame_s, frame_off(local));
                builder.def_var(vars[&local], val);
            }
        }
        builder.ins().jump(seg_blks[0][0], &[]);

        for k in 1..=n_awaits {
            let ap = &await_points[k - 1];
            let seg_idx_in_bb = bb_awaits[ap.bb_idx]
                .iter()
                .position(|&i| i == k - 1)
                .unwrap();
            let resume_seg = seg_idx_in_bb + 1;

            builder.switch_to_block(state_blks[k]);
            {
                let frame_s = builder.use_var(frame_var);
                for i in 0..num_locals {
                    let local = Local(i);
                    let ty = cl_type(&func.locals[i].ty);
                    let val = builder.ins().load(ty, mf, frame_s, frame_off(local));
                    builder.def_var(vars[&local], val);
                }
                let sub_future = builder.ins().load(types::I64, mf, frame_s, 8);
                let sm_poll_id = *self.func_ids.get("__olive_sm_poll").unwrap();
                let sm_poll_ref = self.module.declare_func_in_func(sm_poll_id, builder.func);
                let poll_call = builder.ins().call(sm_poll_ref, &[sub_future]);
                let poll_result = builder.inst_results(poll_call)[0];

                let pend_c = builder.ins().iconst(types::I64, POLL_PENDING);
                let is_pend = builder.ins().icmp(IntCC::Equal, poll_result, pend_c);

                let pend_blk = builder.create_block();
                let cont_blk = builder.create_block();
                builder.ins().brif(is_pend, pend_blk, &[], cont_blk, &[]);

                builder.seal_block(pend_blk);
                builder.switch_to_block(pend_blk);
                builder.ins().return_(&[pend_c]);

                builder.seal_block(cont_blk);
                builder.switch_to_block(cont_blk);
                let frame_c = builder.use_var(frame_var);
                for i in 0..num_locals {
                    let local = Local(i);
                    let ty = cl_type(&func.locals[i].ty);
                    let val = builder.ins().load(ty, mf, frame_c, frame_off(local));
                    builder.def_var(vars[&local], val);
                }
                builder.def_var(vars[&ap.result_local], poll_result);
            }
            builder.ins().jump(seg_blks[ap.bb_idx][resume_seg], &[]);
        }

        for bb_i in 0..n_bbs {
            let bb = func.basic_blocks[bb_i].clone();
            for seg_j in 0..n_segs[bb_i] {
                builder.switch_to_block(seg_blks[bb_i][seg_j]);

                let start_stmt = if seg_j == 0 {
                    0
                } else {
                    await_points[bb_awaits[bb_i][seg_j - 1]].stmt_idx + 1
                };
                let (end_stmt, maybe_ap_idx) = if seg_j < bb_awaits[bb_i].len() {
                    let ap_idx = bb_awaits[bb_i][seg_j];
                    (await_points[ap_idx].stmt_idx, Some(ap_idx))
                } else {
                    (bb.statements.len(), None)
                };

                for stmt in &bb.statements[start_stmt..end_stmt] {
                    Self::translate_statement(
                        func,
                        &mut self.module,
                        &self.func_ids,
                        &self.string_ids,
                        &mut builder,
                        stmt,
                        &vars,
                    );
                }

                if let Some(ap_idx) = maybe_ap_idx {
                    let ap = &await_points[ap_idx];
                    let frame_sw = builder.use_var(frame_var);
                    for i in 0..num_locals {
                        let local = Local(i);
                        let val = builder.use_var(vars[&local]);
                        builder.ins().store(mf, val, frame_sw, frame_off(local));
                    }

                    let sub_fv = builder.use_var(vars[&ap.sub_future_local]);
                    builder.ins().store(mf, sub_fv, frame_sw, 8);
                    let new_st = builder.ins().iconst(types::I64, (ap_idx + 1) as i64);
                    builder.ins().store(mf, new_st, frame_sw, 0);
                    let pv = builder.ins().iconst(types::I64, POLL_PENDING);
                    builder.ins().return_(&[pv]);
                } else {
                    match bb.terminator.as_ref().map(|t| t.kind.clone()) {
                        Some(TerminatorKind::Return) => {
                            let ret_val = builder.use_var(vars[&Local(0)]);
                            let frame_r = builder.use_var(frame_var);
                            let done_s = builder.ins().iconst(types::I64, -1i64);
                            builder.ins().store(mf, done_s, frame_r, 0);
                            builder.ins().return_(&[ret_val]);
                        }
                        Some(TerminatorKind::Goto { target }) => {
                            builder.ins().jump(seg_blks[target.0][0], &[]);
                        }
                        Some(TerminatorKind::SwitchInt {
                            discr,
                            targets,
                            otherwise,
                        }) => {
                            let val = Self::translate_operand(
                                &mut builder,
                                &discr,
                                &vars,
                                &self.string_ids,
                                &mut self.module,
                            );
                            if targets.len() == 1 && targets[0].0 == 1 {
                                let cond = builder.ins().icmp_imm(IntCC::NotEqual, val, 0);
                                builder.ins().brif(
                                    cond,
                                    seg_blks[targets[0].1.0][0],
                                    &[],
                                    seg_blks[otherwise.0][0],
                                    &[],
                                );
                            } else {
                                let mut sw2 = cranelift::frontend::Switch::new();
                                for (v, t) in &targets {
                                    sw2.set_entry(*v as u128, seg_blks[t.0][0]);
                                }
                                sw2.emit(&mut builder, val, seg_blks[otherwise.0][0]);
                            }
                        }
                        Some(TerminatorKind::Unreachable) | None => {
                            builder.ins().trap(TrapCode::unwrap_user(1));
                        }
                    }
                }
            }
        }

        builder.switch_to_block(done_blk);
        let z = builder.ins().iconst(types::I64, 0);
        builder.ins().return_(&[z]);

        for seg_row in &seg_blks {
            for &blk in seg_row {
                builder.seal_block(blk);
            }
        }

        builder.finalize();
        self.module.define_function(poll_id, &mut ctx).unwrap();
    }

    pub(super) fn generate_sm_wrapper(&mut self, func: &MirFunction) {
        let poll_name = format!("{}__sm_poll", func.name);
        let poll_fn_id = *self.func_ids.get(&poll_name).unwrap();
        let num_locals = func.locals.len();
        let frame_size = ((num_locals + 2) * 8) as i64;

        let mut ctx = self.module.make_context();
        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            ctx.func.signature.params.push(AbiParam::new(cl_type(ty)));
        }
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut bctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        builder.append_block_params_for_function_params(entry);
        let params: Vec<Value> = builder.block_params(entry).to_vec();

        let mf = MemFlags::trusted();
        let alloc_id = *self.func_ids.get("__olive_alloc").unwrap();
        let alloc_ref = self.module.declare_func_in_func(alloc_id, builder.func);

        let fsz = builder.ins().iconst(types::I64, frame_size);
        let frame_call = builder.ins().call(alloc_ref, &[fsz]);
        let frame_ptr = builder.inst_results(frame_call)[0];

        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().store(mf, zero, frame_ptr, 0);

        for (i, &param) in params.iter().enumerate() {
            let offset = ((i + 3) * 8) as i32;
            builder.ins().store(mf, param, frame_ptr, offset);
        }

        let future_sz = builder.ins().iconst(types::I64, 32);
        let fut_call = builder.ins().call(alloc_ref, &[future_sz]);
        let fut_ptr = builder.inst_results(fut_call)[0];

        let kind_val = builder.ins().iconst(types::I64, KIND_SM_FUTURE);
        builder.ins().store(mf, kind_val, fut_ptr, 0);

        let poll_ref = self.module.declare_func_in_func(poll_fn_id, builder.func);
        let poll_addr = builder.ins().func_addr(types::I64, poll_ref);
        builder.ins().store(mf, poll_addr, fut_ptr, 8);
        builder.ins().store(mf, frame_ptr, fut_ptr, 16);

        builder.ins().return_(&[fut_ptr]);
        builder.finalize();

        let wrapper_id = *self.func_ids.get(&func.name).unwrap();
        self.module.define_function(wrapper_id, &mut ctx).unwrap();
    }

    pub(super) fn generate_async_wrapper(&mut self, func: &MirFunction) {
        let body_name = format!("{}__async_body", func.name);
        let body_func_id = *self.func_ids.get(&body_name).unwrap();

        let mut ctx = self.module.make_context();
        for i in 0..func.arg_count {
            let ty = &func.locals[i + 1].ty;
            ctx.func.signature.params.push(AbiParam::new(cl_type(ty)));
        }
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        builder.append_block_params_for_function_params(entry);
        let params: Vec<Value> = builder.block_params(entry).to_vec();

        let callback_size = 8i64 * (2 + func.arg_count as i64);

        let alloc_id = *self.func_ids.get("__olive_alloc").unwrap();
        let alloc_ref = self.module.declare_func_in_func(alloc_id, builder.func);
        let size_val = builder.ins().iconst(types::I64, callback_size);
        let call = builder.ins().call(alloc_ref, &[size_val]);
        let cb_ptr = builder.inst_results(call)[0];

        let body_ref = self.module.declare_func_in_func(body_func_id, builder.func);
        let fn_ptr_val = builder.ins().func_addr(types::I64, body_ref);
        let mf = MemFlags::new();
        builder.ins().store(mf, fn_ptr_val, cb_ptr, 0);

        let nargs_val = builder.ins().iconst(types::I64, func.arg_count as i64);
        builder.ins().store(mf, nargs_val, cb_ptr, 8);

        for (i, &arg) in params.iter().enumerate() {
            builder.ins().store(mf, arg, cb_ptr, 8 * (2 + i) as i32);
        }

        let spawn_id = *self.func_ids.get("__olive_spawn_task").unwrap();
        let spawn_ref = self.module.declare_func_in_func(spawn_id, builder.func);
        let call = builder.ins().call(spawn_ref, &[cb_ptr]);
        let future_ptr = builder.inst_results(call)[0];

        builder.ins().return_(&[future_ptr]);
        builder.finalize();

        let wrapper_id = *self.func_ids.get(&func.name).unwrap();
        self.module.define_function(wrapper_id, &mut ctx).unwrap();
    }
}
