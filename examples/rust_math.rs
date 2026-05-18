#[no_mangle]
pub extern "C" fn compute_fast_hash(input: i64) -> i64 {
    (input ^ 0xDEADBEEF).wrapping_mul(0x1337)
}
