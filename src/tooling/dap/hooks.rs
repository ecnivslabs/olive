//! Debug hook fast path. Every instrumented program calls these on every
//! statement/assign/enter/return; a program not under a debugger still
//! contains the calls (JIT-only, see `mir::debug_hooks`), so the check
//! against `DEBUGGEE_ENABLED` must be a single load-and-branch. These are
//! registered directly into the JIT via `builder.symbol`, never through
//! `SYMBOL_MAP` or olive_std, so a plain run never links against them.

use std::cell::Cell;

thread_local! {
    /// Set only on the spawned debuggee thread for the lifetime of a debug
    /// session. Runtime worker threads (thread pools, async executors) never
    /// set it, so hooks on those threads are a load, compare, return.
    static DEBUGGEE_ENABLED: Cell<bool> = const { Cell::new(false) };
}

// D2 replaces this file with real frame/breakpoint dispatch behind this
// same guard; the early return looks needless only because nothing follows
// it yet.
#[allow(clippy::needless_return)]
pub extern "C" fn debug_stmt(_packed: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
}

#[allow(clippy::needless_return)]
pub extern "C" fn debug_enter(_fn_id: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
}

#[allow(clippy::needless_return)]
pub extern "C" fn debug_store(_cell_idx: i64, _value: i64) {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
}

#[allow(clippy::needless_return)]
pub extern "C" fn debug_exit() {
    if !DEBUGGEE_ENABLED.get() {
        return;
    }
}

/// Symbols registered unconditionally into every JIT module. Nothing calls
/// through them unless a debug session instrumented the MIR first, so a
/// registered-but-unused symbol costs nothing at runtime.
pub fn jit_symbols() -> [(&'static str, *const u8); 4] {
    [
        ("__olive_debug_stmt", debug_stmt as *const u8),
        ("__olive_debug_enter", debug_enter as *const u8),
        ("__olive_debug_store", debug_store as *const u8),
        ("__olive_debug_exit", debug_exit as *const u8),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn instrumented_program_runs_to_exit_code_zero() {
        let (mut jit, program) = crate::test_utils::compile_instrumented(
            "fn add(a: int, b: int) -> int:\n    return a + b\nfn main():\n    print(add(1, 2))\n",
        );
        assert!(program.functions.iter().any(|f| f.name == "add"));
        assert!(program.functions.iter().any(|f| f.name == "main"));

        let ptr = jit.get_function("__main__").expect("__main__ not found");
        let main_fn: extern "C" fn() -> i64 = unsafe { std::mem::transmute(ptr) };
        let _guard = crate::test_utils::exec_lock();
        assert_eq!(main_fn(), 0);
    }
}
