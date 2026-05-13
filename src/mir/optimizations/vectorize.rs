#![allow(dead_code)]
use crate::mir::loop_utils;
use crate::mir::optimizations::Transform;
use crate::mir::*;
use crate::semantic::types::Type as OliveType;
use crate::span::Span;
use rustc_hash::FxHashMap;

pub struct LoopVectorizer;

impl Transform for LoopVectorizer {
    fn name(&self) -> &'static str {
        "vectorize"
    }

    fn run(&self, func: &mut MirFunction) -> bool {
        let loops = loop_utils::find_loops(func);
        for lp in loops {
            if self.try_vectorize(func, &lp) {
                return true;
            }
        }
        false
    }
}

struct VectorizationPlan {
    induction: Local,
    limit: Operand,
    width: usize,
    loads: Vec<(Local, Operand)>, // (dest, collection) for GetIndex(collection, i)
    stores: Vec<(Operand, Operand)>, // (collection, value) for SetIndex(collection, i, value)
}

impl LoopVectorizer {
    fn try_vectorize(&self, func: &mut MirFunction, lp: &loop_utils::Loop) -> bool {
        if let Some(plan) = self.analyze(func, lp) {
            self.transform(func, lp, &plan)
        } else {
            false
        }
    }

    fn analyze(&self, func: &MirFunction, lp: &loop_utils::Loop) -> Option<VectorizationPlan> {
        for &bb_id in &lp.body {
            for stmt in &func.basic_blocks[bb_id.0].statements {
                match &stmt.kind {
                    StatementKind::Assign(_, Rvalue::VectorLoad(..))
                    | StatementKind::Assign(_, Rvalue::VectorSplat(..))
                    | StatementKind::Assign(_, Rvalue::VectorFMA(..))
                    | StatementKind::VectorStore(..) => return None,
                    _ => {}
                }
            }
        }

        let mut induction = None;
        for &latch_id in &lp.latches {
            let latch = &func.basic_blocks[latch_id.0];
            for stmt in &latch.statements {
                if let StatementKind::Assign(
                    local,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(src),
                        Operand::Constant(Constant::Int(1)),
                    ),
                ) = &stmt.kind
                {
                    if *src == *local {
                        if induction.is_some() {
                            return None;
                        }
                        induction = Some(*local);
                    }
                }
            }
        }
        let i = induction?;

        let header = &func.basic_blocks[lp.header.0];
        if !matches!(
            header.terminator.as_ref().map(|t| &t.kind),
            Some(TerminatorKind::SwitchInt { .. })
        ) {
            return None;
        }

        let mut limit = None;
        for stmt in header.statements.iter().rev() {
            if let StatementKind::Assign(
                _,
                Rvalue::BinaryOp(crate::parser::BinOp::Lt, Operand::Copy(idx), lim),
            ) = &stmt.kind
            {
                if *idx == i {
                    limit = Some(lim.clone());
                    break;
                }
            }
        }
        let limit = limit?;
        let mut loads = Vec::new();
        let mut stores = Vec::new();

        for &bb_id in &lp.body {
            for stmt in &func.basic_blocks[bb_id.0].statements {
                match &stmt.kind {
                    StatementKind::Assign(dest, Rvalue::GetIndex(obj, Operand::Copy(idx)))
                        if *idx == i =>
                    {
                        loads.push((*dest, obj.clone()));
                    }
                    StatementKind::SetIndex(obj, Operand::Copy(idx), _) if *idx == i => {
                        stores.push((obj.clone(), Operand::Copy(*idx)));
                    }
                    _ => {}
                }
            }
        }

        if loads.is_empty() {
            return None;
        }

        if lp.exits.len() > 1 {
            return None;
        }

