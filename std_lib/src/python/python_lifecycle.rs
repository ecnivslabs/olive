use crate::python::python_compat::*;
use crate::python::*;
use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::sync::atomic::Ordering;

fn find_active_python_library() -> Option<String> {
    for cmd in &["python3", "python"] {
        if let Ok(output) = std::process::Command::new(cmd)
            .args([
                "-c",
                "import sys, os, sysconfig; \
                 libdir = sysconfig.get_config_var('LIBDIR'); \
                 ldlibrary = sysconfig.get_config_var('LDLIBRARY'); \
                 path = os.path.join(libdir or '', ldlibrary or '') if libdir and ldlibrary else ''; \
                 print(path if os.path.exists(path) else (ldlibrary or ''))",
            ])
            .output()
        {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    return Some(path_str);
                }
            }
        }
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_initialize() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let mut handle: *mut c_void = std::ptr::null_mut();

        if let Ok(env_path) =
            std::env::var("OLIVE_PYTHON_PATH").or_else(|_| std::env::var("PYTHON_LIBRARY"))
        {
            handle = compat_dlopen(&env_path);
        }

        if handle.is_null() {
            if let Some(detected_path) = find_active_python_library() {
                handle = compat_dlopen(&detected_path);
            }
        }

        if handle.is_null() {
            #[cfg(target_os = "windows")]
            {
                for name in &[
                    "python3.dll",
                    "python312.dll",
                    "python311.dll",
                    "python310.dll",
                    "python39.dll",
                ] {
                    handle = compat_dlopen(name);
                    if !handle.is_null() {
                        break;
                    }
                }
            }
            #[cfg(target_os = "macos")]
            {
                for name in &[
                    "libpython3.dylib",
                    "libpython3.12.dylib",
                    "libpython3.11.dylib",
                    "libpython3.10.dylib",
                    "libpython3.9.dylib",
                    "/opt/homebrew/lib/libpython3.11.dylib",
                    "/opt/homebrew/lib/libpython3.12.dylib",
                ] {
                    handle = compat_dlopen(name);
                    if !handle.is_null() {
                        break;
                    }
                }
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                for name in &[
                    "libpython3.so",
                    "libpython3.12.so",
                    "libpython3.11.so",
                    "libpython3.10.so",
                    "libpython3.9.so",
                ] {
                    handle = compat_dlopen(name);
                    if !handle.is_null() {
                        break;
                    }
                }
            }
        }

        if handle.is_null() {
            eprintln!("Warning: could not load libpython3. Python interop will not work.");
            return;
        }
        LIBPYTHON = handle;

        PY_INITIALIZE = compat_dlsym(handle, "Py_Initialize");
        PY_FINALIZE = compat_dlsym(handle, "Py_Finalize");
        PY_IMPORT_IMPORT_MODULE = compat_dlsym(handle, "PyImport_ImportModule");
        PY_OBJECT_GET_ATTR_STRING = compat_dlsym(handle, "PyObject_GetAttrString");
        PY_OBJECT_SET_ATTR_STRING = compat_dlsym(handle, "PyObject_SetAttrString");
        PY_OBJECT_CALL_OBJECT = compat_dlsym(handle, "PyObject_CallObject");
        PY_OBJECT_CALL = compat_dlsym(handle, "PyObject_Call");
        PY_DEC_REF = compat_dlsym(handle, "Py_DecRef");
        PY_INC_REF = compat_dlsym(handle, "Py_IncRef");
        PY_LONG_AS_LONG = compat_dlsym(handle, "PyLong_AsLong");
        PY_FLOAT_AS_DOUBLE = compat_dlsym(handle, "PyFloat_AsDouble");
        PY_UNICODE_AS_UTF8 = compat_dlsym(handle, "PyUnicode_AsUTF8");
        PY_LONG_FROM_LONG = compat_dlsym(handle, "PyLong_FromLong");
        PY_FLOAT_FROM_DOUBLE = compat_dlsym(handle, "PyFloat_FromDouble");
        PY_UNICODE_FROM_STRING = compat_dlsym(handle, "PyUnicode_FromString");
        PY_LIST_NEW = compat_dlsym(handle, "PyList_New");
        PY_LIST_SET_ITEM = compat_dlsym(handle, "PyList_SetItem");
        PY_OBJECT_GET_ITEM = compat_dlsym(handle, "PyObject_GetItem");
        PY_OBJECT_SET_ITEM = compat_dlsym(handle, "PyObject_SetItem");
        PY_OBJECT_DEL_ITEM = compat_dlsym(handle, "PyObject_DelItem");
        PY_OBJECT_LENGTH = compat_dlsym(handle, "PyObject_Length");
        PY_GILSTATE_ENSURE = compat_dlsym(handle, "PyGILState_Ensure");
        PY_GILSTATE_RELEASE = compat_dlsym(handle, "PyGILState_Release");
        PY_TUPLE_NEW = compat_dlsym(handle, "PyTuple_New");
        PY_TUPLE_SET_ITEM = compat_dlsym(handle, "PyTuple_SetItem");
        PY_DICT_NEW = compat_dlsym(handle, "PyDict_New");
        PY_DICT_SET_ITEM_STRING = compat_dlsym(handle, "PyDict_SetItemString");
        PY_SET_NEW = compat_dlsym(handle, "PySet_New");
        PY_SET_ADD = compat_dlsym(handle, "PySet_Add");
        PY_BYTES_FROM_STRING_AND_SIZE = compat_dlsym(handle, "PyBytes_FromStringAndSize");
        PY_SEQUENCE_LIST = compat_dlsym(handle, "PySequence_List");
        PY_BYTES_AS_STRING = compat_dlsym(handle, "PyBytes_AsString");
        PY_BYTES_SIZE = compat_dlsym(handle, "PyBytes_Size");
        PY_DICT_KEYS = compat_dlsym(handle, "PyDict_Keys");
        PY_OBJECT_TYPE = compat_dlsym(handle, "PyObject_Type");
        PY_OBJECT_STR = compat_dlsym(handle, "PyObject_Str");
        PY_ERR_OCCURRED = compat_dlsym(handle, "PyErr_Occurred");
        PY_ERR_FETCH = compat_dlsym(handle, "PyErr_Fetch");
        PY_ERR_NORMALIZE_EXCEPTION = compat_dlsym(handle, "PyErr_NormalizeException");
        PY_ERR_CLEAR = compat_dlsym(handle, "PyErr_Clear");
        PY_ERR_PRINT = compat_dlsym(handle, "PyErr_Print");
        PY_RUN_SIMPLE_STRING = compat_dlsym(handle, "PyRun_SimpleString");

        PY_BOOL_TYPE = compat_dlsym(handle, "PyBool_Type");
        PY_LONG_TYPE = compat_dlsym(handle, "PyLong_Type");
        PY_FLOAT_TYPE = compat_dlsym(handle, "PyFloat_Type");
        PY_UNICODE_TYPE = compat_dlsym(handle, "PyUnicode_Type");
        PY_LIST_TYPE = compat_dlsym(handle, "PyList_Type");
        PY_DICT_TYPE = compat_dlsym(handle, "PyDict_Type");
        PY_SET_TYPE = compat_dlsym(handle, "PySet_Type");
        PY_BYTES_TYPE = compat_dlsym(handle, "PyBytes_Type");
        PY_TYPE_IS_SUBTYPE = compat_dlsym(handle, "PyType_IsSubtype");
        PY_EVAL_SAVE_THREAD = compat_dlsym(handle, "PyEval_SaveThread");
        PY_EVAL_RESTORE_THREAD = compat_dlsym(handle, "PyEval_RestoreThread");
        PY_EVAL_INIT_THREADS = compat_dlsym(handle, "PyEval_InitThreads");
        PY_SYS_GET_OBJECT = compat_dlsym(handle, "PySys_GetObject");

        PY_DICT_NEXT = compat_dlsym(handle, "PyDict_Next");
        PY_LIST_GET_ITEM = compat_dlsym(handle, "PyList_GetItem");
        PY_TUPLE_GET_ITEM = compat_dlsym(handle, "PyTuple_GetItem");
        PY_TUPLE_TYPE = compat_dlsym(handle, "PyTuple_Type");
        PY_OBJECT_GET_ITER = compat_dlsym(handle, "PyObject_GetIter");
        PY_ITER_NEXT = compat_dlsym(handle, "PyIter_Next");
        PY_CORO_CHECK_EXACT = compat_dlsym(handle, "PyCoro_CheckExact");
        PY_ITER_CHECK = compat_dlsym(handle, "PyIter_Check");

        _PY_NONE_STRUCT = compat_dlsym(handle, "_Py_NoneStruct");

        PY_INITIALIZE();

        let init_ptr: *const () = std::mem::transmute(PY_EVAL_INIT_THREADS);
        if !init_ptr.is_null() && init_ptr != (noop_initialize as *const ()) {
            PY_EVAL_INIT_THREADS();
        }

        // Check Python major version is 3
        {
            let ver_obj = PY_SYS_GET_OBJECT(b"version_info\0".as_ptr() as *const c_char);
            if !ver_obj.is_null() {
                let major_key = CString::new("major").unwrap();
                let major_attr = PY_OBJECT_GET_ATTR_STRING(ver_obj, major_key.as_ptr());
                if !major_attr.is_null() {
                    let major = PY_LONG_AS_LONG(major_attr) as i64;
                    PY_DEC_REF(major_attr);
                    if major < 3 {
                        eprintln!(
                            "Warning: Python {major} detected - Python interop requires Python 3. \
                             Olive Python interop will not function correctly."
                        );
                    }
                }
            }
        }

        crate::python_proxy::setup_native_proxies(handle, compat_dlsym);

        // Preload traceback formatter
        let tb_mod = PY_IMPORT_IMPORT_MODULE(b"traceback\0".as_ptr() as *const c_char);
        if !tb_mod.is_null() {
            let fmt_fn =
                PY_OBJECT_GET_ATTR_STRING(tb_mod, b"format_exception\0".as_ptr() as *const c_char);
            if !fmt_fn.is_null() {
                PY_TRACEBACK_FORMAT_EXCEPTION = fmt_fn;
            }
            PY_DEC_REF(tb_mod);
        }

        let save_ptr: *const () = std::mem::transmute(PY_EVAL_SAVE_THREAD);
        if !save_ptr.is_null() && save_ptr != (noop_save_thread as *const ()) {
            MAIN_THREAD_STATE = PY_EVAL_SAVE_THREAD();
        }
        INITIALIZED.store(true, Ordering::SeqCst);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_finalize() {
    unsafe {
        if INITIALIZED.load(Ordering::SeqCst) {
            let restore_ptr: *const () = std::mem::transmute(PY_EVAL_RESTORE_THREAD);
            if !restore_ptr.is_null()
                && restore_ptr != (noop_restore_thread as *const ())
                && !MAIN_THREAD_STATE.is_null()
            {
                PY_EVAL_RESTORE_THREAD(MAIN_THREAD_STATE);
                MAIN_THREAD_STATE = std::ptr::null_mut();
            } else {
                let _gil = PY_GILSTATE_ENSURE();
            }
            if !PY_TRACEBACK_FORMAT_EXCEPTION.is_null() {
                PY_DEC_REF(PY_TRACEBACK_FORMAT_EXCEPTION);
                PY_TRACEBACK_FORMAT_EXCEPTION = std::ptr::null_mut();
            }
            PY_FINALIZE();

            INITIALIZED.store(false, Ordering::SeqCst);
        }
    }
}

pub fn is_readable_ptr(ptr: *const c_void) -> bool {
    crate::is_active_object(ptr as i64)
}

/// Called from ctypes proxy `_check_alive`. Returns 1 if `ptr` still points to
/// a live Olive object, 0 if it has been freed. This prevents use-after-free
/// when Python holds a proxy to an Olive list/dict that was subsequently freed.
#[unsafe(no_mangle)]
pub extern "C" fn olive_py_check_alive(ptr: i64) -> i64 {
    if crate::is_active_object(ptr) { 1 } else { 0 }
}
