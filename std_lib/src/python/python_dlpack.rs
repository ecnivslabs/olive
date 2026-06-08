//! Generic (type-agnostic) DLPack export + import (R16). Same philosophy
//! as `python_export_buffer`: olive_std owns the wire protocol (struct
//! layout, capsule naming/rename handoff, deleter plumbing), never a
//! buffer's allocation or a "Tensor" abstraction. A future pod hands over
//! (ptr, dtype, shape, strides, deleter) and gets a real DLPack-consumable
//! Python object back; or hands over a foreign `__dlpack__`-capable object
//! and gets back the parsed fields plus a release handle.
//!
//! CPU-only (`kDLCPU`) in v1, matching R16's original scope statement --
//! a foreign GPU tensor's device fields are read but never dereferenced.

use crate::python::python_bindings::*;
use crate::python::*;
use std::os::raw::{c_char, c_void};
use std::sync::atomic::Ordering;

pub const DL_CPU: i32 = 1;

pub const DL_INT: u8 = 0;
pub const DL_UINT: u8 = 1;
pub const DL_FLOAT: u8 = 2;
pub const DL_BOOL: u8 = 6;

const DLTENSOR_NAME: &[u8] = b"dltensor\0";
const USED_DLTENSOR_NAME: &[u8] = b"used_dltensor\0";

#[repr(C)]
struct DLDevice {
    device_type: i32,
    device_id: i32,
}

#[repr(C)]
struct DLDataType {
    code: u8,
    bits: u8,
    lanes: u16,
}

#[repr(C)]
struct DLTensor {
    data: *mut c_void,
    device: DLDevice,
    ndim: i32,
    dtype: DLDataType,
    shape: *mut i64,
    strides: *mut i64,
    byte_offset: u64,
}

#[repr(C)]
struct DLManagedTensor {
    dl_tensor: DLTensor,
    manager_ctx: *mut c_void,
    deleter: Option<unsafe extern "C" fn(*mut DLManagedTensor)>,
}

/// Bundles the caller-supplied deleter with the `DLManagedTensor` so
/// `managed_deleter` can run it without olive_std knowing what it does.
struct ManagerCtx {
    deleter: Option<unsafe extern "C" fn(*mut c_void)>,
    deleter_ctx: *mut c_void,
}

unsafe extern "C" fn managed_deleter(dlmt: *mut DLManagedTensor) {
    unsafe {
        let mgr = Box::from_raw((*dlmt).manager_ctx as *mut ManagerCtx);
        if let Some(del) = mgr.deleter {
            del(mgr.deleter_ctx);
        }
        let ndim = (*dlmt).dl_tensor.ndim as usize;
        drop(Vec::from_raw_parts((*dlmt).dl_tensor.shape, ndim, ndim));
        if !(*dlmt).dl_tensor.strides.is_null() {
            drop(Vec::from_raw_parts((*dlmt).dl_tensor.strides, ndim, ndim));
        }
        drop(Box::from_raw(dlmt));
    }
}

/// Producer/consumer handoff: a consumer that imports the tensor renames
/// the capsule `"dltensor"` -> `"used_dltensor"` (see `dlpack_import`
/// below) so this destructor knows not to double-run the deleter.
unsafe extern "C" fn capsule_deleter(capsule: PyObject) {
    unsafe {
        if PY_CAPSULE_IS_VALID(capsule, USED_DLTENSOR_NAME.as_ptr() as *const c_char) != 0 {
            return;
        }
        let dlmt = PY_CAPSULE_GET_POINTER(capsule, DLTENSOR_NAME.as_ptr() as *const c_char)
            as *mut DLManagedTensor;
        if !dlmt.is_null()
            && let Some(del) = (*dlmt).deleter
        {
            del(dlmt);
        }
    }
}

