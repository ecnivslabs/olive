use super::*;
use crate::format::D_INT;
use crate::list::list_from_vec;

const SHARED: &[u8] = &[D_STRUCT_SHARED, 14, b'R', 14, 14, b's', D_STR];

fn shared_value() -> (i64, i64) {
    let text = crate::olive_str_internal("resource field");
    let value = crate::olive_struct_alloc(1);
    unsafe { *((value as *mut i64).add(1)) = text };
    (value, text)
}

#[test]
fn list_releases_shared_struct_elements() {
    let (value, text) = shared_value();
    crate::struct_share::retain_struct(value);
    let list = list_from_vec(vec![value]);
    let descriptor = [vec![D_LIST], SHARED.to_vec()].concat();
    olive_free_typed(list, descriptor.as_ptr() as i64);
    assert!(slot_is_live(value));
    olive_free_typed(value, SHARED.as_ptr() as i64);
    assert!(!slot_is_live(value));
    assert!(!slot_is_live(crate::string_slab::str_body(text)));
}

#[test]
fn dict_releases_shared_struct_values() {
    let (value, text) = shared_value();
    let dict = crate::olive_obj_new();
    crate::olive_obj_set(dict, 0, value);
    let descriptor = [vec![D_DICT, D_INT], SHARED.to_vec()].concat();
    olive_free_typed(dict, descriptor.as_ptr() as i64);
    assert!(!slot_is_live(value));
    assert!(!slot_is_live(crate::string_slab::str_body(text)));
}

#[test]
fn list_releases_recursive_struct_elements() {
    let parent = crate::olive_struct_alloc(1);
    let child = crate::olive_struct_alloc(1);
    let children = list_from_vec(vec![child]);
    unsafe { *((parent as *mut i64).add(1)) = children };
    let descriptor = [D_STRUCT, 14, b'N', 14, 14, b'c', D_LIST, D_BACKREF, 0, 0];
    olive_free_typed(parent, descriptor.as_ptr() as i64);
    assert!(!slot_is_live(parent));
    assert!(!slot_is_live(children));
    assert!(!slot_is_live(child));
}

extern "C" fn drop_list(value: i64) -> i64 {
    olive_free_typed(value, [D_LIST, D_INT].as_ptr() as i64);
    0
}

extern "C" fn drop_list_with_allocation(value: i64) -> i64 {
    let temporary = crate::olive_list_new(2);
    drop_list(temporary);
    drop_list(value)
}

#[test]
fn element_destructor_cannot_reuse_buffer_being_traversed() {
    check_destructor_allocation(&[D_LIST, D_FATPTR]);
}

#[test]
fn tuple_destructor_cannot_reuse_buffer_being_traversed() {
    check_destructor_allocation(&[D_TUPLE, 3, D_FATPTR, D_FATPTR]);
}

fn check_destructor_allocation(descriptor: &[u8]) {
    let first = list_from_vec(vec![1]);
    let second = list_from_vec(vec![2]);
    let shim = drop_list_with_allocation as *const () as i64;
    let a = crate::struct_obj::fatptr_new(first, 0, shim, 0);
    let b = crate::struct_obj::fatptr_new(second, 0, shim, 0);
    let list = list_from_vec(vec![a, b]);
    olive_free_typed(list, descriptor.as_ptr() as i64);
    assert!(!slot_is_live(a));
    assert!(!slot_is_live(b));
    assert!(!slot_is_live(first));
    assert!(!slot_is_live(second));
}

#[test]
fn list_releases_trait_object_elements() {
    let data = list_from_vec(vec![42]);
    let value = crate::struct_obj::fatptr_new(data, 0, drop_list as *const () as i64, 0);
    let list = list_from_vec(vec![value]);
    olive_free_typed(list, [D_LIST, D_FATPTR].as_ptr() as i64);
    assert!(!slot_is_live(list));
    assert!(!slot_is_live(value));
    assert!(!slot_is_live(data));
}

#[test]
fn tuple_clear_uses_each_field_descriptor() {
    let text = crate::olive_str_internal("tuple field");
    let child = list_from_vec(vec![42]);
    let tuple = list_from_vec(vec![7, text, child]);
    let descriptor = [D_TUPLE, 4, D_INT, D_STR, D_LIST, D_INT];
    olive_clear_typed(tuple, descriptor.as_ptr() as i64);
    assert!(slot_is_live(tuple));
    assert!(!slot_is_live(crate::string_slab::str_body(text)));
    assert!(!slot_is_live(child));
    olive_free_typed(tuple, descriptor.as_ptr() as i64);
}

#[test]
fn repeated_task_teardown_releases_list_buffers() {
    use crate::slab::{ACTIVE_SLABS, SlabSet};

    for _ in 0..32 {
        let mut slabs = SlabSet::new();
        let previous = ACTIVE_SLABS.replace(&mut slabs);
        let list = list_from_vec(vec![1, 2, 3]);
        olive_free_typed(list, [D_LIST, D_INT].as_ptr() as i64);
        let live = list_from_vec(vec![4, 5]);
        assert!(slot_is_live(live));
        ACTIVE_SLABS.set(previous);
        drop(slabs);
        assert!(!slot_is_live(live));
    }
}
