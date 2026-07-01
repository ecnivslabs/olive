use super::ownership::push_local;
use crate::mir::*;
use crate::semantic::types::Type;
use std::collections::HashSet;

/// Returns the set of struct names that define `__drop__`, derived by
/// scanning all function names for the `::__drop__` suffix.
pub fn collect_struct_has_drop(functions: &[MirFunction]) -> HashSet<String> {
    let mut result = HashSet::new();
    for func in functions {
        if let Some(name) = func.name.strip_suffix("::__drop__") {
            result.insert(name.to_string());
        }
    }
    result
}

/// Build the monomorphized name for a struct type, matching the naming
/// convention used by the generic monomorphizer.
pub fn monomorphized_name(struct_name: &str, type_args: &[Type]) -> String {
    if type_args.is_empty() {
        return struct_name.to_string();
    }
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
    format!("{}_{}", struct_name, arg_str)
}

/// The name of the struct whose `__drop__` we are currently inside, if any.
/// Derived from `func.name` (e.g. `"MyStruct::__drop__"`). Inside a drop
/// handler the struct's own drops are left as ordinary slab frees so that
/// dropping `self` at scope exit does not recurse.
fn drop_self_struct(func: &MirFunction) -> Option<&str> {
    func.name.strip_suffix("::__drop__")
}

/// After the ownership pass, replaces `Drop(local)` with a call to the
/// struct's `__drop__` method for structs that define one. The set of
/// such structs must be provided by `collect_struct_has_drop`.
pub fn lower_drop_hooks(func: &mut MirFunction, has_drop: &HashSet<String>) {
    if has_drop.is_empty() {
        return;
    }
    let self_struct = drop_self_struct(func);
    struct DropSite {
        bb: usize,
        idx: usize,
        drop_fn: String,
        local: Local,
    }
    let mut sites: Vec<DropSite> = Vec::new();
    for (bb_idx, block) in func.basic_blocks.iter().enumerate() {
        for (idx, stmt) in block.statements.iter().enumerate() {
            if let StatementKind::Drop(local) = &stmt.kind
                && let Type::Struct(name, args, _) = &func.locals[local.0].ty
            {
                let drop_name = monomorphized_name(name, args);
                if has_drop.contains(&drop_name) && self_struct != Some(name.as_str()) {
                    sites.push(DropSite {
                        bb: bb_idx,
                        idx,
                        drop_fn: format!("{}::__drop__", drop_name),
                        local: *local,
                    });
                }
            }
        }
    }
    for site in sites {
        let tmp = push_local(func, Type::Any);
        func.basic_blocks[site.bb].statements[site.idx].kind = StatementKind::Assign(
            tmp,
            Rvalue::Call {
                func: Operand::Constant(Constant::Function(site.drop_fn)),
                args: vec![Operand::Move(site.local)],
            },
        );
    }
}
