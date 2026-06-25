//! Devirtualizes trait-object dispatch when inlining exposes the coercion.
//! A fat-pointer record built and consumed inside one function with a
//! constant vtable resolves its method loads at compile time; the record
//! allocation, indirect calls, and drop-shim round trip all disappear, and
//! the concrete struct becomes visible to scalarization.

use super::Transform;
use crate::mir::*;
use crate::semantic::types::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub struct Devirtualize<'a> {
    pub vtables: &'a HashMap<String, Vec<String>>,
    /// Structs with a `__drop__` hook keep the dynamic path: the record's
    /// drop shim runs the hook, and this pass runs after hook lowering.
    pub has_drop: &'a std::collections::HashSet<String>,
}

#[derive(Clone)]
struct RecordInfo {
    data: Operand,
    vtable: String,
}

fn operand_local(op: &Operand) -> Option<Local> {
    match op {
        Operand::Copy(l) | Operand::Move(l) => Some(*l),
        Operand::Constant(_) => None,
    }
}

impl Transform for Devirtualize<'_> {
    fn run(&self, func: &mut MirFunction) -> bool {
        if self.vtables.is_empty() {
            return false;
        }

        let mut def_counts: HashMap<Local, usize> = HashMap::default();
        let mut roots: HashMap<Local, RecordInfo> = HashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                if let StatementKind::Assign(dest, rval) = &stmt.kind {
                    *def_counts.entry(*dest).or_insert(0) += 1;
                    if let Rvalue::Aggregate(AggregateKind::FatPtr, ops) = rval
                        && let [data, Operand::Constant(Constant::GlobalData(vt)), _] =
                            ops.as_slice()
                        && self.vtables.contains_key(vt.as_str())
                    {
                        roots.insert(
                            *dest,
                            RecordInfo {
                                data: data.clone(),
                                vtable: vt.clone(),
                            },
                        );
                    }
                }
            }
        }
        if roots.is_empty() {
            return false;
        }

        // The data operand must be a single-def local: its value is re-read at
        // former FatPtrData sites, so a reassignment in between would change it.
        roots.retain(|dest, info| {
            def_counts.get(dest) == Some(&1)
                && dest.0 > func.arg_count
                && match operand_local(&info.data) {
                    Some(l) => {
                        def_counts.get(&l) == Some(&1)
                            && l.0 > func.arg_count
                            && match &func.locals[l.0].ty {
                                Type::Struct(name, _, is_ffi) => {
                                    !is_ffi && !self.has_drop.contains(name)
                                }
                                _ => false,
                            }
                    }
                    None => false,
                }
        });

        // Aliases: single-def locals holding a plain copy of a record root.
        let mut group: HashMap<Local, Local> = HashMap::default();
        for &r in roots.keys() {
            group.insert(r, r);
        }
        let mut changed_alias = true;
        while changed_alias {
            changed_alias = false;
            for bb in &func.basic_blocks {
                for stmt in &bb.statements {
                    if let StatementKind::Assign(dest, Rvalue::Use(op)) = &stmt.kind
                        && let Some(src) = operand_local(op)
                        && let Some(&root) = group.get(&src)
                        && def_counts.get(dest) == Some(&1)
                        && dest.0 > func.arg_count
                        && dest.0 != 0
                        && !group.contains_key(dest)
                    {
                        group.insert(*dest, root);
                        changed_alias = true;
                    }
                }
            }
        }

        // Any use outside the rewritable shapes keeps the record dynamic.
        let mut poisoned: HashSet<Local> = HashSet::default();
        let mut vtl_methods: HashMap<Local, (Local, String)> = HashMap::default();
        for bb in &func.basic_blocks {
            for stmt in &bb.statements {
                let mut reads: Vec<Local> = Vec::new();
                match &stmt.kind {
                    StatementKind::Assign(dest, rval) => match rval {
                        Rvalue::VTableLoad { vtable, method_idx } => {
                            if let Some(l) = operand_local(vtable)
                                && let Some(root) = group.get(&l)
                            {
                                let info = &roots[root];
                                let methods = &self.vtables[&info.vtable];
                                match methods.get(*method_idx) {
                                    Some(m) if def_counts.get(dest) == Some(&1) => {
                                        vtl_methods.insert(*dest, (*root, m.clone()));
                                    }
                                    _ => reads.push(l),
                                }
                            }
                        }
                        Rvalue::FatPtrData(op) => {
                            if let Some(l) = operand_local(op)
                                && !group.contains_key(&l)
                            {
                                reads.push(l);
                            }
                        }
                        Rvalue::Use(op) => {
                            if let Some(l) = operand_local(op)
                                && group.contains_key(&l)
                                && (!group.contains_key(dest) || dest.0 == 0)
                            {
                                reads.push(l);
                            }
                        }
                        _ => Self::rvalue_locals(rval, &mut reads),
                    },
                    StatementKind::Drop(_) => {}
                    StatementKind::SetIndex(o, i, v, _) => {
                        for op in [o, i, v] {
                            reads.extend(operand_local(op));
                        }
                    }
                    StatementKind::SetAttr(o, _, v) | StatementKind::PtrStore(o, v) => {
                        for op in [o, v] {
                            reads.extend(operand_local(op));
                        }
                    }
                    StatementKind::VectorStore(o, i, v) => {
                        for op in [o, i, v] {
                            reads.extend(operand_local(op));
                        }
                    }
                    StatementKind::GenCheck { value, generation } => {
                        reads.push(*value);
                        reads.push(*generation);
                    }
                    StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
                }
                for l in reads {
                    if let Some(&root) = group.get(&l) {
                        poisoned.insert(root);
                    }
                }
            }
            if let Some(term) = &bb.terminator
                && let TerminatorKind::SwitchInt { discr, .. } = &term.kind
                && let Some(l) = operand_local(discr)
                && let Some(&root) = group.get(&l)
            {
                poisoned.insert(root);
            }
        }
        roots.retain(|r, _| !poisoned.contains(r));
        if roots.is_empty() {
            return false;
        }
        group.retain(|_, root| roots.contains_key(root));
        vtl_methods.retain(|_, (root, _)| roots.contains_key(root));

        let live_root = |l: &Local| group.get(l).filter(|r| roots.contains_key(r)).copied();

        // Rewrite: method loads become function constants, data reads collapse
        // to the wrapped struct, the record's drop moves to the struct itself,
        // and the record aggregate plus its aliases die for DCE to sweep.
        let mut changed = false;
        let mut retypes: Vec<(Local, Local)> = Vec::new();
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                let new_kind = match &stmt.kind {
                    StatementKind::Assign(dest, Rvalue::VTableLoad { .. }) => {
                        vtl_methods.get(dest).map(|(_, m)| {
                            StatementKind::Assign(
                                *dest,
                                Rvalue::Use(Operand::Constant(Constant::Function(m.clone()))),
                            )
                        })
                    }
                    StatementKind::Assign(dest, Rvalue::FatPtrData(op)) => operand_local(op)
                        .and_then(|l| live_root(&l))
                        .map(|root| {
                            let data = roots[&root].data.clone();
                            let src = operand_local(&data).unwrap();
                            // The raw struct pointer must dispatch by its
                            // concrete type, not the temp's erased `Any`.
                            if def_counts.get(dest) == Some(&1) {
                                retypes.push((*dest, src));
                            }
                            StatementKind::Assign(*dest, Rvalue::Use(Operand::Copy(src)))
                        }),
                    StatementKind::Assign(dest, Rvalue::Aggregate(AggregateKind::FatPtr, _))
                        if roots.contains_key(dest) =>
                    {
                        Some(StatementKind::StorageLive(*dest))
                    }
                    StatementKind::Assign(dest, Rvalue::Use(op)) => {
                        match (
                            operand_local(op).and_then(|l| live_root(&l)),
                            live_root(dest),
                        ) {
                            (Some(_), Some(_)) => Some(StatementKind::StorageLive(*dest)),
                            _ => None,
                        }
                    }
                    StatementKind::Drop(l) => live_root(l).map(|root| {
                        let src = operand_local(&roots[&root].data).unwrap();
                        StatementKind::Drop(src)
                    }),
                    _ => None,
                };
                if let Some(kind) = new_kind {
                    stmt.kind = kind;
                    changed = true;
                }
            }
        }

        for (dest, src) in retypes {
            func.locals[dest.0].ty = func.locals[src.0].ty.clone();
        }

        // Calls through a resolved method local become direct.
        for bb in &mut func.basic_blocks {
            for stmt in &mut bb.statements {
                if let StatementKind::Assign(_, Rvalue::Call { func: f, .. }) = &mut stmt.kind
                    && let Some(l) = operand_local(f)
                    && let Some((_, m)) = vtl_methods.get(&l)
                {
                    *f = Operand::Constant(Constant::Function(m.clone()));
                    changed = true;
                }
            }
        }

        changed
    }
}

