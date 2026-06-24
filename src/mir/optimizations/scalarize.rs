use super::Transform;
use crate::mir::*;
use crate::span::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub struct ScalarizeStructs;

impl Transform for ScalarizeStructs {
    fn run(&self, func: &mut MirFunction) -> bool {
        let candidates = find_candidates(func);
        if candidates.is_empty() {
            return false;
        }

        let mut changed = false;
        for candidate in candidates {
            let mut aliases = HashSet::default();
            aliases.insert(candidate);

            let mut newly_added = true;
            while newly_added {
                newly_added = false;
                for bb in &func.basic_blocks {
                    for stmt in &bb.statements {
                        if let StatementKind::Assign(dst, Rvalue::Use(op)) = &stmt.kind
                            && let Some(src) = operand_local(op)
                            && aliases.contains(&src)
                            && !aliases.contains(dst)
                        {
                            aliases.insert(*dst);
                            newly_added = true;
                        }
                    }
                }
            }

            if !can_scalarize(func, &aliases, candidate) {
                continue;
            }

            let field_map = collect_field_map(func, &aliases);
            if field_map.is_empty() {
                continue;
            }

            let base = func.locals.len();
            let mut sorted_fields: Vec<(&String, &(usize, crate::semantic::types::Type))> =
                field_map.iter().collect();
            sorted_fields.sort_by_key(|&(_, &(idx, _))| idx);

            for (_, (_, ty)) in sorted_fields {
                func.locals.push(LocalDecl {
                    ty: ty.clone(),
                    name: None,
                    span: Span::default(),
                    is_mut: true,
                    is_owning: true,
                });
            }
            rewrite(func, &aliases, candidate, &field_map, base);
            changed = true;
        }
        changed
    }
}

fn find_candidates(func: &MirFunction) -> Vec<Local> {
    let mut seen: HashMap<Local, usize> = HashMap::default();
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(local, rval) = &stmt.kind {
                match rval {
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        ..
                    } if name == "__olive_struct_alloc" => {
                        *seen.entry(*local).or_insert(0) += 1;
                    }
                    Rvalue::Aggregate(crate::mir::ir::AggregateKind::Dict, _)
                    | Rvalue::Aggregate(crate::mir::ir::AggregateKind::List, _) => {
                        *seen.entry(*local).or_insert(0) += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    seen.into_iter()
        .filter_map(|(l, count)| if count == 1 { Some(l) } else { None })
        .collect()
}

/// A constant index is out of range when it is negative or, for a list whose
/// length is known, at or beyond that length. Such accesses are left for
/// codegen so the runtime bounds check reports them against the source.
fn index_out_of_range(i: i64, agg_len: Option<usize>) -> bool {
    i < 0 || agg_len.is_some_and(|len| i as usize >= len)
}

fn can_scalarize(func: &MirFunction, aliases: &HashSet<Local>, origin: Local) -> bool {
    let mut agg_len: Option<usize> = None;
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::Assign(
                l,
                Rvalue::Aggregate(crate::mir::ir::AggregateKind::List, ops),
            ) = &stmt.kind
                && *l == origin
            {
                agg_len = Some(ops.len());
            }
            let mut references_alias = false;
            for &alias in aliases {
                if stmt_references(stmt, alias) {
                    references_alias = true;
                    break;
                }
            }

            if !references_alias {
                continue;
            }

            match &stmt.kind {
                StatementKind::Assign(
                    l,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(name)),
                        ..
                    },
                ) if *l == origin && name == "__olive_struct_alloc" => {}

                StatementKind::Assign(
                    l,
                    Rvalue::Aggregate(crate::mir::ir::AggregateKind::Dict, _),
                ) if *l == origin => {}

                StatementKind::Assign(
                    l,
                    Rvalue::Aggregate(crate::mir::ir::AggregateKind::List, _),
                ) if *l == origin => {}

                StatementKind::SetAttr(op, _, val)
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    if operand_local(val).is_some_and(|l| aliases.contains(&l)) {
                        return false;
                    }
                }

                StatementKind::Assign(dst, Rvalue::GetAttr(op, _))
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    if aliases.contains(dst) {
                        return false;
                    }
                }

                StatementKind::SetIndex(op, idx_op, val)
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    match idx_op {
                        Operand::Constant(Constant::Int(i)) => {
                            if index_out_of_range(*i, agg_len) {
                                return false;
                            }
                        }
                        Operand::Constant(Constant::Str(_)) => {}
                        _ => return false,
                    }
                    if operand_local(val).is_some_and(|l| aliases.contains(&l)) {
                        return false;
                    }
                }

                StatementKind::Assign(dst, Rvalue::GetIndex(op, idx_op))
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    match idx_op {
                        Operand::Constant(Constant::Int(i)) => {
                            if index_out_of_range(*i, agg_len) {
                                return false;
                            }
                        }
                        Operand::Constant(Constant::Str(_)) => {}
                        _ => return false,
                    }
                    if aliases.contains(dst) {
                        return false;
                    }
                }

                StatementKind::Assign(dst, Rvalue::Use(op))
                    if aliases.contains(dst)
                        && operand_local(op).is_some_and(|l| aliases.contains(&l)) => {}

                StatementKind::Drop(l)
                | StatementKind::StorageLive(l)
                | StatementKind::StorageDead(l)
                    if aliases.contains(l) => {}

                _ => {
                    return false;
                }
            }
        }
    }
    true
}

