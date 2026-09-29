use crate::python::*;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

/// Minimal C-compatible representation of `PyModuleDef` (Python 3.5+ layout).
#[repr(C)]
struct PyModuleDef {
    m_base: PyModuleDefBase,
    m_name: *const c_char,
    m_doc: *const c_char,
    m_size: isize,
    m_methods: *mut c_void,
    m_slots: *mut c_void,
    m_traverse: *mut c_void,
    m_clear: *mut c_void,
    m_free: *mut c_void,
}

/// Matches CPython's `PyModuleDef_Base` which starts with `PyObject_HEAD`
/// (ob_refcnt, ob_type) followed by m_init, m_index, m_copy.
#[repr(C)]
struct PyModuleDefBase {
    ob_refcnt: isize,
    ob_type: *mut c_void,
    m_init: Option<unsafe extern "C" fn() -> *mut c_void>,
    m_index: isize,
    m_copy: *mut c_void,
}

/// Heap-allocated bundle that holds both the `PyModuleDef` and the owned
/// module-name bytes so the C-string pointer stays valid for the module's
/// lifetime (the module def stores a raw `*const c_char`).
struct ModuleDefBundle {
    _name: CString,
    def: PyModuleDef,
}

/// CPython's `PYTHON_API_VERSION` — frozen at 1013 for 3.x.
const PYTHON_API_VERSION: c_int = 1013;

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_create_module(name: *const c_char) -> PyObject {
    check_python_loaded();
    unsafe {
        let name_str = CStr::from_ptr(name).to_str().unwrap();
        let name_owned = CString::new(name_str).unwrap();
        let bundle = Box::new(ModuleDefBundle {
            _name: name_owned,
            def: PyModuleDef {
                m_base: PyModuleDefBase {
                    ob_refcnt: 1_isize,
                    ob_type: std::ptr::null_mut(),
                    m_init: None,
                    m_index: 0,
                    m_copy: std::ptr::null_mut(),
                },
                m_name: std::ptr::null(), // filled below
                m_doc: std::ptr::null(),
                m_size: -1_isize,
                m_methods: std::ptr::null_mut(),
                m_slots: std::ptr::null_mut(),
                m_traverse: std::ptr::null_mut(),
                m_clear: std::ptr::null_mut(),
                m_free: std::ptr::null_mut(),
            },
        });

        // Write back the name pointer into the def, now that both share the
        // same allocation.
        let def_ptr: *mut PyModuleDef = &bundle.def as *const PyModuleDef as *mut PyModuleDef;
        (*def_ptr).m_name = bundle._name.as_ptr();

        let def_ptr_raw = def_ptr as *mut c_void;
        let module = PY_MODULE_CREATE2(def_ptr_raw, PYTHON_API_VERSION);
        if module.is_null() {
            // Module creation failed; reclaim the bundle.
            handle_py_error();
            return std::ptr::null_mut();
        }

        // Leak the bundle: the module def's name pointer must stay valid for
        // the lifetime of the module object.
        let _ = Box::into_raw(bundle);
        module
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_module_add_object(
    module: PyObject,
    name: *const c_char,
    obj: PyObject,
) -> i64 {
    check_python_loaded();
    // `module` may be a raw module pointer (from `olive_py_create_module`)
    // or a handle; unwrap passes raw pointers through, so this accepts both.
    with_gil(|| unsafe {
        let raw_module = olive_py_unwrap(module);
        let raw_obj = olive_py_unwrap(obj);
        if raw_module.is_null() || raw_obj.is_null() {
            return 0;
        }
        // `PyModule_AddObject` steals a reference on success, so hand it an
        // extra one: the Olive handle keeps its own, and the module owns the
        // new reference. Without this the handle's later decref would leave
        // the module pointing at a freed object.
        PY_INC_REF(raw_obj);
        let result = PY_MODULE_ADD_OBJECT(raw_module, name, raw_obj);
        if result < 0 {
            PY_DEC_REF(raw_obj);
            handle_py_error();
            0
        } else {
            1
        }
    })
}

/// Releases a raw module reference from `olive_py_create_module` on a
/// `PyInit_*` failure path. Only for raw module pointers, never slab
/// handles (`olive_py_decref` owns those and would double-free this one).
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_module_release(module: PyObject) {
    if module.is_null() {
        return;
    }
    with_gil(|| unsafe {
        PY_DEC_REF(module);
    });
}
