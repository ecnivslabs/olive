use crate::mir::loop_utils;
use crate::mir::optimizations::Transform;
use crate::mir::*;
use crate::semantic::types::Type as OliveType;
use crate::span::Span;
use rustc_hash::FxHashMap;

pub struct LoopVectorizer;

impl Transform for LoopVectorizer {
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
    loads: Vec<(Local, Operand)>,
}

/// A type is vectorizable only if it lowers to a Cranelift SIMD lane type.
/// Pointer/aggregate element types (structs, `bytes`, etc.) fall back to a
/// scalar `i64` in `cl_type`, so wrapping them in a vector would declare a
/// scalar local while emitting a real SIMD value, a codegen type mismatch.
fn is_simd_scalar(ty: &OliveType) -> bool {
    matches!(
        ty,
        OliveType::Int
            | OliveType::U64
            | OliveType::Usize
            | OliveType::I32
            | OliveType::U32
            | OliveType::I16
            | OliveType::U16
            | OliveType::I8
            | OliveType::U8
            | OliveType::Bool
            | OliveType::Float
            | OliveType::F32
    )
}

/// Cranelift lane width in bits for a SIMD-compatible scalar.
fn simd_lane_bits(ty: &OliveType) -> Option<u32> {
    match ty {
        OliveType::Int | OliveType::U64 | OliveType::Usize | OliveType::Float => Some(64),
        OliveType::I32 | OliveType::U32 | OliveType::F32 => Some(32),
        OliveType::I16 | OliveType::U16 => Some(16),
        OliveType::I8 | OliveType::U8 | OliveType::Bool => Some(8),
        _ => None,
    }
}

fn operand_local(op: &Operand) -> Option<Local> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => Some(*l),
        Operand::Constant(_) => None,
    }
}

