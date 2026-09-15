use super::*;
use crate::slab::{ACTIVE_SLABS, SlabSet, slot_is_live};

#[test]
fn empty_buffers_allow_vec_access() {
    let mut bytes = OliveBytes::empty();
    assert!(unsafe { bytes.take_vec() }.is_empty());
    bytes.append(&[]);
    bytes.with_vec(|v| v.extend_from_slice(b"first"));
    assert_eq!(bytes.as_slice(), b"first");
    assert_eq!(unsafe { bytes.take_vec() }, b"first");
    bytes.append(b"second");
    assert_eq!(bytes.as_slice(), b"second");
    drop(unsafe { bytes.take_vec() });
}

#[test]
fn replacing_native_storage_releases_previous_buffers() {
    let mut bytes = OliveBytes::empty();
    for value in 0..32 {
        bytes.set_vec(vec![value; 128]);
        assert_eq!(bytes.as_slice(), &[value; 128]);
    }
    drop(unsafe { bytes.take_vec() });
}

#[test]
fn stack_bytes_release_storage_on_drop() {
    for _ in 0..32 {
        let mut bytes = OliveBytes::empty();
        bytes.set_vec(vec![42; 128]);
        assert_eq!(bytes.as_slice(), &[42; 128]);
    }
}

#[test]
fn task_teardown_releases_live_bytes() {
    let mut slabs = SlabSet::new();
    let previous = ACTIVE_SLABS.replace(&mut slabs);
    let bytes = new_buf(vec![42; 128]);
    ACTIVE_SLABS.set(previous);
    drop(slabs);
    let still_live = slot_is_live(bytes);
    if still_live {
        olive_buf_free(bytes);
    }
    assert!(!still_live);
}

#[test]
fn task_bytes_survive_thread_migration() {
    let (mut slabs, bytes) = std::thread::spawn(|| {
        let mut slabs = Box::new(SlabSet::new());
        let previous = ACTIVE_SLABS.replace(&mut *slabs);
        let bytes = new_buf(b"task payload".to_vec());
        ACTIVE_SLABS.set(previous);
        (slabs, bytes)
    })
    .join()
    .unwrap();
    assert!(slot_is_live(bytes));
    let previous = ACTIVE_SLABS.replace(&mut *slabs);
    let contents = unsafe { (&*(bytes as *const OliveBytes)).as_slice().to_vec() };
    olive_buf_free(bytes);
    let freed = !slot_is_live(bytes);
    ACTIVE_SLABS.set(previous);
    assert!(freed);
    assert_eq!(contents, b"task payload");
}

#[test]
fn relocated_bytes_survive_sender_teardown_and_receiver_free() {
    let bytes = std::thread::spawn(|| {
        let source = new_buf(b"relocated payload".to_vec());
        let relocated = crate::copy_typed::olive_relocate_typed(
            source,
            [crate::format::D_BYTES].as_ptr() as i64,
        );
        olive_buf_free(source);
        relocated
    })
    .join()
    .unwrap();
    assert!(slot_is_live(bytes));
    assert!(crate::slab::chunk_is_global(bytes as usize));
    let contents = std::thread::spawn(move || {
        let contents = unsafe { (&*(bytes as *const OliveBytes)).as_slice().to_vec() };
        olive_buf_free(bytes);
        assert!(!slot_is_live(bytes));
        contents
    })
    .join()
    .unwrap();
    assert_eq!(contents, b"relocated payload");
}

#[test]
fn thread_teardown_releases_live_native_storage() {
    std::thread::spawn(|| {
        for _ in 0..32 {
            new_buf(vec![42; 128]);
        }
    })
    .join()
    .unwrap();
}
