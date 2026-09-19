//! Runtime `__drop__` registry: maps struct names to their hook entry points
//! so cleanup runs on paths where no MIR hook site exists (nested
//! containers, enum payloads, `Any` values). MIR hook sites (direct, union,
//! list, tuple, dict, set) keep working exactly as before: whoever consumes
//! the slot first wins, and every other free path sees a dead slot and
//! skips through the generation guard, so user code runs exactly once.
//!
//! Registration is emitted into `__main__`'s prologue (one call per struct,
//! under every known name spelling), which runs before any drop in both JIT
//! and AOT. Reads take the lock only for the lookup; the hook itself runs
//! unlocked so nested drops that look up other hooks cannot deadlock.

use std::collections::HashMap;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

pub(crate) type DropHook = extern "C" fn(i64) -> i64;

fn registry() -> &'static Mutex<HashMap<String, i64>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Set on the first registration; lets hot free paths skip the lock
/// entirely when the program defines no `__drop__` at all.
static ANY_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Records `hook` (a `__drop__` entry point as a word, the same
/// `Constant::Function`-as-address convention the per-element helpers use)
/// under the struct name `name` (a tagged Olive string word). Called from
/// `__main__`'s prologue, before any value can drop.
#[unsafe(no_mangle)]
pub extern "C" fn olive_register_drop(name: i64, hook: i64) {
    if name == 0 || hook == 0 {
        return;
    }
    let key = crate::olive_str_from_ptr(name);
    registry().lock().unwrap().insert(key, hook);
    ANY_REGISTERED.store(true, Ordering::Release);
}

fn lookup(name: &str) -> Option<DropHook> {
    let table = registry().lock().unwrap();
    table
        .get(name)
        .copied()
        .or_else(|| {
            let stripped = name.rsplit("::").next().unwrap_or(name);
            if stripped == name {
                None
            } else {
                table.get(stripped).copied()
            }
        })
        .map(|addr| unsafe { std::mem::transmute(addr as usize) })
}

/// Whether anything registered yet: a lock-free atomic on the hot free
/// paths, so programs without `__drop__` pay one load per struct free and
/// nothing more.
pub(crate) fn has_registrations() -> bool {
    ANY_REGISTERED.load(Ordering::Acquire)
}

/// Looks up the hook for the struct named by the descriptor at `pos` (past
/// any tag byte) and runs it on `val`. Borrows the name bytes for the
/// synchronous lookup only, allocating nothing; length byte is biased by
/// 13 like every other descriptor string. Returns whether a hook ran (in
/// which case the caller must treat the slot as consumed).
pub(crate) fn run_hook_for_desc(desc: *const u8, pos: usize, val: i64) -> bool {
    if val == 0 || !has_registrations() {
        return false;
    }
    let len = unsafe { *desc.add(pos) } as usize - 13;
    let bytes = unsafe { std::slice::from_raw_parts(desc.add(pos + 1), len) };
    let Ok(name) = std::str::from_utf8(bytes) else {
        return false;
    };
    run_registered_hook(name, val)
}

/// Runs the registered `__drop__` for the struct named `name` on `val`, if
/// any. The hook consumes the value fully (user cleanup plus its own
/// storage reclaim), so the caller must treat the slot as gone afterwards
/// and only advance past its descriptor.
pub(crate) fn run_registered_hook(name: &str, val: i64) -> bool {
    if val == 0 {
        return false;
    }
    let hook = lookup(name);
    match hook {
        Some(hook) => {
            hook(val);
            true
        }
        None => false,
    }
}
