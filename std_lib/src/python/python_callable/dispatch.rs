//! Arity/scalar-shape dispatch for calling a closure thunk from Rust.
//!
//! Every Olive scalar crosses the trampoline/thunk boundary as one i64 word.
//! The thunk still uses its native ABI, so integer, f64, and f32 slots need
//! distinct monomorphic function signatures.

macro_rules! slot_ty {
    (I) => {
        i64
    };
    (F) => {
        f64
    };
    (F32) => {
        f32
    };
}
macro_rules! slot_val {
    (I, $v:expr) => {
        $v
    };
    (F, $v:expr) => {
        f64::from_bits($v as u64)
    };
    (F32, $v:expr) => {
        f32::from_bits($v as u32)
    };
}
macro_rules! ret_val {
    (I, $v:expr) => {
        $v
    };
    (F, $v:expr) => {
        $v.to_bits() as i64
    };
    (F32, $v:expr) => {
        $v.to_bits() as i64
    };
}
macro_rules! shape_call {
    ($thunk:expr, $env:expr, $ret:tt $(; $($k:tt : $v:expr),+)?) => {{
        type ThunkFn = unsafe extern "C" fn($($(slot_ty!($k),)+)? i64) -> slot_ty!($ret);
        let f: ThunkFn = unsafe { std::mem::transmute($thunk as usize) };
        let raw = unsafe { f($($(slot_val!($k, $v),)+)? $env) };
        ret_val!($ret, raw)
    }};
}

