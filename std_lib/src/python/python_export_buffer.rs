//! Generic (type-agnostic) zero-copy buffer-protocol export (R16). Any pod
//! that owns a native contiguous buffer hands olive_std a
//! (ptr, itemsize, format, ndim, shape, strides, deleter) tuple and gets
//! back a real Python object any buffer-protocol consumer (numpy,
//! memoryview, ...) can read/write with no copy. olive_std owns none of
//! the buffer's lifetime beyond running the caller's own deleter when the
//! last Python reference to the wrapper goes away -- it never allocates,
//! frees, or otherwise knows what the buffer is for.
//!
//! v1 always exports a writable view (no `PyExc_BufferError` binding yet
//! for rejecting `PyBUF_WRITABLE` against a read-only source).

use crate::python::python_bindings::*;
use crate::python::python_buffer::{PYBUF_FORMAT, PYBUF_ND, PYBUF_STRIDES, PyBuffer};
use crate::python::*;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Once;
use std::sync::atomic::Ordering;

#[repr(C)]
struct BufferViewObject {
    ob_refcnt: isize,
    ob_type: PyObject,
    ptr: *mut c_void,
    itemsize: isize,
    format: [u8; 4],
    ndim: i32,
    shape: *mut isize,
    strides: *mut isize,
    deleter: Option<unsafe extern "C" fn(*mut c_void)>,
    deleter_ctx: *mut c_void,
}

static mut BUFFER_VIEW_TYPE: PyObject = std::ptr::null_mut();
static INIT_TYPE: Once = Once::new();

unsafe extern "C" fn bufferview_dealloc(obj: PyObject) {
    unsafe {
        let self_ = &*(obj as *const BufferViewObject);
        if let Some(deleter) = self_.deleter {
            deleter(self_.deleter_ctx);
        }
        drop(Vec::from_raw_parts(
            self_.shape,
            self_.ndim as usize,
            self_.ndim as usize,
        ));
        drop(Vec::from_raw_parts(
            self_.strides,
            self_.ndim as usize,
            self_.ndim as usize,
        ));
        PY_OBJECT_FREE(obj as *mut c_void);
    }
}

unsafe extern "C" fn bufferview_getbuffer(obj: PyObject, view: *mut c_void, flags: c_int) -> c_int {
    unsafe {
        let self_ = &*(obj as *const BufferViewObject);
        let view = &mut *(view as *mut PyBuffer);
        view.obj = obj;
        PY_INC_REF(obj);
        let total: isize = (0..self_.ndim as usize)
            .map(|i| *self_.shape.add(i))
            .product();
        view.buf = self_.ptr;
        view.len = total * self_.itemsize;
        view.readonly = 0;
        view.itemsize = self_.itemsize;
        view.format = if flags & PYBUF_FORMAT != 0 {
            self_.format.as_ptr() as *mut c_char
        } else {
            std::ptr::null_mut()
        };
        view.ndim = self_.ndim;
        view.shape = if flags & PYBUF_ND != 0 {
            self_.shape
        } else {
            std::ptr::null_mut()
        };
        view.strides = if flags & PYBUF_STRIDES != 0 {
            self_.strides
        } else {
            std::ptr::null_mut()
        };
        view.suboffsets = std::ptr::null_mut();
        view.internal = std::ptr::null_mut();
        0
    }
}

unsafe extern "C" fn bufferview_releasebuffer(_obj: PyObject, _view: *mut c_void) {}