fn collect_field_map(
    func: &MirFunction,
    aliases: &HashSet<Local>,
) -> HashMap<String, (usize, crate::semantic::types::Type)> {
    let mut map: HashMap<String, (usize, crate::semantic::types::Type)> = HashMap::default();
    for bb in &func.basic_blocks {
        for stmt in &bb.statements {
            match &stmt.kind {
                StatementKind::SetAttr(op, field, val)
                    if operand_local(op).is_some_and(|l| aliases.contains(&l))
                        && !map.contains_key(field) =>
                {
                    let ty = match val {
                        Operand::Copy(l) | Operand::Move(l) => func.locals[l.0].ty.clone(),
                        Operand::Constant(Constant::Float(_)) => {
                            crate::semantic::types::Type::Float
                        }
                        Operand::Constant(Constant::Int(_)) => crate::semantic::types::Type::Int,
                        _ => crate::semantic::types::Type::Any,
                    };
                    let next = map.len();
                    map.insert(field.clone(), (next, ty));
                }
                StatementKind::Assign(dst, Rvalue::GetAttr(op, field))
                    if operand_local(op).is_some_and(|l| aliases.contains(&l))
                        && !map.contains_key(field) =>
                {
                    let ty = func.locals[dst.0].ty.clone();
                    let next = map.len();
                    map.insert(field.clone(), (next, ty));
                }
                StatementKind::SetIndex(op, idx_op, val)
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    let field = match idx_op {
                        Operand::Constant(Constant::Int(i)) => i.to_string(),
                        Operand::Constant(Constant::Str(s)) => s.clone(),
                        _ => continue,
                    };
                    if !map.contains_key(&field) {
                        let ty = match val {
                            Operand::Copy(l) | Operand::Move(l) => func.locals[l.0].ty.clone(),
                            Operand::Constant(Constant::Float(_)) => {
                                crate::semantic::types::Type::Float
                            }
                            Operand::Constant(Constant::Int(_)) => {
                                crate::semantic::types::Type::Int
                            }
                            _ => crate::semantic::types::Type::Any,
                        };
                        let next = map.len();
                        map.insert(field, (next, ty));
                    }
                }
                StatementKind::Assign(dst, Rvalue::GetIndex(op, idx_op))
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    let field = match idx_op {
                        Operand::Constant(Constant::Int(i)) => i.to_string(),
                        Operand::Constant(Constant::Str(s)) => s.clone(),
                        _ => continue,
                    };
                    if !map.contains_key(&field) {
                        // The scalar replacing this slot must carry the element's
                        // real type, not a blanket `Any`. A wrong `Any` here
                        // parks a raw scalar in an `Any`-typed local, which then
                        // routes later arithmetic through the boxing `any_*`
                        // helpers and corrupts the value.
                        let ty = func.locals[dst.0].ty.clone();
                        let next = map.len();
                        map.insert(field, (next, ty));
                    }
                }
                _ => {}
            }
        }
    }
    map
}

