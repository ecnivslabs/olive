//! Runtime side of dual-variant tiering. `launch.rs` builds one
//! `DebugVariantTable` per session from the codegen's dispatch cells and
//! clean/`$debug` addresses; `sync`/`activate_all` are the only things that
//! ever write to those cells afterward, so this file -- not `CraneliftCodegen`
//! itself, which `DebugSession` keeps alive but the hot path never touches --
//! is the whole runtime surface for the swap.

use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicI64, Ordering};

struct VariantEntry {
    cell: *mut i64,
    clean_addr: i64,
    debug_addr: i64,
}

/// Raw pointers into the JIT module's own data segment, which lives for the
/// whole session (never freed until `DebugSession` drops, same lifetime the
/// codegen instance it came from already has -- see `tier_up.rs`'s identical
/// reasoning for why this is sound to send across threads).
unsafe impl Send for VariantEntry {}
unsafe impl Sync for VariantEntry {}

pub struct DebugVariantTable {
    by_fn_id: FxHashMap<u32, VariantEntry>,
}

impl DebugVariantTable {
    pub fn new() -> Self {
        Self {
            by_fn_id: FxHashMap::default(),
        }
    }

    pub fn insert(&mut self, fn_id: u32, cell: *mut i64, clean_addr: i64, debug_addr: i64) {
        self.by_fn_id.insert(
            fn_id,
            VariantEntry {
                cell,
                clean_addr,
                debug_addr,
            },
        );
    }

    fn atomic(cell: *mut i64) -> &'static AtomicI64 {
        unsafe { &*(cell as *const AtomicI64) }
    }

    /// Activates exactly the functions in `active`; every other function in
    /// the table reverts to its clean body. Called on `setBreakpoints` (with
    /// the current bp-owning fn_id set) and on a plain `continue` after a
    /// step/pause resolves (with the same set, since that's what free-running
    /// should look like again).
    pub fn sync(&self, active: &rustc_hash::FxHashSet<u32>) {
        for (fn_id, entry) in &self.by_fn_id {
            let target = if active.contains(fn_id) {
                entry.debug_addr
            } else {
                entry.clean_addr
            };
            Self::atomic(entry.cell).store(target, Ordering::Release);
        }
    }

    /// Every function instrumented, unconditionally -- `pause`/`next`/
    /// `stepIn`/`stepOut` need this since a step can land in any function,
    /// not just ones that happen to own a breakpoint right now.
    pub fn activate_all(&self) {
        for entry in self.by_fn_id.values() {
            Self::atomic(entry.cell).store(entry.debug_addr, Ordering::Release);
        }
    }
}

impl Default for DebugVariantTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_activates_only_the_requested_set() {
        let mut a_cell: i64 = 111;
        let mut b_cell: i64 = 222;
        let mut table = DebugVariantTable::new();
        table.insert(1, &mut a_cell as *mut i64, 111, 999);
        table.insert(2, &mut b_cell as *mut i64, 222, 888);

        let mut active = rustc_hash::FxHashSet::default();
        active.insert(1u32);
        table.sync(&active);
        assert_eq!(a_cell, 999);
        assert_eq!(b_cell, 222);

        table.sync(&rustc_hash::FxHashSet::default());
        assert_eq!(a_cell, 111);
        assert_eq!(b_cell, 222);
    }

    #[test]
    fn activate_all_sets_every_cell_to_debug() {
        let mut a_cell: i64 = 111;
        let mut b_cell: i64 = 222;
        let mut table = DebugVariantTable::new();
        table.insert(1, &mut a_cell as *mut i64, 111, 999);
        table.insert(2, &mut b_cell as *mut i64, 222, 888);
        table.activate_all();
        assert_eq!(a_cell, 999);
        assert_eq!(b_cell, 888);
    }
}