/// Collects every local read by an rvalue.
fn rvalue_reads(rval: &Rvalue, out: &mut Vec<Local>) {
    let mut push = |op: &Operand| {
        if let Some(l) = operand_local(op) {
            out.push(l);
        }
    };
    match rval {
        Rvalue::Use(op)
        | Rvalue::UnaryOp(_, op)
        | Rvalue::Cast(op, _)
        | Rvalue::GetAttr(op, _)
        | Rvalue::GetTag(op)
        | Rvalue::GetTypeId(op)
        | Rvalue::VectorSplat(op, _)
        | Rvalue::PtrLoad(op)
        | Rvalue::GenOf(op)
        | Rvalue::FatPtrData(op)
        | Rvalue::VTableLoad { vtable: op, .. } => push(op),
        Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) | Rvalue::VectorLoad(a, b, _) => {
            push(a);
            push(b);
        }
        Rvalue::VectorFMA(a, b, c) => {
            push(a);
            push(b);
            push(c);
        }
        Rvalue::Call { func, args } => {
            push(func);
            for a in args {
                push(a);
            }
        }
        Rvalue::Aggregate(_, ops) => {
            for op in ops {
                push(op);
            }
        }
        Rvalue::Ref(l) | Rvalue::MutRef(l) => out.push(*l),
    }
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
                    && *src == *local
                {
                    if induction.is_some() {
                        return None;
                    }
                    induction = Some(*local);
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
                && *idx == i
            {
                limit = Some(lim.clone());
                break;
            }
        }
        let limit = limit?;
        let mut loads = Vec::new();

        for &bb_id in &lp.body {
            for stmt in &func.basic_blocks[bb_id.0].statements {
                if let StatementKind::Assign(dest, Rvalue::GetIndex(obj, Operand::Copy(idx), _)) =
                    &stmt.kind
                    && *idx == i
                {
                    if !is_simd_scalar(&func.locals[dest.0].ty) {
                        return None;
                    }
                    loads.push((*dest, obj.clone()));
                }
            }
        }

        if loads.is_empty() {
            return None;
        }

        if lp.exits.len() > 1 {
            return None;
        }

        // All loaded lanes must share a width so they pack into one SIMD
        // register, and that register is capped at 128 bits, the width every
        // Cranelift target backend supports for integer lanes (wider vectors
        // like i64x4 need AVX2 and are rejected by the backend).
        let lane_bits = simd_lane_bits(&func.locals[loads[0].0.0].ty)?;
        for (dest, _) in &loads {
            if simd_lane_bits(&func.locals[dest.0].ty) != Some(lane_bits) {
                return None;
            }
        }
        let width = (128 / lane_bits) as usize;
        if width < 2 {
            return None;
        }

        // Build the set of locals that will become vectors: the loaded lanes
        // plus any value derived from them through elementwise binary ops.
        let mut vec_locals: rustc_hash::FxHashSet<Local> = loads.iter().map(|(d, _)| *d).collect();
        loop {
            let mut changed = false;
            for &bb_id in &lp.body {
                for stmt in &func.basic_blocks[bb_id.0].statements {
                    if let StatementKind::Assign(
                        dest,
                        Rvalue::BinaryOp(_, Operand::Copy(l), Operand::Copy(r)),
                    ) = &stmt.kind
                        && (vec_locals.contains(l) || vec_locals.contains(r))
                        && !vec_locals.contains(dest)
                    {
                        if simd_lane_bits(&func.locals[dest.0].ty) != Some(lane_bits) {
                            return None;
                        }
                        vec_locals.insert(*dest);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // The transform only knows how to rewrite three shapes that touch a
        // vectorized local: a load at `i`, an elementwise binary op of two
        // operands, and a store at `i`. If any other statement reads a
        // vectorized local, vectorizing would feed a SIMD value into scalar
        // code, so bail out and leave the loop untouched.
        for &bb_id in &lp.body {
            for stmt in &func.basic_blocks[bb_id.0].statements {
                let allowed = match &stmt.kind {
                    StatementKind::Assign(dest, Rvalue::GetIndex(_, Operand::Copy(idx), _))
                        if *idx == i && vec_locals.contains(dest) =>
                    {
                        true
                    }
                    StatementKind::Assign(
                        dest,
                        Rvalue::BinaryOp(_, Operand::Copy(_), Operand::Copy(_)),
                    ) if vec_locals.contains(dest) => true,
                    StatementKind::SetIndex(_, Operand::Copy(idx), _, _) if *idx == i => true,
                    _ => {
                        let mut reads = Vec::new();
                        match &stmt.kind {
                            StatementKind::Assign(_, rval) => rvalue_reads(rval, &mut reads),
                            StatementKind::SetAttr(o, _, v) | StatementKind::PtrStore(o, v) => {
                                reads.extend(operand_local(o));
                                reads.extend(operand_local(v));
                            }
                            StatementKind::SetIndex(o, idx, v, _)
                            | StatementKind::VectorStore(o, idx, v) => {
                                reads.extend(operand_local(o));
                                reads.extend(operand_local(idx));
                                reads.extend(operand_local(v));
                            }
                            StatementKind::Drop(l) => reads.push(*l),
                            StatementKind::GenCheck { value, generation } => {
                                reads.push(*value);
                                reads.push(*generation);
                            }
                            StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
                        }
                        !reads.iter().any(|l| vec_locals.contains(l))
                    }
                };
                if !allowed {
                    return None;
                }
            }
        }

        // A vectorized local must not escape the loop body. If one is read in
        // any other block (e.g. a reduction accumulator printed afterwards),
        // the scalar copy never receives the per-lane updates, so refuse to
        // vectorize rather than miscompile.
        let body_set: rustc_hash::FxHashSet<BasicBlockId> = lp.body.iter().copied().collect();
        for (bb_id, bb) in func.basic_blocks.iter().enumerate() {
            if body_set.contains(&BasicBlockId(bb_id)) {
                continue;
            }
            for stmt in &bb.statements {
                let mut reads = Vec::new();
                match &stmt.kind {
                    StatementKind::Assign(_, rval) => rvalue_reads(rval, &mut reads),
                    StatementKind::SetAttr(o, _, v) | StatementKind::PtrStore(o, v) => {
                        reads.extend(operand_local(o));
                        reads.extend(operand_local(v));
                    }
                    StatementKind::SetIndex(o, idx, v, _)
                    | StatementKind::VectorStore(o, idx, v) => {
                        reads.extend(operand_local(o));
                        reads.extend(operand_local(idx));
                        reads.extend(operand_local(v));
                    }
                    StatementKind::Drop(l) => reads.push(*l),
                    StatementKind::GenCheck { value, generation } => {
                        reads.push(*value);
                        reads.push(*generation);
                    }
                    StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
                }
                if reads.iter().any(|l| vec_locals.contains(l)) {
                    return None;
                }
            }
            if let Some(term) = &bb.terminator {
                let discr = match &term.kind {
                    TerminatorKind::SwitchInt { discr, .. } => operand_local(discr),
                    _ => None,
                };
                if let Some(l) = discr
                    && vec_locals.contains(&l)
                {
                    return None;
                }
            }
        }

        Some(VectorizationPlan {
            induction: i,
            limit,
            width,
            loads,
        })
    }

    fn transform(
        &self,
        func: &mut MirFunction,
        lp: &loop_utils::Loop,
        plan: &VectorizationPlan,
    ) -> bool {
        let i = plan.induction;
        let width = plan.width;

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
            is_owning: true,
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
                    && *idx == i
                {
                    found = Some(*local);
                    break;
                }
            }
            found
        };

        if let Some(cond_local) = cond_local {
            let header = &mut func.basic_blocks[lp.header.0];
            for stmt in &mut header.statements {
                if let StatementKind::Assign(l, _) = &stmt.kind
                    && *l == cond_local
                {
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
            if let Some(term) = &mut header.terminator
                && let TerminatorKind::SwitchInt { otherwise, .. } = &mut term.kind
            {
                *otherwise = epilogue_header;
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
                    StatementKind::Assign(dest, Rvalue::GetIndex(obj, Operand::Copy(idx), _))
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

                    StatementKind::SetIndex(obj, Operand::Copy(idx), Operand::Copy(val), _)
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
                    && *local == i
                    && *src == i
                {
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

        true
    }

    fn fuse_fma(stmts: &mut Vec<Statement>) {
        let mut i = 0;
        while i + 1 < stmts.len() {
            let mul_info = match &stmts[i].kind {
                StatementKind::Assign(
                    dest,
                    Rvalue::BinaryOp(crate::parser::BinOp::Mul, va, vb),
                ) => Some((*dest, va.clone(), vb.clone())),
                _ => None,
            };
            if let Some((mul_dest, va, vb)) = mul_info {
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
            is_owning: true,
        });
        id
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;

    fn sp() -> crate::span::Span {
        crate::span::Span {
            file_id: 0,
            line: 0,
            col: 0,
            start: 0,
            end: 0,
        }
    }

    fn assign(l: usize, rv: Rvalue) -> Statement {
        Statement {
            kind: StatementKind::Assign(Local(l), rv),
            span: sp(),
        }
    }

    fn bb(stmts: Vec<Statement>, kind: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator { kind, span: sp() }),
        }
    }

    fn func(name: &str, locals: Vec<LocalDecl>) -> MirFunction {
        MirFunction {
            name: name.into(),
            locals,
            basic_blocks: vec![],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn local_decl(ty: crate::semantic::types::Type) -> LocalDecl {
        LocalDecl {
            ty,
            name: None,
            span: sp(),
            is_mut: true,
            is_owning: true,
        }
    }

    #[test]
    fn no_loops_no_vectorize() {
        let mut f = func("f", vec![]);
        assert!(!LoopVectorizer.run(&mut f));
    }

    #[test]
    fn empty_func_no_vectorize() {
        let mut f = func("f", vec![]);
        assert!(!LoopVectorizer.run(&mut f));
    }

    #[test]
    fn alloc_vector_local_creates_vector_type() {
        let mut f = func("test", vec![local_decl(OliveType::Float)]);
        let v = LoopVectorizer.alloc_vector_local(&mut f, Local(0), 4);
        assert_eq!(v.0, 1);
        assert_eq!(f.locals.len(), 2);
        match &f.locals[v.0].ty {
            OliveType::Vector(inner, width) => {
                assert_eq!(**inner, OliveType::Float);
                assert_eq!(*width, 4);
            }
            _ => panic!("expected Vector type"),
        }
    }

    #[test]
    fn fuse_fma_combines_mul_add() {
        let mut stmts = vec![
            assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Mul,
                    Operand::Copy(Local(1)),
                    Operand::Copy(Local(2)),
                ),
            ),
            assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(Local(0)),
                    Operand::Copy(Local(3)),
                ),
            ),
        ];
        LoopVectorizer::fuse_fma(&mut stmts);
        assert_eq!(stmts.len(), 1);
        assert!(matches!(
            &stmts[0].kind,
            StatementKind::Assign(_, Rvalue::VectorFMA(..))
        ));
    }

    #[test]
    fn fuse_fma_no_mul_keeps_stmts() {
        let mut stmts = vec![
            assign(0, Rvalue::Use(Operand::Constant(Constant::Int(1)))),
            assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(Local(1)),
                    Operand::Copy(Local(2)),
                ),
            ),
        ];
        let len = stmts.len();
        LoopVectorizer::fuse_fma(&mut stmts);
        assert_eq!(stmts.len(), len);
    }

    #[test]
    fn fuse_fma_reordered() {
        let mut stmts = vec![
            assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Mul,
                    Operand::Copy(Local(1)),
                    Operand::Copy(Local(2)),
                ),
            ),
            assign(
                0,
                Rvalue::BinaryOp(
                    crate::parser::BinOp::Add,
                    Operand::Copy(Local(3)),
                    Operand::Copy(Local(0)),
                ),
            ),
        ];
        LoopVectorizer::fuse_fma(&mut stmts);
        assert_eq!(stmts.len(), 1);
        assert!(matches!(
            &stmts[0].kind,
            StatementKind::Assign(_, Rvalue::VectorFMA(..))
        ));
    }
}