        Some(VectorizationPlan {
            induction: i,
            limit,
            width: 4,
            loads,
            stores,
        })
    }

    // vectorize loop
    fn transform(
        &self,
        func: &mut MirFunction,
        lp: &loop_utils::Loop,
        plan: &VectorizationPlan,
    ) -> bool {
        let i = plan.induction;
        let width = plan.width;

        // Step 1: Clone the original loop body to serve as the scalar epilogue.
        let epilogue_map = loop_utils::clone_blocks(func, &lp.body);
        let epilogue_header = match epilogue_map.get(&lp.header) {
            Some(&h) => h,
            None => return false,
        };

        let vec_limit_local = Local(func.locals.len());
        func.locals.push(LocalDecl {
            ty: OliveType::Int,
            name: Some("vec_limit".into()),
            span: Span::default(),
            is_mut: false,
        });

        let pre_header_id = BasicBlockId(func.basic_blocks.len());
        func.basic_blocks.push(BasicBlock {
            statements: vec![Statement {
                kind: StatementKind::Assign(
                    vec_limit_local,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Sub,
                        plan.limit.clone(),
                        Operand::Constant(Constant::Int((width - 1) as i64)),
                    ),
                ),
                span: Span::default(),
            }],
            terminator: Some(Terminator {
                kind: TerminatorKind::Goto { target: lp.header },
                span: Span::default(),
            }),
        });

        // Redirect external predecessors of the loop header to the pre-header.
        for bb_idx in 0..pre_header_id.0 {
            let bb_id = BasicBlockId(bb_idx);
            if lp.body.contains(&bb_id) {
                continue;
            }
            if epilogue_map.values().any(|&v| v == bb_id) {
                continue;
            }
            let bb = &mut func.basic_blocks[bb_idx];
            if let Some(term) = &mut bb.terminator {
                match &mut term.kind {
                    TerminatorKind::Goto { target } if *target == lp.header => {
                        *target = pre_header_id;
                    }
                    TerminatorKind::SwitchInt {
                        targets, otherwise, ..
                    } => {
                        for (_, t) in targets.iter_mut() {
                            if *t == lp.header {
                                *t = pre_header_id;
                            }
                        }
                        if *otherwise == lp.header {
                            *otherwise = pre_header_id;
                        }
                    }
                    _ => {}
                }
            }
        }

        let cond_local = {
            let header = &func.basic_blocks[lp.header.0];
            let mut found = None;
            for stmt in &header.statements {
                if let StatementKind::Assign(
                    local,
                    Rvalue::BinaryOp(crate::parser::BinOp::Lt, Operand::Copy(idx), _),
                ) = &stmt.kind
                {
                    if *idx == i {
                        found = Some(*local);
                        break;
                    }
                }
            }
            found
        };

        if let Some(cond_local) = cond_local {
            let header = &mut func.basic_blocks[lp.header.0];
            for stmt in &mut header.statements {
                if let StatementKind::Assign(l, _) = &stmt.kind {
                    if *l == cond_local {
                        stmt.kind = StatementKind::Assign(
                            cond_local,
                            Rvalue::BinaryOp(
                                crate::parser::BinOp::Lt,
                                Operand::Copy(i),
                                Operand::Copy(vec_limit_local),
                            ),
                        );
                        break;
                    }
                }
            }
            if let Some(term) = &mut header.terminator {
                if let TerminatorKind::SwitchInt { otherwise, .. } = &mut term.kind {
                    *otherwise = epilogue_header;
                }
            }
        } else {
            return false;
        }

        let mut vector_locals: FxHashMap<Local, Local> = FxHashMap::default();
        let load_set: FxHashMap<Local, Operand> = plan.loads.iter().cloned().collect();

        for &bb_id in &lp.body {
            let mut new_stmts = Vec::new();
            let old_stmts = std::mem::take(&mut func.basic_blocks[bb_id.0].statements);

            for stmt in old_stmts {
                match &stmt.kind {
                    // Linear load → VectorLoad
                    StatementKind::Assign(dest, Rvalue::GetIndex(obj, Operand::Copy(idx)))
                        if *idx == i && load_set.contains_key(dest) =>
                    {
                        let v = self.alloc_vector_local(func, *dest, width);
                        vector_locals.insert(*dest, v);
                        new_stmts.push(Statement {
                            kind: StatementKind::Assign(
                                v,
                                Rvalue::VectorLoad(obj.clone(), Operand::Copy(i), width),
                            ),
                            span: stmt.span,
                        });
                    }

                    // BinaryOp where both operands have vector versions
                    StatementKind::Assign(
                        dest,
                        Rvalue::BinaryOp(op, Operand::Copy(l), Operand::Copy(r)),
                    ) if vector_locals.contains_key(l) || vector_locals.contains_key(r) => {
                        let vl = self.ensure_vector(
                            func,
                            *l,
                            width,
                            &mut vector_locals,
                            &mut new_stmts,
                            stmt.span,
                        );
                        let vr = self.ensure_vector(
                            func,
                            *r,
                            width,
                            &mut vector_locals,
                            &mut new_stmts,
                            stmt.span,
                        );
                        let v = self.alloc_vector_local(func, *dest, width);
                        vector_locals.insert(*dest, v);
                        new_stmts.push(Statement {
                            kind: StatementKind::Assign(
                                v,
                                Rvalue::BinaryOp(op.clone(), Operand::Copy(vl), Operand::Copy(vr)),
                            ),
                            span: stmt.span,
                        });
                    }

                    // Linear store → VectorStore
                    StatementKind::SetIndex(obj, Operand::Copy(idx), Operand::Copy(val))
                        if *idx == i =>
                    {
                        if let Some(&vval) = vector_locals.get(val) {
                            new_stmts.push(Statement {
                                kind: StatementKind::VectorStore(
                                    obj.clone(),
                                    Operand::Copy(i),
                                    Operand::Copy(vval),
                                ),
                                span: stmt.span,
                            });
                        } else {
                            new_stmts.push(stmt);
                        }
                    }

                    _ => new_stmts.push(stmt),
                }
            }
            func.basic_blocks[bb_id.0].statements = new_stmts;
        }

        // FMA fusion: a*b+c or c+a*b → VectorFMA(a, b, c)
        for &bb_id in &lp.body {
            Self::fuse_fma(&mut func.basic_blocks[bb_id.0].statements);
        }

        for &latch_id in &lp.latches {
            let latch = &mut func.basic_blocks[latch_id.0];
            for stmt in &mut latch.statements {
                if let StatementKind::Assign(
                    local,
                    Rvalue::BinaryOp(
                        crate::parser::BinOp::Add,
                        Operand::Copy(src),
                        Operand::Constant(Constant::Int(1)),
                    ),
                ) = &mut stmt.kind
                {
                    if *local == i && *src == i {
                        stmt.kind = StatementKind::Assign(
                            i,
                            Rvalue::BinaryOp(
                                crate::parser::BinOp::Add,
                                Operand::Copy(i),
                                Operand::Constant(Constant::Int(width as i64)),
                            ),
                        );
                    }
                }
            }
        }

        true
    }

    fn fuse_fma(stmts: &mut Vec<Statement>) {
        let mut i = 0;
        while i + 1 < stmts.len() {
            // match: tmp = Mul(va, vb)
            let mul_info = match &stmts[i].kind {
                StatementKind::Assign(
                    dest,
                    Rvalue::BinaryOp(crate::parser::BinOp::Mul, va, vb),
                ) => Some((*dest, va.clone(), vb.clone())),
                _ => None,
            };
            if let Some((mul_dest, va, vb)) = mul_info {
                // match: result = Add(Copy(mul_dest), vc) or Add(vc, Copy(mul_dest))
                let fma_info = match &stmts[i + 1].kind {
                    StatementKind::Assign(
                        add_dest,
                        Rvalue::BinaryOp(crate::parser::BinOp::Add, lhs, rhs),
                    ) => {
                        let add_dest = *add_dest;
                        if lhs == &Operand::Copy(mul_dest) {
                            Some((add_dest, rhs.clone()))
                        } else if rhs == &Operand::Copy(mul_dest) {
                            Some((add_dest, lhs.clone()))
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some((add_dest, vc)) = fma_info {
                    let span = stmts[i + 1].span;
                    stmts.remove(i);
                    stmts[i] = Statement {
                        kind: StatementKind::Assign(add_dest, Rvalue::VectorFMA(va, vb, vc)),
                        span,
                    };
                    continue;
                }
            }
            i += 1;
        }
    }

    // splat scalar to vector if needed
    fn ensure_vector(
        &self,
        func: &mut MirFunction,
        local: Local,
        width: usize,
        vector_locals: &mut FxHashMap<Local, Local>,
        stmts: &mut Vec<Statement>,
        span: Span,
    ) -> Local {
        if let Some(&v) = vector_locals.get(&local) {
            return v;
        }
        // Scalar value — splat it.
        let v = self.alloc_vector_local(func, local, width);
        vector_locals.insert(local, v);
        stmts.push(Statement {
            kind: StatementKind::Assign(v, Rvalue::VectorSplat(Operand::Copy(local), width)),
            span,
        });
        v
    }

    fn alloc_vector_local(&self, func: &mut MirFunction, original: Local, width: usize) -> Local {
        let ty = func.locals[original.0].ty.clone();
        let vec_ty = OliveType::Vector(Box::new(ty), width);
        let id = Local(func.locals.len());
        func.locals.push(LocalDecl {
            ty: vec_ty,
            name: Some(format!("v{}", original.0)),
            span: Span::default(),
            is_mut: true,
        });
        id
    }
}