pub(super) fn invoke_thunk(
    thunk: i64,
    env: i64,
    args: &[i64],
    arg_kinds: [u8; 4],
    ret_kind: u8,
) -> i64 {
    match args.len() {
        0 => match ret_kind {
            0 => shape_call!(thunk, env, I),
            1 => shape_call!(thunk, env, F),
            2 => shape_call!(thunk, env, F32),
            _ => unreachable!("invalid callable return kind"),
        },
        1 => {
            let a0 = arg_kinds[0];
            match (a0, ret_kind) {
                (0, 0) => shape_call!(thunk, env, I; I: args[0]),
                (0, 1) => shape_call!(thunk, env, F; I: args[0]),
                (0, 2) => shape_call!(thunk, env, F32; I: args[0]),
                (1, 0) => shape_call!(thunk, env, I; F: args[0]),
                (1, 1) => shape_call!(thunk, env, F; F: args[0]),
                (1, 2) => shape_call!(thunk, env, F32; F: args[0]),
                (2, 0) => shape_call!(thunk, env, I; F32: args[0]),
                (2, 1) => shape_call!(thunk, env, F; F32: args[0]),
                (2, 2) => shape_call!(thunk, env, F32; F32: args[0]),
                _ => unreachable!("invalid callable scalar shape"),
            }
        }
        2 => {
            let a0 = arg_kinds[0];
            let a1 = arg_kinds[1];
            match (a0, a1, ret_kind) {
                (0, 0, 0) => shape_call!(thunk, env, I; I: args[0], I: args[1]),
                (0, 0, 1) => shape_call!(thunk, env, F; I: args[0], I: args[1]),
                (0, 0, 2) => shape_call!(thunk, env, F32; I: args[0], I: args[1]),
                (0, 1, 0) => shape_call!(thunk, env, I; I: args[0], F: args[1]),
                (0, 1, 1) => shape_call!(thunk, env, F; I: args[0], F: args[1]),
                (0, 1, 2) => shape_call!(thunk, env, F32; I: args[0], F: args[1]),
                (0, 2, 0) => shape_call!(thunk, env, I; I: args[0], F32: args[1]),
                (0, 2, 1) => shape_call!(thunk, env, F; I: args[0], F32: args[1]),
                (0, 2, 2) => shape_call!(thunk, env, F32; I: args[0], F32: args[1]),
                (1, 0, 0) => shape_call!(thunk, env, I; F: args[0], I: args[1]),
                (1, 0, 1) => shape_call!(thunk, env, F; F: args[0], I: args[1]),
                (1, 0, 2) => shape_call!(thunk, env, F32; F: args[0], I: args[1]),
                (1, 1, 0) => shape_call!(thunk, env, I; F: args[0], F: args[1]),
                (1, 1, 1) => shape_call!(thunk, env, F; F: args[0], F: args[1]),
                (1, 1, 2) => shape_call!(thunk, env, F32; F: args[0], F: args[1]),
                (1, 2, 0) => shape_call!(thunk, env, I; F: args[0], F32: args[1]),
                (1, 2, 1) => shape_call!(thunk, env, F; F: args[0], F32: args[1]),
                (1, 2, 2) => shape_call!(thunk, env, F32; F: args[0], F32: args[1]),
                (2, 0, 0) => shape_call!(thunk, env, I; F32: args[0], I: args[1]),
                (2, 0, 1) => shape_call!(thunk, env, F; F32: args[0], I: args[1]),
                (2, 0, 2) => shape_call!(thunk, env, F32; F32: args[0], I: args[1]),
                (2, 1, 0) => shape_call!(thunk, env, I; F32: args[0], F: args[1]),
                (2, 1, 1) => shape_call!(thunk, env, F; F32: args[0], F: args[1]),
                (2, 1, 2) => shape_call!(thunk, env, F32; F32: args[0], F: args[1]),
                (2, 2, 0) => shape_call!(thunk, env, I; F32: args[0], F32: args[1]),
                (2, 2, 1) => shape_call!(thunk, env, F; F32: args[0], F32: args[1]),
                (2, 2, 2) => shape_call!(thunk, env, F32; F32: args[0], F32: args[1]),
                _ => unreachable!("invalid callable scalar shape"),
            }
        }
        3 => {
            let a0 = arg_kinds[0];
            let a1 = arg_kinds[1];
            let a2 = arg_kinds[2];
            match (a0, a1, a2, ret_kind) {
                (0, 0, 0, 0) => shape_call!(thunk, env, I; I: args[0], I: args[1], I: args[2]),
                (0, 0, 0, 1) => shape_call!(thunk, env, F; I: args[0], I: args[1], I: args[2]),
                (0, 0, 0, 2) => shape_call!(thunk, env, F32; I: args[0], I: args[1], I: args[2]),
                (0, 0, 1, 0) => shape_call!(thunk, env, I; I: args[0], I: args[1], F: args[2]),
                (0, 0, 1, 1) => shape_call!(thunk, env, F; I: args[0], I: args[1], F: args[2]),
                (0, 0, 1, 2) => shape_call!(thunk, env, F32; I: args[0], I: args[1], F: args[2]),
                (0, 0, 2, 0) => shape_call!(thunk, env, I; I: args[0], I: args[1], F32: args[2]),
                (0, 0, 2, 1) => shape_call!(thunk, env, F; I: args[0], I: args[1], F32: args[2]),
                (0, 0, 2, 2) => shape_call!(thunk, env, F32; I: args[0], I: args[1], F32: args[2]),
                (0, 1, 0, 0) => shape_call!(thunk, env, I; I: args[0], F: args[1], I: args[2]),
                (0, 1, 0, 1) => shape_call!(thunk, env, F; I: args[0], F: args[1], I: args[2]),
                (0, 1, 0, 2) => shape_call!(thunk, env, F32; I: args[0], F: args[1], I: args[2]),
                (0, 1, 1, 0) => shape_call!(thunk, env, I; I: args[0], F: args[1], F: args[2]),
                (0, 1, 1, 1) => shape_call!(thunk, env, F; I: args[0], F: args[1], F: args[2]),
                (0, 1, 1, 2) => shape_call!(thunk, env, F32; I: args[0], F: args[1], F: args[2]),
                (0, 1, 2, 0) => shape_call!(thunk, env, I; I: args[0], F: args[1], F32: args[2]),
                (0, 1, 2, 1) => shape_call!(thunk, env, F; I: args[0], F: args[1], F32: args[2]),
                (0, 1, 2, 2) => shape_call!(thunk, env, F32; I: args[0], F: args[1], F32: args[2]),
                (0, 2, 0, 0) => shape_call!(thunk, env, I; I: args[0], F32: args[1], I: args[2]),
                (0, 2, 0, 1) => shape_call!(thunk, env, F; I: args[0], F32: args[1], I: args[2]),
                (0, 2, 0, 2) => shape_call!(thunk, env, F32; I: args[0], F32: args[1], I: args[2]),
                (0, 2, 1, 0) => shape_call!(thunk, env, I; I: args[0], F32: args[1], F: args[2]),
                (0, 2, 1, 1) => shape_call!(thunk, env, F; I: args[0], F32: args[1], F: args[2]),
                (0, 2, 1, 2) => shape_call!(thunk, env, F32; I: args[0], F32: args[1], F: args[2]),
                (0, 2, 2, 0) => shape_call!(thunk, env, I; I: args[0], F32: args[1], F32: args[2]),
                (0, 2, 2, 1) => shape_call!(thunk, env, F; I: args[0], F32: args[1], F32: args[2]),
                (0, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], F32: args[2])
                }
                (1, 0, 0, 0) => shape_call!(thunk, env, I; F: args[0], I: args[1], I: args[2]),
                (1, 0, 0, 1) => shape_call!(thunk, env, F; F: args[0], I: args[1], I: args[2]),
                (1, 0, 0, 2) => shape_call!(thunk, env, F32; F: args[0], I: args[1], I: args[2]),
                (1, 0, 1, 0) => shape_call!(thunk, env, I; F: args[0], I: args[1], F: args[2]),
                (1, 0, 1, 1) => shape_call!(thunk, env, F; F: args[0], I: args[1], F: args[2]),
                (1, 0, 1, 2) => shape_call!(thunk, env, F32; F: args[0], I: args[1], F: args[2]),
                (1, 0, 2, 0) => shape_call!(thunk, env, I; F: args[0], I: args[1], F32: args[2]),
                (1, 0, 2, 1) => shape_call!(thunk, env, F; F: args[0], I: args[1], F32: args[2]),
                (1, 0, 2, 2) => shape_call!(thunk, env, F32; F: args[0], I: args[1], F32: args[2]),
                (1, 1, 0, 0) => shape_call!(thunk, env, I; F: args[0], F: args[1], I: args[2]),
                (1, 1, 0, 1) => shape_call!(thunk, env, F; F: args[0], F: args[1], I: args[2]),
                (1, 1, 0, 2) => shape_call!(thunk, env, F32; F: args[0], F: args[1], I: args[2]),
                (1, 1, 1, 0) => shape_call!(thunk, env, I; F: args[0], F: args[1], F: args[2]),
                (1, 1, 1, 1) => shape_call!(thunk, env, F; F: args[0], F: args[1], F: args[2]),
                (1, 1, 1, 2) => shape_call!(thunk, env, F32; F: args[0], F: args[1], F: args[2]),
                (1, 1, 2, 0) => shape_call!(thunk, env, I; F: args[0], F: args[1], F32: args[2]),
                (1, 1, 2, 1) => shape_call!(thunk, env, F; F: args[0], F: args[1], F32: args[2]),
                (1, 1, 2, 2) => shape_call!(thunk, env, F32; F: args[0], F: args[1], F32: args[2]),
                (1, 2, 0, 0) => shape_call!(thunk, env, I; F: args[0], F32: args[1], I: args[2]),
                (1, 2, 0, 1) => shape_call!(thunk, env, F; F: args[0], F32: args[1], I: args[2]),
                (1, 2, 0, 2) => shape_call!(thunk, env, F32; F: args[0], F32: args[1], I: args[2]),
                (1, 2, 1, 0) => shape_call!(thunk, env, I; F: args[0], F32: args[1], F: args[2]),
                (1, 2, 1, 1) => shape_call!(thunk, env, F; F: args[0], F32: args[1], F: args[2]),
                (1, 2, 1, 2) => shape_call!(thunk, env, F32; F: args[0], F32: args[1], F: args[2]),
                (1, 2, 2, 0) => shape_call!(thunk, env, I; F: args[0], F32: args[1], F32: args[2]),
                (1, 2, 2, 1) => shape_call!(thunk, env, F; F: args[0], F32: args[1], F32: args[2]),
                (1, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], F32: args[2])
                }
                (2, 0, 0, 0) => shape_call!(thunk, env, I; F32: args[0], I: args[1], I: args[2]),
                (2, 0, 0, 1) => shape_call!(thunk, env, F; F32: args[0], I: args[1], I: args[2]),
                (2, 0, 0, 2) => shape_call!(thunk, env, F32; F32: args[0], I: args[1], I: args[2]),
                (2, 0, 1, 0) => shape_call!(thunk, env, I; F32: args[0], I: args[1], F: args[2]),
                (2, 0, 1, 1) => shape_call!(thunk, env, F; F32: args[0], I: args[1], F: args[2]),
                (2, 0, 1, 2) => shape_call!(thunk, env, F32; F32: args[0], I: args[1], F: args[2]),
                (2, 0, 2, 0) => shape_call!(thunk, env, I; F32: args[0], I: args[1], F32: args[2]),
                (2, 0, 2, 1) => shape_call!(thunk, env, F; F32: args[0], I: args[1], F32: args[2]),
                (2, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], F32: args[2])
                }
                (2, 1, 0, 0) => shape_call!(thunk, env, I; F32: args[0], F: args[1], I: args[2]),
                (2, 1, 0, 1) => shape_call!(thunk, env, F; F32: args[0], F: args[1], I: args[2]),
                (2, 1, 0, 2) => shape_call!(thunk, env, F32; F32: args[0], F: args[1], I: args[2]),
                (2, 1, 1, 0) => shape_call!(thunk, env, I; F32: args[0], F: args[1], F: args[2]),
                (2, 1, 1, 1) => shape_call!(thunk, env, F; F32: args[0], F: args[1], F: args[2]),
                (2, 1, 1, 2) => shape_call!(thunk, env, F32; F32: args[0], F: args[1], F: args[2]),
                (2, 1, 2, 0) => shape_call!(thunk, env, I; F32: args[0], F: args[1], F32: args[2]),
                (2, 1, 2, 1) => shape_call!(thunk, env, F; F32: args[0], F: args[1], F32: args[2]),
                (2, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], F32: args[2])
                }
                (2, 2, 0, 0) => shape_call!(thunk, env, I; F32: args[0], F32: args[1], I: args[2]),
                (2, 2, 0, 1) => shape_call!(thunk, env, F; F32: args[0], F32: args[1], I: args[2]),
                (2, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], I: args[2])
                }
                (2, 2, 1, 0) => shape_call!(thunk, env, I; F32: args[0], F32: args[1], F: args[2]),
                (2, 2, 1, 1) => shape_call!(thunk, env, F; F32: args[0], F32: args[1], F: args[2]),
                (2, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F: args[2])
                }
                (2, 2, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], F32: args[2])
                }
                (2, 2, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], F32: args[2])
                }
                (2, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F32: args[2])
                }
                _ => unreachable!("invalid callable scalar shape"),
            }
        }
        4 => {
            let a0 = arg_kinds[0];
            let a1 = arg_kinds[1];
            let a2 = arg_kinds[2];
            let a3 = arg_kinds[3];
            match (a0, a1, a2, a3, ret_kind) {
                (0, 0, 0, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], I: args[2], I: args[3])
                }
                (0, 0, 0, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], I: args[2], I: args[3])
                }
                (0, 0, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], I: args[2], I: args[3])
                }
                (0, 0, 0, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], I: args[2], F: args[3])
                }
                (0, 0, 0, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], I: args[2], F: args[3])
                }
                (0, 0, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], I: args[2], F: args[3])
                }
                (0, 0, 0, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (0, 0, 0, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (0, 0, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (0, 0, 1, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], F: args[2], I: args[3])
                }
                (0, 0, 1, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], F: args[2], I: args[3])
                }
                (0, 0, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], F: args[2], I: args[3])
                }
                (0, 0, 1, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], F: args[2], F: args[3])
                }
                (0, 0, 1, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], F: args[2], F: args[3])
                }
                (0, 0, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], F: args[2], F: args[3])
                }
                (0, 0, 1, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (0, 0, 1, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (0, 0, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (0, 0, 2, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (0, 0, 2, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (0, 0, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (0, 0, 2, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (0, 0, 2, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (0, 0, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (0, 0, 2, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (0, 0, 2, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (0, 0, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (0, 1, 0, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], I: args[2], I: args[3])
                }
                (0, 1, 0, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], I: args[2], I: args[3])
                }
                (0, 1, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], I: args[2], I: args[3])
                }
                (0, 1, 0, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], I: args[2], F: args[3])
                }
                (0, 1, 0, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], I: args[2], F: args[3])
                }
                (0, 1, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], I: args[2], F: args[3])
                }
                (0, 1, 0, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (0, 1, 0, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (0, 1, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (0, 1, 1, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], F: args[2], I: args[3])
                }
                (0, 1, 1, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], F: args[2], I: args[3])
                }
                (0, 1, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], F: args[2], I: args[3])
                }
                (0, 1, 1, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], F: args[2], F: args[3])
                }
                (0, 1, 1, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], F: args[2], F: args[3])
                }
                (0, 1, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], F: args[2], F: args[3])
                }
                (0, 1, 1, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (0, 1, 1, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (0, 1, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (0, 1, 2, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (0, 1, 2, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (0, 1, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (0, 1, 2, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (0, 1, 2, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (0, 1, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (0, 1, 2, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (0, 1, 2, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (0, 1, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (0, 2, 0, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (0, 2, 0, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (0, 2, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (0, 2, 0, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (0, 2, 0, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (0, 2, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (0, 2, 0, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (0, 2, 0, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (0, 2, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (0, 2, 1, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (0, 2, 1, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (0, 2, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (0, 2, 1, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (0, 2, 1, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (0, 2, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (0, 2, 1, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (0, 2, 1, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (0, 2, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (0, 2, 2, 0, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (0, 2, 2, 0, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (0, 2, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (0, 2, 2, 1, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (0, 2, 2, 1, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (0, 2, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (0, 2, 2, 2, 0) => {
                    shape_call!(thunk, env, I; I: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (0, 2, 2, 2, 1) => {
                    shape_call!(thunk, env, F; I: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (0, 2, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; I: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (1, 0, 0, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], I: args[2], I: args[3])
                }
                (1, 0, 0, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], I: args[2], I: args[3])
                }
                (1, 0, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], I: args[2], I: args[3])
                }
                (1, 0, 0, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], I: args[2], F: args[3])
                }
                (1, 0, 0, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], I: args[2], F: args[3])
                }
                (1, 0, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], I: args[2], F: args[3])
                }
                (1, 0, 0, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (1, 0, 0, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (1, 0, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (1, 0, 1, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], F: args[2], I: args[3])
                }
                (1, 0, 1, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], F: args[2], I: args[3])
                }
                (1, 0, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], F: args[2], I: args[3])
                }
                (1, 0, 1, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], F: args[2], F: args[3])
                }
                (1, 0, 1, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], F: args[2], F: args[3])
                }
                (1, 0, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], F: args[2], F: args[3])
                }
                (1, 0, 1, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (1, 0, 1, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (1, 0, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (1, 0, 2, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (1, 0, 2, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (1, 0, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (1, 0, 2, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (1, 0, 2, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (1, 0, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (1, 0, 2, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (1, 0, 2, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (1, 0, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (1, 1, 0, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], I: args[2], I: args[3])
                }
                (1, 1, 0, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], I: args[2], I: args[3])
                }
                (1, 1, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], I: args[2], I: args[3])
                }
                (1, 1, 0, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], I: args[2], F: args[3])
                }
                (1, 1, 0, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], I: args[2], F: args[3])
                }
                (1, 1, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], I: args[2], F: args[3])
                }
                (1, 1, 0, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (1, 1, 0, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (1, 1, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (1, 1, 1, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], F: args[2], I: args[3])
                }
                (1, 1, 1, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], F: args[2], I: args[3])
                }
                (1, 1, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], F: args[2], I: args[3])
                }
                (1, 1, 1, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], F: args[2], F: args[3])
                }
                (1, 1, 1, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], F: args[2], F: args[3])
                }
                (1, 1, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], F: args[2], F: args[3])
                }
                (1, 1, 1, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (1, 1, 1, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (1, 1, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (1, 1, 2, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (1, 1, 2, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (1, 1, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (1, 1, 2, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (1, 1, 2, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (1, 1, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (1, 1, 2, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (1, 1, 2, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (1, 1, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (1, 2, 0, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (1, 2, 0, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (1, 2, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (1, 2, 0, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (1, 2, 0, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (1, 2, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (1, 2, 0, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (1, 2, 0, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (1, 2, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (1, 2, 1, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (1, 2, 1, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (1, 2, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (1, 2, 1, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (1, 2, 1, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (1, 2, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (1, 2, 1, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (1, 2, 1, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (1, 2, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (1, 2, 2, 0, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (1, 2, 2, 0, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (1, 2, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (1, 2, 2, 1, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (1, 2, 2, 1, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (1, 2, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (1, 2, 2, 2, 0) => {
                    shape_call!(thunk, env, I; F: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (1, 2, 2, 2, 1) => {
                    shape_call!(thunk, env, F; F: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (1, 2, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (2, 0, 0, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], I: args[2], I: args[3])
                }
                (2, 0, 0, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], I: args[2], I: args[3])
                }
                (2, 0, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], I: args[2], I: args[3])
                }
                (2, 0, 0, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], I: args[2], F: args[3])
                }
                (2, 0, 0, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], I: args[2], F: args[3])
                }
                (2, 0, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], I: args[2], F: args[3])
                }
                (2, 0, 0, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (2, 0, 0, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (2, 0, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], I: args[2], F32: args[3])
                }
                (2, 0, 1, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], F: args[2], I: args[3])
                }
                (2, 0, 1, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], F: args[2], I: args[3])
                }
                (2, 0, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], F: args[2], I: args[3])
                }
                (2, 0, 1, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], F: args[2], F: args[3])
                }
                (2, 0, 1, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], F: args[2], F: args[3])
                }
                (2, 0, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], F: args[2], F: args[3])
                }
                (2, 0, 1, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (2, 0, 1, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (2, 0, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], F: args[2], F32: args[3])
                }
                (2, 0, 2, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (2, 0, 2, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (2, 0, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], F32: args[2], I: args[3])
                }
                (2, 0, 2, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (2, 0, 2, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (2, 0, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], F32: args[2], F: args[3])
                }
                (2, 0, 2, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (2, 0, 2, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (2, 0, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], I: args[1], F32: args[2], F32: args[3])
                }
                (2, 1, 0, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], I: args[2], I: args[3])
                }
                (2, 1, 0, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], I: args[2], I: args[3])
                }
                (2, 1, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], I: args[2], I: args[3])
                }
                (2, 1, 0, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], I: args[2], F: args[3])
                }
                (2, 1, 0, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], I: args[2], F: args[3])
                }
                (2, 1, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], I: args[2], F: args[3])
                }
                (2, 1, 0, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (2, 1, 0, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (2, 1, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], I: args[2], F32: args[3])
                }
                (2, 1, 1, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], F: args[2], I: args[3])
                }
                (2, 1, 1, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], F: args[2], I: args[3])
                }
                (2, 1, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], F: args[2], I: args[3])
                }
                (2, 1, 1, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], F: args[2], F: args[3])
                }
                (2, 1, 1, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], F: args[2], F: args[3])
                }
                (2, 1, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], F: args[2], F: args[3])
                }
                (2, 1, 1, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (2, 1, 1, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (2, 1, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], F: args[2], F32: args[3])
                }
                (2, 1, 2, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (2, 1, 2, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (2, 1, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], F32: args[2], I: args[3])
                }
                (2, 1, 2, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (2, 1, 2, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (2, 1, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], F32: args[2], F: args[3])
                }
                (2, 1, 2, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (2, 1, 2, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (2, 1, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F: args[1], F32: args[2], F32: args[3])
                }
                (2, 2, 0, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (2, 2, 0, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (2, 2, 0, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], I: args[2], I: args[3])
                }
                (2, 2, 0, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (2, 2, 0, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (2, 2, 0, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], I: args[2], F: args[3])
                }
                (2, 2, 0, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (2, 2, 0, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (2, 2, 0, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], I: args[2], F32: args[3])
                }
                (2, 2, 1, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (2, 2, 1, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (2, 2, 1, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F: args[2], I: args[3])
                }
                (2, 2, 1, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (2, 2, 1, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (2, 2, 1, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F: args[2], F: args[3])
                }
                (2, 2, 1, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (2, 2, 1, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (2, 2, 1, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F: args[2], F32: args[3])
                }
                (2, 2, 2, 0, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (2, 2, 2, 0, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (2, 2, 2, 0, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F32: args[2], I: args[3])
                }
                (2, 2, 2, 1, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (2, 2, 2, 1, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (2, 2, 2, 1, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F32: args[2], F: args[3])
                }
                (2, 2, 2, 2, 0) => {
                    shape_call!(thunk, env, I; F32: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (2, 2, 2, 2, 1) => {
                    shape_call!(thunk, env, F; F32: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                (2, 2, 2, 2, 2) => {
                    shape_call!(thunk, env, F32; F32: args[0], F32: args[1], F32: args[2], F32: args[3])
                }
                _ => unreachable!("invalid callable scalar shape"),
            }
        }
        n => unreachable!("invoke_thunk: arity {n} out of range (max 4)"),
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
    unsafe extern "C" fn thunk_f32_1(a: f32, _env: i64) -> f32 {
        a + 1.0
    }
    unsafe extern "C" fn thunk_mixed(a: i64, b: f64, env: i64) -> f64 {
        a as f64 + b + env as f64
    }

    #[test]
    fn arity0_all_int() {
        let r = invoke_thunk(thunk_i0 as *const () as i64, 41, &[], [0, 0, 0, 0], 0);
        assert_eq!(r, 42);
    }

    #[test]
    fn arity2_all_int() {
        let r = invoke_thunk(thunk_i2 as *const () as i64, 10, &[1, 2], [0, 0, 0, 0], 0);
        assert_eq!(r, 13);
    }

    #[test]
    fn arity1_float_param_float_ret() {
        let r = invoke_thunk(
            thunk_f1 as *const () as i64,
            5,
            &[3.5f64.to_bits() as i64],
            [1, 0, 0, 0],
            1,
        );
        assert_eq!(f64::from_bits(r as u64), 12.0);
    }

    #[test]
    fn arity1_f32_param_f32_ret() {
        let r = invoke_thunk(
            thunk_f32_1 as *const () as i64,
            0,
            &[(2.5f32).to_bits() as i64],
            [2, 0, 0, 0],
            2,
        );
        assert_eq!(f32::from_bits(r as u32), 3.5);
    }

    #[test]
    fn arity2_mixed_int_float() {
        let args = [7i64, 1.5f64.to_bits() as i64];
        let r = invoke_thunk(thunk_mixed as *const () as i64, 2, &args, [0, 1, 0, 0], 1);
        assert_eq!(f64::from_bits(r as u64), 10.5);
    }
}
