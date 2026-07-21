//! Refcounted sharing for structs that manage an external resource (source
//! defines `__drop__`). The compiler's implicit copies -- async task-arg
//! marshalling, ownership escape-copies -- duplicate a plain data struct's
//! fields safely, since each copy ends up with its own independent fields.
//! A resource handle isn't safely duplicable that way: two independent
//! copies would each free the same underlying resource. `encode_descriptor`
//! tags such a struct `D_STRUCT_SHARED` instead of `D_STRUCT`, which routes
//! its copy through `retain_struct` (bump a count, hand back the same
//! allocation) and its self-drop through `release_struct` (only the last
//! reference actually reclaims memory).
//!
//! Retain and release can run on different OS threads (that is the entire
//! point of sharing a channel/mutex handle across spawned tasks), so the
//! count lives in a global table rather than anything thread-local.

use rustc_hash::FxHashMap;
use std::sync::{Mutex, OnceLock};

static COUNTS: OnceLock<Mutex<FxHashMap<i64, usize>>> = OnceLock::new();

fn counts() -> &'static Mutex<FxHashMap<i64, usize>> {
    COUNTS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// One more reference to `ptr` now exists; the allocation itself is untouched.
pub(crate) fn retain_struct(ptr: i64) -> i64 {
    if ptr == 0 {
        return 0;
    }
    let mut table = counts().lock().unwrap();
    let total = table.remove(&ptr).unwrap_or(1) + 1;
    table.insert(ptr, total);
    ptr
}

/// One reference to `ptr` is going away. Returns whether this was the last
/// one -- the caller must reclaim the allocation only when this is true.
pub(crate) fn release_struct(ptr: i64) -> bool {
    if ptr == 0 {
        return true;
    }
    let mut table = counts().lock().unwrap();
    match table.remove(&ptr) {
        None => true,
        Some(n) if n <= 2 => false,
        Some(n) => {
            table.insert(ptr, n - 1);
            false
        }
    }
}

/// Compiler-inserted gate at the start of every has-drop struct's `__drop__`
/// (`mir/builder/lower_stmt/functions.rs`): one reference is going away, and
/// the body -- the user's own cleanup plus `self`'s ordinary end-of-function
/// reclaim -- must run only when this is the last one. Every other caller's
/// `__drop__` invocation short-circuits to a no-op, leaving the shared
/// allocation and whatever it owns untouched for the remaining references.
#[unsafe(no_mangle)]
pub extern "C" fn olive_struct_gate(ptr: i64) -> i64 {
    release_struct(ptr) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sole_owner_frees_immediately() {
        assert!(release_struct(0x1000));
    }

    #[test]
    fn shared_owner_defers_free_to_last() {
        let ptr = 0x2000;
        retain_struct(ptr);
        retain_struct(ptr);
        assert!(!release_struct(ptr));
        assert!(!release_struct(ptr));
        assert!(release_struct(ptr));
    }

    #[test]
    fn zero_pointer_is_a_no_op() {
        assert_eq!(retain_struct(0), 0);
        assert!(release_struct(0));
    }
}
