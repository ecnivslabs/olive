use super::super::*;

#[test]
fn backref_skip_consumes_exactly_three_bytes() {
    let descriptor = [D_BACKREF, 0, 2];
    let mut pos = 0;
    skip(descriptor.as_ptr(), &mut pos);
    assert_eq!(pos, 3);
}

#[test]
fn recursive_backref_formats_target_at_nonzero_offset() {
    let d = vec![
        D_TUPLE, 3, D_STRUCT, 14, b'N', 15, 14, b'v', D_STR, 14, b'n', D_BACKREF, 0, 2, D_INT, 0,
    ];
    let child = crate::olive_struct_alloc(2);
    let root = crate::olive_struct_alloc(2);
    unsafe {
        *((root + 8) as *mut i64) = crate::olive_str_internal("root");
        *((root + 16) as *mut i64) = child;
        *((child + 8) as *mut i64) = crate::olive_str_internal("leaf");
        *((child + 16) as *mut i64) = 0;
    }
    let tuple = crate::list::list_from_vec(vec![root, 7]);
    assert_eq!(
        format_desc(tuple, d.as_ptr() as i64),
        "(N(v=\"root\", n=N(v=\"leaf\", n=None)), 7)"
    );
    crate::free_typed::olive_free_typed(tuple, d.as_ptr() as i64);
}

#[test]
fn owned_subdescriptor_rebases_nonzero_backrefs() {
    let d = vec![
        D_TUPLE, 3, D_STRUCT, 14, b'N', 15, 14, b'v', D_STR, 14, b'n', D_BACKREF, 0, 2, D_INT, 0,
    ];
    let owned = owned_subdescriptor(d.as_ptr(), 2);
    assert_eq!(owned.as_ptr() as usize % 8, 0);
    assert_eq!(owned.as_slice()[9], D_BACKREF);
    assert_eq!(owned.as_slice()[10], 0);
    assert_eq!(owned.as_slice()[11], 0);
    let mut end = 0;
    skip(owned.as_ptr(), &mut end);
    assert_eq!(end, owned.as_slice().len() - 1);
}

#[test]
fn owned_subdescriptor_includes_external_backref_target() {
    let d = vec![
        D_TUPLE, 4, D_STRUCT, 14, b'O', 14, 14, b'x', D_INT, D_STRUCT, 14, b'C', 14, 14, b'o',
        D_BACKREF, 0, 2, D_INT, 0,
    ];
    let owned = owned_subdescriptor(d.as_ptr(), 9);
    let bytes = owned.as_slice();
    let backref = 6;
    let target = ((bytes[backref + 1] as usize) << 8) | bytes[backref + 2] as usize;
    assert!(target > 0);
    assert_eq!(bytes[target], D_STRUCT);
    let persistent = crate::index_any::intern_sub_descriptor(owned.as_ptr(), 0);
    assert_eq!(persistent as usize % 8, 0);
}

#[test]
fn recursive_equality_follows_original_root_backref() {
    let d = vec![
        D_TUPLE, 3, D_STRUCT, 14, b'N', 15, 14, b'v', D_STR, 14, b'n', D_BACKREF, 0, 2, D_INT, 0,
    ];
    let make = || {
        let child = crate::olive_struct_alloc(2);
        let root = crate::olive_struct_alloc(2);
        unsafe {
            *((root + 8) as *mut i64) = crate::olive_str_internal("same");
            *((root + 16) as *mut i64) = child;
            *((child + 8) as *mut i64) = crate::olive_str_internal("leaf");
            *((child + 16) as *mut i64) = 0;
        }
        crate::list::list_from_vec(vec![root, 7])
    };
    let left = make();
    let right = make();
    assert_eq!(
        crate::eq_typed::olive_eq_typed(left, right, d.as_ptr() as i64),
        1
    );
    crate::free_typed::olive_free_typed(left, d.as_ptr() as i64);
    crate::free_typed::olive_free_typed(right, d.as_ptr() as i64);
}