unsafe fn ensure_type() -> PyObject {
    unsafe {
        INIT_TYPE.call_once(|| {
            static NAME: &[u8] = b"olive.BufferView\0";
            static mut SLOTS: [PyTypeSlot; 4] = [
                PyTypeSlot {
                    slot: 0,
                    pfunc: std::ptr::null_mut(),
                },
                PyTypeSlot {
                    slot: 0,
                    pfunc: std::ptr::null_mut(),
                },
                PyTypeSlot {
                    slot: 0,
                    pfunc: std::ptr::null_mut(),
                },
                PyTypeSlot {
                    slot: 0,
                    pfunc: std::ptr::null_mut(),
                },
            ];
            #[allow(static_mut_refs)]
            {
                SLOTS[0] = PyTypeSlot {
                    slot: PY_TP_DEALLOC,
                    pfunc: bufferview_dealloc as *mut c_void,
                };
                SLOTS[1] = PyTypeSlot {
                    slot: PY_BF_GETBUFFER,
                    pfunc: bufferview_getbuffer as *mut c_void,
                };
                SLOTS[2] = PyTypeSlot {
                    slot: PY_BF_RELEASEBUFFER,
                    pfunc: bufferview_releasebuffer as *mut c_void,
                };
                SLOTS[3] = PyTypeSlot {
                    slot: 0,
                    pfunc: std::ptr::null_mut(),
                };
                let mut spec = PyTypeSpec {
                    name: NAME.as_ptr() as *const c_char,
                    basicsize: std::mem::size_of::<BufferViewObject>() as c_int,
                    itemsize: 0,
                    flags: PY_TPFLAGS_DEFAULT,
                    slots: SLOTS.as_mut_ptr(),
                };
                BUFFER_VIEW_TYPE = PY_TYPE_FROM_SPEC(&mut spec);
            }
        });
        BUFFER_VIEW_TYPE
    }
}

/// Describes a native buffer to export, zero-copy, as a real Python object.
/// `format` is a CPython `struct`-module format character (`d`=f64, `f`=f32,
/// `q`=i64, `i`=i32, `B`=u8, `?`=bool). `shape`/`strides` are element counts
/// and element-strides (converted to byte-strides internally to match
/// `Py_buffer`). `deleter`, if given, runs exactly once, when the last
/// Python reference to the returned wrapper is released; `deleter_ctx` is
/// passed through unexamined.
pub struct BufferSpec<'a> {
    pub ptr: *mut c_void,
    pub itemsize: isize,
    pub format: u8,
    pub shape: &'a [isize],
    pub strides_elems: &'a [isize],
    pub deleter: Option<unsafe extern "C" fn(*mut c_void)>,
    pub deleter_ctx: *mut c_void,
}

/// Wraps `spec` as a real Python object exporting the buffer protocol.
///
/// # Safety
/// `spec.ptr` must stay valid and unmoved for as long as any Python
/// reference to the returned object (or a view derived from it, e.g. a
/// `numpy` array) exists; nothing here can enforce that, it is on the
/// caller.
pub unsafe fn export_buffer_view(spec: BufferSpec) -> PyObject {
    unsafe {
        let ndim = spec.shape.len();
        if !HAS_TYPE_FROMSPEC.load(Ordering::SeqCst) || spec.strides_elems.len() != ndim {
            return std::ptr::null_mut();
        }
        let ty = ensure_type();
        if ty.is_null() {
            return std::ptr::null_mut();
        }
        let obj = PY_TYPE_GENERIC_ALLOC(ty, 0);
        if obj.is_null() {
            return std::ptr::null_mut();
        }
        let self_ = &mut *(obj as *mut BufferViewObject);
        self_.ptr = spec.ptr;
        self_.itemsize = spec.itemsize;
        self_.format = [spec.format, 0, 0, 0];
        self_.ndim = ndim as i32;
        self_.shape = spec.shape.to_vec().leak().as_mut_ptr();
        self_.strides = spec
            .strides_elems
            .iter()
            .map(|s| s * spec.itemsize)
            .collect::<Vec<_>>()
            .leak()
            .as_mut_ptr();
        self_.deleter = spec.deleter;
        self_.deleter_ctx = spec.deleter_ctx;
        obj
    }
}