/// Describes a native buffer to export via DLPack. `shape`/`strides_elems`
/// are both in elements (DLPack strides are element-based, unlike
/// `Py_buffer`'s byte-based strides). `device_type`/`device_id` default to
/// `DL_CPU`/`0` for anything allocated on the host.
pub struct DlpackSpec<'a> {
    pub ptr: *mut c_void,
    pub dtype_code: u8,
    pub bits: u8,
    pub shape: &'a [i64],
    pub strides_elems: &'a [i64],
    pub device_type: i32,
    pub device_id: i32,
    pub deleter: Option<unsafe extern "C" fn(*mut c_void)>,
    pub deleter_ctx: *mut c_void,
}

/// Wraps `spec` as a `"dltensor"`-named `PyCapsule`, real zero-copy: no
/// bytes of the underlying buffer are touched, only descriptor metadata is
/// allocated. Returns null if `PyCapsule`/`PyType_FromSpec` support isn't
/// available in the loaded libpython.
///
/// # Safety
/// `spec.ptr` must stay valid and unmoved for as long as the returned
/// capsule (or a tensor derived from it, e.g. `torch.from_dlpack`) exists.
pub unsafe fn dlpack_export(spec: DlpackSpec) -> PyObject {
    unsafe {
        if !HAS_CAPSULE.load(Ordering::SeqCst) {
            return std::ptr::null_mut();
        }
        let ndim = spec.shape.len();
        let shape_buf = spec.shape.to_vec().leak().as_mut_ptr();
        let strides_buf = spec.strides_elems.to_vec().leak().as_mut_ptr();
        let mgr = Box::into_raw(Box::new(ManagerCtx {
            deleter: spec.deleter,
            deleter_ctx: spec.deleter_ctx,
        }));
        let dlmt = Box::into_raw(Box::new(DLManagedTensor {
            dl_tensor: DLTensor {
                data: spec.ptr,
                device: DLDevice {
                    device_type: spec.device_type,
                    device_id: spec.device_id,
                },
                ndim: ndim as i32,
                dtype: DLDataType {
                    code: spec.dtype_code,
                    bits: spec.bits,
                    lanes: 1,
                },
                shape: shape_buf,
                strides: strides_buf,
                byte_offset: 0,
            },
            manager_ctx: mgr as *mut c_void,
            deleter: Some(managed_deleter),
        }));
        PY_CAPSULE_NEW(
            dlmt as *mut c_void,
            DLTENSOR_NAME.as_ptr() as *const c_char,
            Some(capsule_deleter),
        )
    }
}

/// A foreign tensor imported via `__dlpack__`, zero-copy: `data_ptr` points
/// directly at the exporter's own buffer. The caller must eventually call
/// `release` exactly once (never touch `data_ptr` after that) so the
/// exporter's own deleter runs; dropping this value without calling
/// `release` leaks the foreign tensor forever, matching the DLPack
/// contract (an unconsumed capsule frees itself when GC'd instead, via
/// `capsule_deleter` above -- but a *consumed* one, like this, must be
/// released explicitly since Python no longer owns it once the rename
/// happens below).
pub struct ImportedDlpack {
    dlmt: *mut DLManagedTensor,
}

impl ImportedDlpack {
    pub fn data_ptr(&self) -> *mut c_void {
        unsafe { (*self.dlmt).dl_tensor.data }
    }
    pub fn ndim(&self) -> i32 {
        unsafe { (*self.dlmt).dl_tensor.ndim }
    }
    pub fn shape(&self) -> &[i64] {
        unsafe { std::slice::from_raw_parts((*self.dlmt).dl_tensor.shape, self.ndim() as usize) }
    }
    pub fn strides(&self) -> Option<&[i64]> {
        unsafe {
            let s = (*self.dlmt).dl_tensor.strides;
            if s.is_null() {
                None
            } else {
                Some(std::slice::from_raw_parts(s, self.ndim() as usize))
            }
        }
    }
    pub fn dtype_code(&self) -> u8 {
        unsafe { (*self.dlmt).dl_tensor.dtype.code }
    }
    pub fn bits(&self) -> u8 {
        unsafe { (*self.dlmt).dl_tensor.dtype.bits }
    }
    pub fn device_type(&self) -> i32 {
        unsafe { (*self.dlmt).dl_tensor.device.device_type }
    }
    /// Runs the exporter's deleter. Must be called exactly once.
    pub fn release(self) {
        unsafe {
            if let Some(del) = (*self.dlmt).deleter {
                del(self.dlmt);
            }
        }
    }
}

