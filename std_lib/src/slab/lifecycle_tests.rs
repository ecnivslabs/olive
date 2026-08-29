use super::*;
use crate::format::{D_DICT, D_ENUM, D_INT, D_SET};

fn allocation_wave(free: bool, typed: bool) {
    for _ in 0..32 {
        let dict = crate::olive_obj_new();
        crate::olive_obj_set(dict, 0, 42);
        let set = crate::set::olive_set_new(4);
        crate::set::olive_set_add(set, 42);
        let variant = crate::olive_enum_new(1, 0, 1, 0);
        crate::olive_enum_set(variant, 0, 42);
        if free && typed {
            crate::free_typed::olive_free_typed(dict, [D_DICT, D_INT, D_INT].as_ptr() as i64);
            crate::free_typed::olive_free_typed(set, [D_SET, D_INT].as_ptr() as i64);
            crate::free_typed::olive_free_typed(
                variant,
                [D_ENUM, 14, b'E', 14, 14, b'V', 14, D_INT].as_ptr() as i64,
            );
        } else if free {
            crate::olive_free_obj(dict);
            crate::set::olive_free_set(set);
            crate::olive_free_enum(variant);
        }
    }
}

#[test]
fn task_teardown_releases_container_storage() {
    for free in [false, true] {
        for typed in [false, true] {
            let mut slabs = SlabSet::new();
            let previous = ACTIVE_SLABS.replace(&mut slabs);
            allocation_wave(free, typed);
            ACTIVE_SLABS.set(previous);
            drop(slabs);
        }
    }
}

#[test]
fn thread_teardown_releases_container_storage() {
    for free in [false, true] {
        for typed in [false, true] {
            std::thread::spawn(move || allocation_wave(free, typed))
                .join()
                .unwrap();
        }
    }
}

#[test]
fn enum_reuse_handles_growing_empty_and_shrinking_payloads() {
    let variant = crate::olive_enum_new(1, 0, 8, 0);
    for n in [3, 0, 9, 9, 1] {
        let old_len = unsafe { (*(variant as *const crate::OliveEnum)).payload_len };
        for i in 0..old_len {
            crate::olive_enum_set(variant, i as i64, 99);
        }
        let reused = crate::enum_obj::olive_enum_new_reuse(variant, 2, 1, n, 1, 0);
        assert_eq!(reused, variant);
        assert_eq!(crate::olive_enum_type_id(variant), 2);
        assert_eq!(crate::olive_enum_tag(variant), 1);
        assert_eq!(
            unsafe { (*(variant as *const crate::OliveEnum)).payload_len },
            n as usize
        );
        for i in 0..n {
            assert_eq!(crate::olive_enum_get(variant, i), 0);
        }
    }
    crate::olive_free_enum(variant);
    assert!(!slot_is_live(variant));
}

#[test]
fn escaped_struct_size_classes_keep_global_ownership() {
    let structs =
        with_escape_arena(|| [0, 1, 4, 16, 17, 63, 512].map(|n| crate::olive_struct_alloc(n)));
    for &value in &structs {
        assert!(chunk_is_global(value as usize));
    }
    std::thread::spawn(move || {
        for value in structs {
            crate::olive_free_struct(value);
            assert!(!slot_is_live(value));
        }
    })
    .join()
    .unwrap();
}

#[test]
fn retired_struct_borrow_is_stale() {
    let (ptr, generation) = std::thread::spawn(|| {
        let ptr = crate::olive_struct_alloc(2);
        (ptr, crate::struct_obj::olive_struct_gen_of(ptr))
    })
    .join()
    .unwrap();
    assert_ne!(generation, 0);
    let current = crate::struct_obj::olive_struct_gen_of(ptr);
    assert_ne!(
        current, generation,
        "retired arena must tear down or recycle with a new epoch, not keep the same live generation"
    );
    assert_eq!(
        crate::struct_obj::olive_struct_gen_stale(ptr, generation),
        1
    );
}

#[test]
fn retired_string_borrow_is_stale() {
    let (ptr, generation) = std::thread::spawn(|| {
        let ptr = crate::string_slab::str_alloc(b"retired string");
        (ptr, crate::string_slab::olive_str_gen_of(ptr))
    })
    .join()
    .unwrap();
    assert_ne!(generation, 0);
    let current = crate::string_slab::olive_str_gen_of(ptr);
    assert_ne!(
        current, generation,
        "retired arena must tear down or recycle with a new epoch, not keep the same live generation"
    );
    assert_eq!(crate::string_slab::olive_str_gen_stale(ptr, generation), 1);
}