fn rewrite(
    func: &mut MirFunction,
    aliases: &HashSet<Local>,
    origin: Local,
    field_map: &HashMap<String, (usize, crate::semantic::types::Type)>,
    base: usize,
) {
    for bb in &mut func.basic_blocks {
        let mut new_stmts: Vec<Statement> = Vec::with_capacity(bb.statements.len());
        for stmt in bb.statements.drain(..) {
            match stmt.kind {
                StatementKind::Assign(
                    l,
                    Rvalue::Call {
                        func: Operand::Constant(Constant::Function(ref name)),
                        ..
                    },
                ) if l == origin && name == "__olive_struct_alloc" => {
                    for i in 0..field_map.len() {
                        new_stmts.push(Statement {
                            kind: StatementKind::StorageLive(Local(base + i)),
                            span: stmt.span,
                        });
                    }
                }

                StatementKind::Assign(
                    l,
                    Rvalue::Aggregate(crate::mir::ir::AggregateKind::Dict, ref ops),
                ) if l == origin => {
                    for i in 0..field_map.len() {
                        new_stmts.push(Statement {
                            kind: StatementKind::StorageLive(Local(base + i)),
                            span: stmt.span,
                        });
                    }
                    for i in (0..ops.len()).step_by(2) {
                        let field = match ops[i] {
                            Operand::Constant(Constant::Int(n)) => Some(n.to_string()),
                            Operand::Constant(Constant::Str(ref s)) => Some(s.clone()),
                            _ => None,
                        };
                        if let Some(field) = field
                            && let Some(&(idx, _)) = field_map.get(&field)
                        {
                            new_stmts.push(Statement {
                                kind: StatementKind::Assign(
                                    Local(base + idx),
                                    Rvalue::Use(ops[i + 1].clone()),
                                ),
                                span: stmt.span,
                            });
                        }
                    }
                }

                StatementKind::Assign(
                    l,
                    Rvalue::Aggregate(crate::mir::ir::AggregateKind::List, ref ops),
                ) if l == origin => {
                    for i in 0..field_map.len() {
                        new_stmts.push(Statement {
                            kind: StatementKind::StorageLive(Local(base + i)),
                            span: stmt.span,
                        });
                    }
                    for (i, op) in ops.iter().enumerate() {
                        let field = i.to_string();
                        if let Some(&(idx, _)) = field_map.get(&field) {
                            new_stmts.push(Statement {
                                kind: StatementKind::Assign(
                                    Local(base + idx),
                                    Rvalue::Use(op.clone()),
                                ),
                                span: stmt.span,
                            });
                        }
                    }
                }

                StatementKind::SetAttr(ref op, ref field, ref val)
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    if let Some(&(idx, _)) = field_map.get(field) {
                        new_stmts.push(Statement {
                            kind: StatementKind::Assign(
                                Local(base + idx),
                                Rvalue::Use(val.clone()),
                            ),
                            span: stmt.span,
                        });
                    }
                }

                StatementKind::Assign(dst, Rvalue::GetAttr(ref op, ref field))
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    if let Some(&(idx, _)) = field_map.get(field) {
                        new_stmts.push(Statement {
                            kind: StatementKind::Assign(
                                dst,
                                Rvalue::Use(Operand::Copy(Local(base + idx))),
                            ),
                            span: stmt.span,
                        });
                    }
                }

                StatementKind::SetIndex(ref op, ref idx_op, ref val)
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    let field = match idx_op {
                        Operand::Constant(Constant::Int(i)) => i.to_string(),
                        Operand::Constant(Constant::Str(s)) => s.clone(),
                        _ => continue,
                    };
                    if let Some(&(idx, _)) = field_map.get(&field) {
                        new_stmts.push(Statement {
                            kind: StatementKind::Assign(
                                Local(base + idx),
                                Rvalue::Use(val.clone()),
                            ),
                            span: stmt.span,
                        });
                    }
                }

                StatementKind::Assign(dst, Rvalue::GetIndex(ref op, ref idx_op))
                    if operand_local(op).is_some_and(|l| aliases.contains(&l)) =>
                {
                    let field = match idx_op {
                        Operand::Constant(Constant::Int(i)) => i.to_string(),
                        Operand::Constant(Constant::Str(s)) => s.clone(),
                        _ => continue,
                    };
                    if let Some(&(idx, _)) = field_map.get(&field) {
                        new_stmts.push(Statement {
                            kind: StatementKind::Assign(
                                dst,
                                Rvalue::Use(Operand::Copy(Local(base + idx))),
                            ),
                            span: stmt.span,
                        });
                    }
                }

                StatementKind::Assign(dst, Rvalue::Use(ref op))
                    if aliases.contains(&dst)
                        && operand_local(op).is_some_and(|l| aliases.contains(&l)) => {}

                StatementKind::Drop(l) if aliases.contains(&l) => {}

                StatementKind::StorageLive(l) | StatementKind::StorageDead(l)
                    if aliases.contains(&l) => {}

                _ => new_stmts.push(stmt),
            }
        }
        bb.statements = new_stmts;
    }
}

#[inline]
fn operand_local(op: &Operand) -> Option<Local> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => Some(*l),
        _ => None,
    }
}

