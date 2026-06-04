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
        PY_NUMBER_OR = compat_dlsym(handle, "PyNumber_Or");
        PY_NUMBER_ADD = compat_dlsym(handle, "PyNumber_Add");
        PY_NUMBER_SUBTRACT = compat_dlsym(handle, "PyNumber_Subtract");
        PY_NUMBER_MULTIPLY = compat_dlsym(handle, "PyNumber_Multiply");
        PY_NUMBER_TRUEDIVIDE = compat_dlsym(handle, "PyNumber_TrueDivide");
        PY_NUMBER_REMAINDER = compat_dlsym(handle, "PyNumber_Remainder");
        PY_NUMBER_POWER = compat_dlsym(handle, "PyNumber_Power");
        PY_FINALIZE = compat_dlsym(handle, "Py_Finalize");
        PY_IMPORT_IMPORT_MODULE = compat_dlsym(handle, "PyImport_ImportModule");
        PY_OBJECT_GET_ATTR_STRING = compat_dlsym(handle, "PyObject_GetAttrString");
        PY_OBJECT_SET_ATTR_STRING = compat_dlsym(handle, "PyObject_SetAttrString");
        PY_OBJECT_CALL_OBJECT = compat_dlsym(handle, "PyObject_CallObject");
        PY_OBJECT_CALL = compat_dlsym(handle, "PyObject_Call");
        PY_DEC_REF = compat_dlsym(handle, "Py_DecRef");
        PY_INC_REF = compat_dlsym(handle, "Py_IncRef");
        PY_LONG_AS_LONG = compat_dlsym(handle, "PyLong_AsLong");
        PY_NUMBER_LONG = compat_dlsym(handle, "PyNumber_Long");
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
        PY_OBJECT_RICHCOMPAREBOOL = compat_dlsym(handle, "PyObject_RichCompareBool");
        PY_SLICE_NEW = compat_dlsym(handle, "PySlice_New");
        PY_CORO_CHECK_EXACT = compat_dlsym(handle, "PyCoro_CheckExact");
        PY_ITER_CHECK = compat_dlsym(handle, "PyIter_Check");

        _PY_NONE_STRUCT = compat_dlsym(handle, "_Py_NoneStruct");

        let crucial_missing =
            std::mem::transmute::<_, *const ()>(PY_INITIALIZE).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_NUMBER_OR).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_FINALIZE).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_IMPORT_IMPORT_MODULE).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_OBJECT_GET_ATTR_STRING).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_DEC_REF).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_INC_REF).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_LONG_AS_LONG).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_UNICODE_AS_UTF8).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_LONG_FROM_LONG).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_UNICODE_FROM_STRING).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_LIST_NEW).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_LIST_SET_ITEM).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_OBJECT_GET_ITEM).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_OBJECT_SET_ITEM).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_OBJECT_DEL_ITEM).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_OBJECT_LENGTH).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_GILSTATE_ENSURE).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_GILSTATE_RELEASE).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_DICT_NEW).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_DICT_SET_ITEM_STRING).is_null() ||
            std::mem::transmute::<_, *const ()>(PY_SYS_GET_OBJECT).is_null();

        if crucial_missing {
            eprintln!("Warning: crucial Python API symbols are missing in the loaded library. Disabling Python interop.");
            LIBPYTHON = std::ptr::null_mut();
            PY_INITIALIZE = noop_initialize;
            PY_NUMBER_OR = noop_pynumber;
            PY_NUMBER_ADD = noop_pynumber;
            PY_NUMBER_SUBTRACT = noop_pynumber;
            PY_NUMBER_MULTIPLY = noop_pynumber;
            PY_NUMBER_TRUEDIVIDE = noop_pynumber;
            PY_NUMBER_REMAINDER = noop_pynumber;
            PY_NUMBER_POWER = noop_pynumber_power;
            PY_FINALIZE = noop_finalize;
            PY_IMPORT_IMPORT_MODULE = noop_import;
            PY_OBJECT_GET_ATTR_STRING = noop_getattr;
            PY_OBJECT_SET_ATTR_STRING = noop_setattr;
            PY_OBJECT_CALL_OBJECT = noop_call;
            PY_OBJECT_CALL = noop_call_kw;
            PY_DEC_REF = noop_decref;
            PY_INC_REF = noop_incref;
            PY_LONG_AS_LONG = noop_as_long;
            PY_NUMBER_LONG = noop_number_long;
            PY_FLOAT_AS_DOUBLE = noop_as_double;
            PY_UNICODE_AS_UTF8 = noop_as_utf8;
            PY_LONG_FROM_LONG = noop_from_long;
            PY_FLOAT_FROM_DOUBLE = noop_from_double;
            PY_UNICODE_FROM_STRING = noop_from_string;
            PY_LIST_NEW = noop_list_new;
            PY_LIST_SET_ITEM = noop_list_setitem;
            PY_OBJECT_GET_ITEM = noop_getitem;
            PY_OBJECT_SET_ITEM = noop_setitem;
            PY_OBJECT_DEL_ITEM = noop_dict_setitem_del;
            PY_OBJECT_LENGTH = noop_length;
            PY_GILSTATE_ENSURE = noop_gil_ensure;
            PY_GILSTATE_RELEASE = noop_gil_release;
            PY_TUPLE_NEW = noop_tuple_new;
            PY_TUPLE_SET_ITEM = noop_tuple_setitem;
            PY_DICT_NEW = noop_dict_new;
            PY_DICT_SET_ITEM_STRING = noop_dict_setitemstring;
            PY_DICT_KEYS = noop_call_1;
            PY_OBJECT_TYPE = noop_call_1;
            PY_OBJECT_STR = noop_call_1;
            PY_ERR_OCCURRED = noop_dict_new;
            PY_ERR_FETCH = noop_err_fetch;
            PY_ERR_NORMALIZE_EXCEPTION = noop_err_fetch;
            PY_ERR_CLEAR = noop_initialize;
            PY_SET_NEW = noop_call_1;
            PY_SET_ADD = noop_set_add;
            PY_BYTES_FROM_STRING_AND_SIZE = noop_bytes_from_string;
            PY_SEQUENCE_LIST = noop_call_1;
            PY_BYTES_AS_STRING = noop_bytes_as_string;
            PY_BYTES_SIZE = noop_bytes_size;
            PY_TYPE_IS_SUBTYPE = noop_is_subtype;
            PY_EVAL_SAVE_THREAD = noop_save_thread;
            PY_EVAL_RESTORE_THREAD = noop_restore_thread;
            PY_EVAL_INIT_THREADS = noop_initialize;
            PY_SYS_GET_OBJECT = noop_import;
            PY_OBJECT_GET_ITER = noop_get_iter;
            PY_ITER_NEXT = noop_iter_next;
            PY_OBJECT_RICHCOMPAREBOOL = crate::python::python_noop::noop_richcomparebool;
            PY_CORO_CHECK_EXACT = noop_check_int;
            PY_ITER_CHECK = noop_check_int;
            PY_DICT_NEXT = noop_dict_next;
            PY_LIST_GET_ITEM = noop_getitem_idx;
            PY_TUPLE_GET_ITEM = noop_getitem_idx;
            PY_ERR_PRINT = noop_err_print;
            PY_SLICE_NEW = crate::python::python_noop::noop_slice_new;
            return;
        }

        PY_INITIALIZE();

        // Prepend '' (cwd) to sys.path so project-local Python modules take
        // priority over any same-named files in site-packages. Embedded Python
        // does not add '' automatically unlike interactive/script mode.
        PY_RUN_SIMPLE_STRING(b"import sys; sys.path.insert(0, '')\0".as_ptr() as *const c_char);

        let init_ptr: *const () = std::mem::transmute(PY_EVAL_INIT_THREADS);
        if !init_ptr.is_null() && init_ptr != (noop_initialize as *const ()) {
            PY_EVAL_INIT_THREADS();
        }

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

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_check_alive(ptr: i64) -> i64 {
    if crate::is_active_object(ptr) { 1 } else { 0 }
}
