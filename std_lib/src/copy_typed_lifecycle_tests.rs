use super::olive_obj_update_typed;
use crate::format::{D_DICT, D_INT, D_LIST, D_STR};
use crate::free_typed::olive_free_typed;
use crate::list::{list_from_vec, olive_list_get};
use crate::obj::{olive_obj_get, olive_obj_new, olive_obj_set};

#[test]
fn update_releases_displaced_list_and_preserves_source() {
    let descriptor = [D_DICT, D_INT, D_LIST, D_INT];
    let desc = descriptor.as_ptr() as i64;
    let target = olive_obj_new();
    let source = olive_obj_new();
    let old = list_from_vec(vec![7]);
    let replacement = list_from_vec(vec![9]);
    olive_obj_set(target, 2, old);
    olive_obj_set(source, 2, replacement);

    olive_obj_update_typed(target, source, desc);

    let old_released = !crate::slab::slot_is_live(old);
    let copied = olive_obj_get(target, 2);
    assert_ne!(copied, replacement);
    assert_eq!(olive_list_get(copied, 0), 9);
    assert_eq!(olive_list_get(replacement, 0), 9);
    olive_free_typed(target, desc);
    olive_free_typed(source, desc);
    if !old_released {
        olive_free_typed(old, descriptor[2..].as_ptr() as i64);
    }
    assert!(old_released, "update must release the displaced owned list");
}

#[test]
fn self_update_releases_displaced_strings() {
    let descriptor = [D_DICT, D_INT, D_STR];
    let desc = descriptor.as_ptr() as i64;
    let target = olive_obj_new();
    let old = crate::olive_str_internal("original");
    let generation = crate::string_slab::olive_str_gen_of(old);
    olive_obj_set(target, 2, old);

    olive_obj_update_typed(target, target, desc);

    let old_released = crate::string_slab::olive_str_gen_stale(old, generation) == 1;
    assert_eq!(
        crate::olive_str_from_ptr(olive_obj_get(target, 2)),
        "original"
    );
    olive_free_typed(target, desc);
    if !old_released {
        crate::olive_free_str(old);
    }
    assert!(old_released, "self-update must release replaced strings");
}