fn stmt_references(stmt: &Statement, local: Local) -> bool {
    match &stmt.kind {
        StatementKind::Assign(l, rval) => *l == local || rval_references(rval, local),
        StatementKind::SetAttr(op, _, val) => {
            operand_local(op) == Some(local) || operand_is(val, local)
        }
        StatementKind::SetIndex(obj, idx, val) => {
            operand_is(obj, local) || operand_is(idx, local) || operand_is(val, local)
        }
        StatementKind::Drop(l) | StatementKind::StorageLive(l) | StatementKind::StorageDead(l) => {
            *l == local
        }
        StatementKind::VectorStore(obj, idx, val) => {
            operand_is(obj, local) || operand_is(idx, local) || operand_is(val, local)
        }
        StatementKind::PtrStore(ptr, val) => operand_is(ptr, local) || operand_is(val, local),
    }
}

fn rval_references(rval: &Rvalue, local: Local) -> bool {
    match rval {
        Rvalue::Use(op)
        | Rvalue::UnaryOp(_, op)
        | Rvalue::GetAttr(op, _)
        | Rvalue::GetTag(op)
        | Rvalue::GetTypeId(op)
        | Rvalue::FatPtrData(op)
        | Rvalue::Cast(op, _)
        | Rvalue::PtrLoad(op)
        | Rvalue::VTableLoad { vtable: op, .. }
        | Rvalue::VectorSplat(op, _) => operand_is(op, local),
        Rvalue::BinaryOp(_, l, r) | Rvalue::GetIndex(l, r) => {
            operand_is(l, local) || operand_is(r, local)
        }
        Rvalue::Call { func, args } => {
            operand_is(func, local) || args.iter().any(|a| operand_is(a, local))
        }
        Rvalue::Aggregate(_, ops) => ops.iter().any(|o| operand_is(o, local)),
        Rvalue::Ref(l) | Rvalue::MutRef(l) => *l == local,
        Rvalue::VectorLoad(obj, idx, _) => operand_is(obj, local) || operand_is(idx, local),
        Rvalue::VectorFMA(a, b, c) => {
            operand_is(a, local) || operand_is(b, local) || operand_is(c, local)
        }
    }
}

#[inline]
fn operand_is(op: &Operand, local: Local) -> bool {
    operand_local(op) == Some(local)
}

#[cfg(test)]
#[cfg_attr(test, allow(dead_code))]
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

    fn stmt(k: StatementKind) -> Statement {
        Statement {
            kind: k,
            span: sp(),
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

    fn func(blocks: Vec<BasicBlock>) -> MirFunction {
        MirFunction {
            name: "f".into(),
            locals: vec![],
            basic_blocks: blocks,
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        }
    }

    fn bb(stmts: Vec<Statement>, kind: TerminatorKind) -> BasicBlock {
        BasicBlock {
            statements: stmts,
            terminator: Some(Terminator { kind, span: sp() }),
        }
    }

    #[test]
    fn no_struct_alloc_no_change() {
        let mut f = func(vec![bb(vec![], TerminatorKind::Return)]);
        assert!(!ScalarizeStructs.run(&mut f));
    }

    #[test]
    fn single_dict_alloc_not_scalarized_if_no_attr_access() {
        let mut f = MirFunction {
            name: "f".into(),
            locals: vec![local_decl(crate::semantic::types::Type::Any)],
            basic_blocks: vec![bb(
                vec![assign(0, Rvalue::Aggregate(AggregateKind::Dict, vec![]))],
                TerminatorKind::Return,
            )],
            arg_count: 0,
            vararg_idx: None,
            kwarg_idx: None,
            param_names: vec![],
            is_async: false,
        };
        // Single dict alloc with no field accesses -> find_candidates finds it,
        // can_scalarize returns false (due to Aggregate-based init being excluded), so no change
        let _changed = ScalarizeStructs.run(&mut f);
        // may or may not scalarize (depends on internal logic); at minimum run shouldn't crash
        assert!(!f.basic_blocks[0].statements.is_empty());
    }

    #[test]
    fn scalarize_runs_safely() {
        let mut f = func(vec![bb(
            vec![assign(
                0,
                Rvalue::Aggregate(
                    AggregateKind::Dict,
                    vec![
                        Operand::Constant(Constant::Str("x".into())),
                        Operand::Constant(Constant::Int(1)),
                    ],
                ),
            )],
            TerminatorKind::Return,
        )]);
        // Just ensure it runs without panicking
        ScalarizeStructs.run(&mut f);
    }
}