impl Devirtualize<'_> {
    fn rvalue_locals(rval: &Rvalue, out: &mut Vec<Local>) {
        match rval {
            Rvalue::Use(op)
            | Rvalue::UnaryOp(_, op)
            | Rvalue::Cast(op, _)
            | Rvalue::GetAttr(op, _)
            | Rvalue::GetTag(op)
            | Rvalue::GetTypeId(op)
            | Rvalue::VectorSplat(op, _)
            | Rvalue::VectorReduce(_, op, _)
            | Rvalue::PtrLoad(op)
            | Rvalue::GenOf(op)
            | Rvalue::FatPtrData(op)
            | Rvalue::VTableLoad { vtable: op, .. } => out.extend(operand_local(op)),
            Rvalue::BinaryOp(_, a, b) | Rvalue::GetIndex(a, b, _) | Rvalue::VectorLoad(a, b, _) => {
                out.extend(operand_local(a));
                out.extend(operand_local(b));
            }
            Rvalue::VectorFMA(a, b, c) => {
                out.extend(operand_local(a));
                out.extend(operand_local(b));
                out.extend(operand_local(c));
            }
            Rvalue::Call { func, args } => {
                out.extend(operand_local(func));
                for a in args {
                    out.extend(operand_local(a));
                }
            }
            Rvalue::Aggregate(_, ops) => {
                for op in ops {
                    out.extend(operand_local(op));
                }
            }
            Rvalue::Ref(l) | Rvalue::MutRef(l) => out.push(*l),
        }
    }
}
