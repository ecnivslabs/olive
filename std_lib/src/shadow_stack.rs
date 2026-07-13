//! Roadmap E13.2: a call chain for a fault in the debug (`pit run`) pipeline.
//! The MIR builder's JIT-only instrumentation pass (`mir::shadow_stack`)
//! wraps every statically-known Olive-to-Olive call with a push/pop around
//! it; a fault mid-chain finds the stack still holding every frame between
//! it and `main`. AOT release never emits the push/pop calls, so this stays
//! permanently empty there -- `render` is a single length check away from a
//! no-op, satisfying "Release AOT stays caret-only (zero cost)" without a
//! second code path.
//!
//! Fires on every instrumented call, so the two calls it costs on top of the
//! callee itself have to stay cheap: a fixed-size ring (no heap growth, no
//! `RefCell` borrow flag) past a small `Cell<usize>` depth, not a `Vec`. Past
//! `CAP` nested frames the ring overwrites the oldest (outermost) entry first,
//! so a fault always sees the frames closest to it -- the ones actually worth
//! showing -- never the ones farthest away.

use crate::olive_str_from_ptr;
use std::cell::{Cell, UnsafeCell};
use std::io::Write;

const CAP: usize = 1024;

struct Ring {
    frames: UnsafeCell<[(i64, i64); CAP]>,
    depth: Cell<usize>,
}

thread_local! {
    static STACK: Ring = const {
        Ring {
            frames: UnsafeCell::new([(0, 0); CAP]),
            depth: Cell::new(0),
        }
    };
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_shadow_push(name: i64, loc: i64) {
    STACK.with(|s| {
        let d = s.depth.get();
        // Sound: this thread never re-enters here (push/pop are plain
        // non-reentrant calls straddling a single callee), so no other
        // reference to `frames` is ever live at the same time.
        unsafe {
            (*s.frames.get())[d % CAP] = (name, loc);
        }
        s.depth.set(d + 1);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_shadow_pop() {
    STACK.with(|s| {
        let d = s.depth.get();
        if d > 0 {
            s.depth.set(d - 1);
        }
    });
}

/// Prints the active call chain under the caret, innermost (closest to the
/// fault) first. A no-op when the stack is empty: a leaf-level fault with no
/// intervening calls, or any AOT release build.
pub fn render(out: &mut impl Write, dim: &str, reset: &str, color: bool) {
    STACK.with(|s| {
        let depth = s.depth.get();
        if depth == 0 {
            return;
        }
        let n = depth.min(CAP);
        let red = if color { "\x1b[31m" } else { "" };
        let _ = writeln!(out, "{dim}  │{reset}");
        let _ = writeln!(out, "{dim}  │{reset} stack (innermost first):");
        for i in 0..n {
            let slot = (depth - 1 - i) % CAP;
            // Sound for the same reason as the push side: nothing else
            // touches `frames` while a fault is being rendered.
            let (name, loc) = unsafe { (*s.frames.get())[slot] };
            let name = olive_str_from_ptr(name);
            let loc = olive_str_from_ptr(loc);
            let _ = writeln!(
                out,
                "{dim}  │{reset}   {i}: {red}{name}{reset} ({dim}{loc}{reset})"
            );
        }
        if depth > CAP {
            let _ = writeln!(out, "{dim}  │{reset}   ... {} more frame(s)", depth - CAP);
        }
    });
}
