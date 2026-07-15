//! Arity/float-shape dispatch for calling a closure thunk from Rust.
//!
//! Every olive scalar crosses the trampoline<->thunk boundary as one i64,
//! except a `float`/`f32` one: the thunk is ordinary Cranelift-compiled
//! code, so its real parameter/return registers follow the native SysV
//! convention (float in XMM, everything else in a GPR) exactly like any
//! other Olive function -- see `translate_call.rs`'s comment on the same
//! register-mismatch hazard for indirect calls. Calling it through a
//! `transmute`'d function pointer therefore needs the *exact* Rust
//! signature per slot, so every reachable (arity, float positions,
//! float return) shape gets its own monomorphic call below. `float_mask`
//! bit `i` set means `args[i]` is a float bit pattern (`f64::to_bits`);
//! `args[i]` always carries the raw i64 word either way.

macro_rules! slot_ty {
    (I) => {
        i64
    };
    (F) => {
        f64
    };
}
macro_rules! slot_val {
    (I, $v:expr) => {
        $v
    };
    (F, $v:expr) => {
        f64::from_bits($v as u64)
    };
}
macro_rules! ret_val {
    (I, $v:expr) => {
        $v
    };
    (F, $v:expr) => {
        $v.to_bits() as i64
    };
}

/// Builds the monomorphic `extern "C" fn(..) -> ..` matching one shape,
/// transmutes `thunk` to it, and calls it with `env` as the trailing
/// (hidden) closure-record argument every thunk takes.
macro_rules! shape_call {
    ($thunk:expr, $env:expr, $ret:tt $(; $($k:tt : $v:expr),+)?) => {{
        type ThunkFn = unsafe extern "C" fn($($(slot_ty!($k),)+)? i64) -> slot_ty!($ret);
        let f: ThunkFn = unsafe { std::mem::transmute($thunk as usize) };
        let raw = unsafe { f($($(slot_val!($k, $v),)+)? $env) };
        ret_val!($ret, raw)
    }};
}