/// Olive-callable entry point. `shape_list`/`strides_list` are the raw
/// handles of `[int]` Olive lists; `deleter_fn` is a raw code address
/// (0 = none), matching `aio.pool_run`'s `fn_ptr: int` convention.
/// CPU-only in v1 (`device_type` is always `DL_CPU`, `device_id` always
/// `0`), matching R16's stated scope.
#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_export(
    ptr: i64,
    dtype_code: i64,
    bits: i64,
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
        let shape = std::slice::from_raw_parts(shape_v.ptr, shape_v.len);
        let strides_elems = std::slice::from_raw_parts(strides_v.ptr, strides_v.len);
        let deleter = if deleter_fn == 0 {
            None
        } else {
            Some(std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut c_void),
            >(deleter_fn as usize))
        };
        dlpack_export(DlpackSpec {
            ptr: ptr as *mut c_void,
            dtype_code: dtype_code as u8,
            bits: bits as u8,
            shape,
            strides_elems,
            device_type: DL_CPU,
            device_id: 0,
            deleter,
            deleter_ctx: deleter_ctx as *mut c_void,
        })
    })
}

/// Olive-callable entry point for import: returns an opaque handle (the
/// underlying `DLManagedTensor` pointer, 0 if the object isn't
/// DLPack-capable) to be read via `olive_dlpack_*` accessors below and
/// finally consumed exactly once via `olive_dlpack_release`.
#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_import(obj: PyObject) -> i64 {
    crate::python::with_gil(|| unsafe { dlpack_import(obj).map_or(0, |d| d.dlmt as i64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_data_ptr(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe { (*(handle as *const DLManagedTensor)).dl_tensor.data as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_ndim(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe { (*(handle as *const DLManagedTensor)).dl_tensor.ndim as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_shape_at(handle: i64, dim: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe {
        let t = &(*(handle as *const DLManagedTensor)).dl_tensor;
        if dim < 0 || dim >= t.ndim as i64 {
            return 0;
        }
        *t.shape.add(dim as usize)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_dtype_code(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe { (*(handle as *const DLManagedTensor)).dl_tensor.dtype.code as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_bits(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe { (*(handle as *const DLManagedTensor)).dl_tensor.dtype.bits as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_device_type(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe {
        (*(handle as *const DLManagedTensor))
            .dl_tensor
            .device
            .device_type as i64
    }
}

/// Runs the exporter's deleter, consuming `handle`. Must be called exactly
/// once, and never touched again afterward.
#[unsafe(no_mangle)]
pub extern "C" fn olive_dlpack_release(handle: i64) {
    if handle == 0 {
        return;
    }
    crate::python::with_gil(|| unsafe {
        let dlmt = handle as *mut DLManagedTensor;
        if let Some(del) = (*dlmt).deleter {
            del(dlmt);
        }
    });
}

/// Imports a foreign DLPack-capable Python object (anything exposing
/// `__dlpack__`), zero-copy. Renames the capsule per the standard
/// producer/consumer handoff (`"dltensor"` -> `"used_dltensor"`) so the
/// exporter's own capsule destructor won't also run the deleter -- from
/// this point, the returned `ImportedDlpack` owns that responsibility via
/// `release`. Returns `None` if the object has no `__dlpack__` method or
/// capsule support isn't available.
pub unsafe fn dlpack_import(obj: PyObject) -> Option<ImportedDlpack> {
    unsafe {
        if !HAS_CAPSULE.load(Ordering::SeqCst) || obj.is_null() {
            return None;
        }
        let method_name = std::ffi::CString::new("__dlpack__").ok()?;
        let method = PY_OBJECT_GET_ATTR_STRING(obj, method_name.as_ptr());
        if method.is_null() {
            return None;
        }
        let empty_args = PY_TUPLE_NEW(0);
        let capsule = PY_OBJECT_CALL_OBJECT(method, empty_args);
        PY_DEC_REF(empty_args);
        PY_DEC_REF(method);
        if capsule.is_null() {
            return None;
        }
        if PY_CAPSULE_IS_VALID(capsule, DLTENSOR_NAME.as_ptr() as *const c_char) == 0 {
            PY_DEC_REF(capsule);
            return None;
        }
        let dlmt = PY_CAPSULE_GET_POINTER(capsule, DLTENSOR_NAME.as_ptr() as *const c_char)
            as *mut DLManagedTensor;
        if dlmt.is_null() {
            PY_DEC_REF(capsule);
            return None;
        }
        PY_CAPSULE_SET_NAME(capsule, USED_DLTENSOR_NAME.as_ptr() as *const c_char);
        PY_DEC_REF(capsule);
        Some(ImportedDlpack { dlmt })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python::with_gil;

    static mut EXPORT_FREED: bool = false;
    unsafe extern "C" fn mark_export_freed(_ctx: *mut c_void) {
        unsafe {
            EXPORT_FREED = true;
        }
    }

    #[test]
    fn export_roundtrips_through_torch_and_runs_deleter() {
        if !is_python_available() {
            return;
        }
        with_gil(|| unsafe {
            if !HAS_CAPSULE.load(Ordering::SeqCst) {
                return;
            }
            let torch_mod = PY_IMPORT_IMPORT_MODULE(b"torch\0".as_ptr() as *const c_char);
            if torch_mod.is_null() {
                PY_ERR_CLEAR();
                return; // torch not installed in this environment; skip
            }

            EXPORT_FREED = false;
            let mut data: Vec<f64> = vec![1.5, 2.5, 3.5, 4.5];
            let ptr = data.as_mut_ptr() as *mut c_void;
            let capsule = dlpack_export(DlpackSpec {
                ptr,
                dtype_code: DL_FLOAT,
                bits: 64,
                shape: &[4],
                strides_elems: &[1],
                device_type: DL_CPU,
                device_id: 0,
                deleter: Some(mark_export_freed),
                deleter_ctx: std::ptr::null_mut(),
            });
            assert!(!capsule.is_null());

            let from_dlpack_name = std::ffi::CString::new("from_dlpack").unwrap();
            let from_dlpack = PY_OBJECT_GET_ATTR_STRING(torch_mod, from_dlpack_name.as_ptr());
            assert!(!from_dlpack.is_null());
            let args_tuple = PY_TUPLE_NEW(1);
            PY_INC_REF(capsule);
            PY_TUPLE_SET_ITEM(args_tuple, 0, capsule); // steals the incref above
            let tensor = PY_OBJECT_CALL_OBJECT(from_dlpack, args_tuple);
            if tensor.is_null() {
                PY_ERR_PRINT();
            }
            assert!(
                !tensor.is_null(),
                "torch.from_dlpack must accept the capsule"
            );
            PY_DEC_REF(args_tuple);

            let data_ptr_name = std::ffi::CString::new("data_ptr").unwrap();
            let data_ptr_method = PY_OBJECT_GET_ATTR_STRING(tensor, data_ptr_name.as_ptr());
            let empty_args = PY_TUPLE_NEW(0);
            let ptr_result = PY_OBJECT_CALL_OBJECT(data_ptr_method, empty_args);
            let torch_ptr = PY_LONG_AS_LONG(ptr_result) as usize;
            assert_eq!(
                torch_ptr, ptr as usize,
                "torch must see the same backing pointer"
            );

            PY_DEC_REF(ptr_result);
            PY_DEC_REF(empty_args);
            PY_DEC_REF(data_ptr_method);
            PY_DEC_REF(from_dlpack);
            PY_DEC_REF(tensor);
            PY_DEC_REF(capsule);
            PY_DEC_REF(torch_mod);
            assert!(
                EXPORT_FREED,
                "deleter must run once torch's tensor is collected"
            );
        });
    }

    #[test]
    fn import_reads_a_real_torch_tensor_zero_copy_then_releases() {
        if !is_python_available() {
            return;
        }
        with_gil(|| unsafe {
            if !HAS_CAPSULE.load(Ordering::SeqCst) {
                return;
            }
            let torch_mod = PY_IMPORT_IMPORT_MODULE(b"torch\0".as_ptr() as *const c_char);
            if torch_mod.is_null() {
                PY_ERR_CLEAR();
                return;
            }
            let ones_name = std::ffi::CString::new("ones").unwrap();
            let ones = PY_OBJECT_GET_ATTR_STRING(torch_mod, ones_name.as_ptr());
            let shape_arg = PY_LONG_FROM_LONG(4);
            let args = PY_TUPLE_NEW(1);
            PY_TUPLE_SET_ITEM(args, 0, shape_arg);
            let tensor = PY_OBJECT_CALL_OBJECT(ones, args);
            assert!(!tensor.is_null());

            let data_ptr_name = std::ffi::CString::new("data_ptr").unwrap();
            let data_ptr_method = PY_OBJECT_GET_ATTR_STRING(tensor, data_ptr_name.as_ptr());
            let empty_args = PY_TUPLE_NEW(0);
            let ptr_result = PY_OBJECT_CALL_OBJECT(data_ptr_method, empty_args);
            let torch_ptr = PY_LONG_AS_LONG(ptr_result) as usize;

            let imported = dlpack_import(tensor).expect("torch tensor must expose __dlpack__");
            assert_eq!(imported.data_ptr() as usize, torch_ptr);
            assert_eq!(imported.ndim(), 1);
            assert_eq!(imported.shape(), &[4]);
            assert_eq!(imported.dtype_code(), DL_FLOAT);
            assert_eq!(imported.device_type(), DL_CPU);
            imported.release();

            PY_DEC_REF(ptr_result);
            PY_DEC_REF(empty_args);
            PY_DEC_REF(data_ptr_method);
            PY_DEC_REF(args);
            PY_DEC_REF(ones);
            PY_DEC_REF(tensor);
            PY_DEC_REF(torch_mod);
        });
    }

    /// Permanent regression test (R16 design note step 7): a real 4096x4096
    /// f64 buffer (134.2 MB) must cross to `torch` by pointer, not copy, and
    /// stay that way -- if a future change quietly turns this export into a
    /// memcpy, `ptr_match` starts failing here instead of only showing up
    /// as a silent perf regression somewhere else.
    #[test]
    fn export_4096x4096_is_pointer_identical_and_orders_of_magnitude_faster_than_a_copy() {
        if !is_python_available() {
            return;
        }
        with_gil(|| unsafe {
            if !HAS_CAPSULE.load(Ordering::SeqCst) {
                return;
            }
            let torch_mod = PY_IMPORT_IMPORT_MODULE(b"torch\0".as_ptr() as *const c_char);
            if torch_mod.is_null() {
                PY_ERR_CLEAR();
                return;
            }

            const N: usize = 4096;
            let mut data: Vec<f64> = vec![0.0; N * N];
            let ptr = data.as_mut_ptr() as *mut c_void;

            let start = std::time::Instant::now();
            let capsule = dlpack_export(DlpackSpec {
                ptr,
                dtype_code: DL_FLOAT,
                bits: 64,
                shape: &[N as i64, N as i64],
                strides_elems: &[N as i64, 1],
                device_type: DL_CPU,
                device_id: 0,
                deleter: None,
                deleter_ctx: std::ptr::null_mut(),
            });
            let export_cost = start.elapsed();
            assert!(!capsule.is_null());

            let from_dlpack_name = std::ffi::CString::new("from_dlpack").unwrap();
            let from_dlpack = PY_OBJECT_GET_ATTR_STRING(torch_mod, from_dlpack_name.as_ptr());
            let args_tuple = PY_TUPLE_NEW(1);
            PY_INC_REF(capsule);
            PY_TUPLE_SET_ITEM(args_tuple, 0, capsule);
            let tensor = PY_OBJECT_CALL_OBJECT(from_dlpack, args_tuple);
            assert!(!tensor.is_null());

            let data_ptr_name = std::ffi::CString::new("data_ptr").unwrap();
            let data_ptr_method = PY_OBJECT_GET_ATTR_STRING(tensor, data_ptr_name.as_ptr());
            let empty_args = PY_TUPLE_NEW(0);
            let ptr_result = PY_OBJECT_CALL_OBJECT(data_ptr_method, empty_args);
            let torch_ptr = PY_LONG_AS_LONG(ptr_result) as usize;
            assert_eq!(
                torch_ptr, ptr as usize,
                "4096x4096 export must stay pointer-identical, never a silent copy"
            );

            // A real forced copy of the same size, for a same-process,
            // same-run comparison baseline (not a cross-run number pasted
            // from a design note).
            let numpy_mod = PY_IMPORT_IMPORT_MODULE(b"numpy\0".as_ptr() as *const c_char);
            let copy_cost = if !numpy_mod.is_null() {
                let array_name = std::ffi::CString::new("array").unwrap();
                let np_array = PY_OBJECT_GET_ATTR_STRING(numpy_mod, array_name.as_ptr());
                let src = export_buffer_view_for_bench(ptr, N);
                let copy_args = PY_TUPLE_NEW(1);
                PY_INC_REF(src);
                PY_TUPLE_SET_ITEM(copy_args, 0, src);
                let kwargs = PY_DICT_NEW();
                let copy_key = std::ffi::CString::new("copy").unwrap();
                let true_val = PY_BOOL_FROM_LONG(1);
                PY_DICT_SET_ITEM_STRING(kwargs, copy_key.as_ptr(), true_val);
                let start = std::time::Instant::now();
                let copied = PY_OBJECT_CALL(np_array, copy_args, kwargs);
                let copy_cost = start.elapsed();
                assert!(!copied.is_null());
                PY_DEC_REF(true_val);
                PY_DEC_REF(kwargs);
                PY_DEC_REF(copy_args);
                PY_DEC_REF(src);
                PY_DEC_REF(copied);
                PY_DEC_REF(np_array);
                PY_DEC_REF(numpy_mod);
                Some(copy_cost)
            } else {
                PY_ERR_CLEAR();
                None
            };

            if let Some(copy_cost) = copy_cost {
                let ratio = copy_cost.as_secs_f64() / export_cost.as_secs_f64().max(1e-12);
                eprintln!(
                    "DLPack export (4096x4096, 134.2MB): {:.2}us; forced copy: {:.3}ms; ratio {:.0}x",
                    export_cost.as_secs_f64() * 1e6,
                    copy_cost.as_secs_f64() * 1e3,
                    ratio
                );
                assert!(
                    ratio > 100.0,
                    "export must be at least two orders of magnitude faster than a real copy"
                );
            }

            PY_DEC_REF(ptr_result);
            PY_DEC_REF(empty_args);
            PY_DEC_REF(data_ptr_method);
            PY_DEC_REF(args_tuple);
            PY_DEC_REF(from_dlpack);
            PY_DEC_REF(tensor);
            PY_DEC_REF(capsule);
            PY_DEC_REF(torch_mod);
        });
    }

    /// Builds a throwaway buffer-protocol view over `ptr` purely so the
    /// forced-copy baseline above can hand numpy something buffer-eligible
    /// without depending on the export path itself for the source object.
    unsafe fn export_buffer_view_for_bench(ptr: *mut c_void, n: usize) -> PyObject {
        unsafe {
            crate::python::python_export_buffer::export_buffer_view(
                crate::python::python_export_buffer::BufferSpec {
                    ptr,
                    itemsize: 8,
                    format: b'd',
                    shape: &[n as isize, n as isize],
                    strides_elems: &[n as isize, 1],
                    deleter: None,
                    deleter_ctx: std::ptr::null_mut(),
                },
            )
        }
    }
}