/// Olive-callable entry point. `shape_list`/`strides_list` are the raw
/// handles of `[int]` Olive lists (element counts / element-strides);
/// `deleter_fn` is a raw code address (0 = none), the same convention
/// `aio.pool_run`'s `fn_ptr: int` already uses for passing a callback as
/// a plain integer across the FFI boundary.
#[unsafe(no_mangle)]
pub extern "C" fn olive_export_buffer_view(
    ptr: i64,
    itemsize: i64,
    format: i64,
    shape_list: i64,
    strides_list: i64,
    deleter_fn: i64,
    deleter_ctx: i64,
) -> PyObject {
    // Self-sufficient: the R13 GIL-fusion pass only wraps `__olive_py_*`-
    // named calls, so a caller reaching this entry point directly (e.g. a
    // pod, not through the pyffi call machinery) may hold no GIL at all.
    crate::python::with_gil(|| unsafe {
        if shape_list == 0 || strides_list == 0 {
            return std::ptr::null_mut();
        }
        let shape_v = &*(shape_list as *const crate::StableVec);
        let strides_v = &*(strides_list as *const crate::StableVec);
        if shape_v.len != strides_v.len {
            return std::ptr::null_mut();
        }
        let shape = std::slice::from_raw_parts(shape_v.ptr as *const isize, shape_v.len);
        let strides_elems =
            std::slice::from_raw_parts(strides_v.ptr as *const isize, strides_v.len);
        let deleter = if deleter_fn == 0 {
            None
        } else {
            Some(std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut c_void),
            >(deleter_fn as usize))
        };
        export_buffer_view(BufferSpec {
            ptr: ptr as *mut c_void,
            itemsize: itemsize as isize,
            format: format as u8,
            shape,
            strides_elems,
            deleter,
            deleter_ctx: deleter_ctx as *mut c_void,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::with_gil;

    static mut FREED: bool = false;
    unsafe extern "C" fn mark_freed(_ctx: *mut c_void) {
        unsafe {
            FREED = true;
        }
    }

    fn getattr(obj: PyObject, name: &str) -> PyObject {
        let cname = std::ffi::CString::new(name).unwrap();
        unsafe { PY_OBJECT_GET_ATTR_STRING(obj, cname.as_ptr()) }
    }

    #[test]
    fn export_roundtrips_through_numpy_and_runs_deleter() {
        if !is_python_available() {
            return;
        }
        with_gil(|| unsafe {
            if !HAS_TYPE_FROMSPEC.load(Ordering::SeqCst) {
                return;
            }
            FREED = false;
            let mut data: Vec<f64> = vec![1.0, 2.0, 3.0, 4.0];
            let ptr = data.as_mut_ptr() as *mut c_void;
            let obj = export_buffer_view(BufferSpec {
                ptr,
                itemsize: 8,
                format: b'd',
                shape: &[4],
                strides_elems: &[1],
                deleter: Some(mark_freed),
                deleter_ctx: std::ptr::null_mut(),
            });
            assert!(!obj.is_null());

            let np_mod = PY_IMPORT_IMPORT_MODULE(b"numpy\0".as_ptr() as *const c_char);
            assert!(!np_mod.is_null());
            let asarray = getattr(np_mod, "asarray");
            let args_tuple = PY_TUPLE_NEW(1);
            PY_INC_REF(obj);
            PY_TUPLE_SET_ITEM(args_tuple, 0, obj); // steals the incref above
            let arr = PY_OBJECT_CALL_OBJECT(asarray, args_tuple);
            if arr.is_null() {
                PY_ERR_PRINT();
            }
            assert!(!arr.is_null());
            PY_DEC_REF(args_tuple);

            // Pointer identity: numpy's own ctypes.data must equal our `ptr`,
            // not a copy -- the actual zero-copy claim, not just "no crash".
            let ctypes_attr = getattr(arr, "ctypes");
            let data_attr = getattr(ctypes_attr, "data");
            let np_ptr = PY_LONG_AS_LONG(data_attr) as usize;
            assert_eq!(
                np_ptr, ptr as usize,
                "numpy must see the same backing pointer"
            );
            PY_DEC_REF(data_attr);
            PY_DEC_REF(ctypes_attr);

            // numpy sees the values Rust wrote before export.
            let idx_read = PY_LONG_FROM_LONG(0);
            let item0 = PY_OBJECT_GET_ITEM(arr, idx_read);
            assert_eq!(PY_FLOAT_AS_DOUBLE(item0), 1.0);
            PY_DEC_REF(idx_read);
            PY_DEC_REF(item0);

            // Bidirectional: a numpy-side write (via the real C-API
            // setitem, same op `arr[0] = 99.0` compiles to) is visible
            // through the original Rust slice with no copy either way.
            let idx0 = PY_LONG_FROM_LONG(0);
            let new_val = PY_FLOAT_FROM_DOUBLE(99.0);
            let rc = PY_OBJECT_SET_ITEM(arr, idx0, new_val);
            assert_eq!(rc, 0);
            PY_DEC_REF(idx0);
            PY_DEC_REF(new_val);
            assert_eq!(data[0], 99.0, "Rust must see numpy's write with no copy");

            PY_DEC_REF(asarray);
            PY_DEC_REF(np_mod);
            PY_DEC_REF(arr);
            PY_DEC_REF(obj);
            assert!(FREED, "deleter must run once the wrapper is collected");
        });
    }
}