/// Calls `thunk` (a closure record's `__thunk` field, arity `args.len()`)
/// with `env` (the record pointer itself) as the trailing hidden argument,
/// returning the raw result as an i64 word (a float result's bit pattern
/// when `ret_is_float`).
pub(super) fn invoke_thunk(
    thunk: i64,
    env: i64,
    args: &[i64],
    float_mask: u8,
    ret_is_float: bool,
) -> i64 {
    match (args.len(), float_mask, ret_is_float) {
        (0, _, false) => shape_call!(thunk, env, I),
        (0, _, true) => shape_call!(thunk, env, F),

        (1, 0b0, false) => shape_call!(thunk, env, I; I: args[0]),
        (1, 0b0, true) => shape_call!(thunk, env, F; I: args[0]),
        (1, 0b1, false) => shape_call!(thunk, env, I; F: args[0]),
        (1, 0b1, true) => shape_call!(thunk, env, F; F: args[0]),

        (2, 0b00, false) => shape_call!(thunk, env, I; I: args[0], I: args[1]),
        (2, 0b00, true) => shape_call!(thunk, env, F; I: args[0], I: args[1]),
        (2, 0b01, false) => shape_call!(thunk, env, I; F: args[0], I: args[1]),
        (2, 0b01, true) => shape_call!(thunk, env, F; F: args[0], I: args[1]),
        (2, 0b10, false) => shape_call!(thunk, env, I; I: args[0], F: args[1]),
        (2, 0b10, true) => shape_call!(thunk, env, F; I: args[0], F: args[1]),
        (2, 0b11, false) => shape_call!(thunk, env, I; F: args[0], F: args[1]),
        (2, 0b11, true) => shape_call!(thunk, env, F; F: args[0], F: args[1]),

        (3, m, false) if m < 8 => match m {
            0b000 => shape_call!(thunk, env, I; I: args[0], I: args[1], I: args[2]),
            0b001 => shape_call!(thunk, env, I; F: args[0], I: args[1], I: args[2]),
            0b010 => shape_call!(thunk, env, I; I: args[0], F: args[1], I: args[2]),
            0b011 => shape_call!(thunk, env, I; F: args[0], F: args[1], I: args[2]),
            0b100 => shape_call!(thunk, env, I; I: args[0], I: args[1], F: args[2]),
            0b101 => shape_call!(thunk, env, I; F: args[0], I: args[1], F: args[2]),
            0b110 => shape_call!(thunk, env, I; I: args[0], F: args[1], F: args[2]),
            0b111 => shape_call!(thunk, env, I; F: args[0], F: args[1], F: args[2]),
            _ => unreachable!(),
        },
        (3, m, true) if m < 8 => match m {
            0b000 => shape_call!(thunk, env, F; I: args[0], I: args[1], I: args[2]),
            0b001 => shape_call!(thunk, env, F; F: args[0], I: args[1], I: args[2]),
            0b010 => shape_call!(thunk, env, F; I: args[0], F: args[1], I: args[2]),
            0b011 => shape_call!(thunk, env, F; F: args[0], F: args[1], I: args[2]),
            0b100 => shape_call!(thunk, env, F; I: args[0], I: args[1], F: args[2]),
            0b101 => shape_call!(thunk, env, F; F: args[0], I: args[1], F: args[2]),
            0b110 => shape_call!(thunk, env, F; I: args[0], F: args[1], F: args[2]),
            0b111 => shape_call!(thunk, env, F; F: args[0], F: args[1], F: args[2]),
            _ => unreachable!(),
        },

        (4, m, false) if m < 16 => match m {
            0b0000 => shape_call!(thunk, env, I; I: args[0], I: args[1], I: args[2], I: args[3]),
            0b0001 => shape_call!(thunk, env, I; F: args[0], I: args[1], I: args[2], I: args[3]),
            0b0010 => shape_call!(thunk, env, I; I: args[0], F: args[1], I: args[2], I: args[3]),
            0b0011 => shape_call!(thunk, env, I; F: args[0], F: args[1], I: args[2], I: args[3]),
            0b0100 => shape_call!(thunk, env, I; I: args[0], I: args[1], F: args[2], I: args[3]),
            0b0101 => shape_call!(thunk, env, I; F: args[0], I: args[1], F: args[2], I: args[3]),
            0b0110 => shape_call!(thunk, env, I; I: args[0], F: args[1], F: args[2], I: args[3]),
            0b0111 => shape_call!(thunk, env, I; F: args[0], F: args[1], F: args[2], I: args[3]),
            0b1000 => shape_call!(thunk, env, I; I: args[0], I: args[1], I: args[2], F: args[3]),
            0b1001 => shape_call!(thunk, env, I; F: args[0], I: args[1], I: args[2], F: args[3]),
            0b1010 => shape_call!(thunk, env, I; I: args[0], F: args[1], I: args[2], F: args[3]),
            0b1011 => shape_call!(thunk, env, I; F: args[0], F: args[1], I: args[2], F: args[3]),
            0b1100 => shape_call!(thunk, env, I; I: args[0], I: args[1], F: args[2], F: args[3]),
            0b1101 => shape_call!(thunk, env, I; F: args[0], I: args[1], F: args[2], F: args[3]),
            0b1110 => shape_call!(thunk, env, I; I: args[0], F: args[1], F: args[2], F: args[3]),
            0b1111 => shape_call!(thunk, env, I; F: args[0], F: args[1], F: args[2], F: args[3]),
            _ => unreachable!(),
        },
        (4, m, true) if m < 16 => match m {
            0b0000 => shape_call!(thunk, env, F; I: args[0], I: args[1], I: args[2], I: args[3]),
            0b0001 => shape_call!(thunk, env, F; F: args[0], I: args[1], I: args[2], I: args[3]),
            0b0010 => shape_call!(thunk, env, F; I: args[0], F: args[1], I: args[2], I: args[3]),
            0b0011 => shape_call!(thunk, env, F; F: args[0], F: args[1], I: args[2], I: args[3]),
            0b0100 => shape_call!(thunk, env, F; I: args[0], I: args[1], F: args[2], I: args[3]),
            0b0101 => shape_call!(thunk, env, F; F: args[0], I: args[1], F: args[2], I: args[3]),
            0b0110 => shape_call!(thunk, env, F; I: args[0], F: args[1], F: args[2], I: args[3]),
            0b0111 => shape_call!(thunk, env, F; F: args[0], F: args[1], F: args[2], I: args[3]),
            0b1000 => shape_call!(thunk, env, F; I: args[0], I: args[1], I: args[2], F: args[3]),
            0b1001 => shape_call!(thunk, env, F; F: args[0], I: args[1], I: args[2], F: args[3]),
            0b1010 => shape_call!(thunk, env, F; I: args[0], F: args[1], I: args[2], F: args[3]),
            0b1011 => shape_call!(thunk, env, F; F: args[0], F: args[1], I: args[2], F: args[3]),
            0b1100 => shape_call!(thunk, env, F; I: args[0], I: args[1], F: args[2], F: args[3]),
            0b1101 => shape_call!(thunk, env, F; F: args[0], I: args[1], F: args[2], F: args[3]),
            0b1110 => shape_call!(thunk, env, F; I: args[0], F: args[1], F: args[2], F: args[3]),
            0b1111 => shape_call!(thunk, env, F; F: args[0], F: args[1], F: args[2], F: args[3]),
            _ => unreachable!(),
        },
        (n, ..) => unreachable!("invoke_thunk: arity {n} out of range (max 4)"),
    }
}

#[cfg(test)]
mod tests {
    use super::invoke_thunk;

    unsafe extern "C" fn thunk_i0(env: i64) -> i64 {
        env + 1
    }
    unsafe extern "C" fn thunk_i2(a: i64, b: i64, env: i64) -> i64 {
        a + b + env
    }
    unsafe extern "C" fn thunk_f1(a: f64, env: i64) -> f64 {
        a * 2.0 + env as f64
    }
    unsafe extern "C" fn thunk_mixed(a: i64, b: f64, env: i64) -> f64 {
        a as f64 + b + env as f64
    }

    #[test]
    fn arity0_all_int() {
        let r = invoke_thunk(thunk_i0 as *const () as i64, 41, &[], 0, false);
        assert_eq!(r, 42);
    }

    #[test]
    fn arity2_all_int() {
        let r = invoke_thunk(thunk_i2 as *const () as i64, 10, &[1, 2], 0b00, false);
        assert_eq!(r, 13);
    }

    #[test]
    fn arity1_float_param_float_ret() {
        let r = invoke_thunk(
            thunk_f1 as *const () as i64,
            5,
            &[3.5f64.to_bits() as i64],
            0b1,
            true,
        );
        assert_eq!(f64::from_bits(r as u64), 12.0);
    }

    #[test]
    fn arity2_mixed_int_float() {
        let args = [7i64, 1.5f64.to_bits() as i64];
        let r = invoke_thunk(thunk_mixed as *const () as i64, 2, &args, 0b10, true);
        assert_eq!(f64::from_bits(r as u64), 10.5);
    }
}
