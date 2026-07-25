use super::*;
use crate::slab::{ACTIVE_SLABS, SlabSet, slot_is_live};
use crate::string_slab::str_body;

#[test]
fn unwrap_or_releases_unused_error() {
    let message = olive_str_internal("unused error");
    let result = olive_result_err(message);
    assert_eq!(olive_result_unwrap_or(result, 42), 42);
    assert!(!slot_is_live(result));
    assert!(!slot_is_live(str_body(message)));
}

#[test]
fn err_msg_releases_unused_success() {
    let payload = olive_str_internal("unused success");
    let result = olive_result_ok(payload);
    assert_eq!(olive_result_err_msg(result), 0);
    assert!(!slot_is_live(result));
    assert!(!slot_is_live(str_body(payload)));
}

#[test]
fn consuming_accessors_recycle_into_active_task_slab() {
    let mut slabs = SlabSet::new();
    let previous = ACTIVE_SLABS.replace(&mut slabs);
    let result = olive_result_ok(42);
    let payload = olive_result_unwrap(result);
    let recycled = olive_result_ok(43);
    olive_free_result(recycled);
    ACTIVE_SLABS.set(previous);
    assert_eq!(payload, 42);
    assert_eq!(recycled, result);
}

#[test]
fn consuming_accessors_recycle_into_escape_arena() {
    let result = crate::slab::with_escape_arena(|| olive_result_ok(42));
    assert_eq!(olive_result_unwrap(result), 42);
    let recycled = RESULT_SLAB.with(|sl| unsafe { (&mut *sl.get()).alloc().0 as i64 });
    let stole_global_slot = recycled == result;
    RESULT_SLAB.with(|sl| unsafe { (&mut *sl.get()).free(recycled as *mut u8) });
    assert!(!stole_global_slot);
}
