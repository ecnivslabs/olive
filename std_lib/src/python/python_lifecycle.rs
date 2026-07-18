use crate::python::python_compat::*;
use crate::python::*;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::Ordering;

/// Extract the Python minor version from a `libpython3.X.so` filename.
/// Returns 0 for unversioned names (e.g. `libpython3.so` symlink).
fn extract_python_minor(name: &str) -> u64 {
    let rest = match name.strip_prefix("libpython3") {
        Some(r) => r,
        None => return 0,
    };
    if rest.is_empty() {
        return 0;
    }
    let after_dot = match rest.strip_prefix('.') {
        Some(d) => d,
        None => return 0,
    };
    let minor_str = after_dot.split('.').next().unwrap_or("");
    minor_str.parse().unwrap_or(0)
}

/// Scan standard library directories for Python 3 shared libraries.
/// Returns paths sorted by version (highest first). Discovers any Python 3.x
/// version without hardcoding -- handles 3.13, 3.14, and beyond automatically.
fn detect_python_libraries() -> Vec<String> {
    if cfg!(target_os = "windows") {
        return Vec::new();
    }

    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };

    let search_dirs: &[&str] = if cfg!(target_os = "macos") {
        &["/usr/lib", "/usr/local/lib", "/opt/homebrew/lib"]
    } else {
        &[
            "/usr/lib",
            "/usr/lib64",
            "/lib",
            "/lib64",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib/arm-linux-gnueabihf",
            "/lib/x86_64-linux-gnu",
            "/lib/aarch64-linux-gnu",
            "/usr/local/lib",
        ]
    };

    let mut candidates: Vec<(u64, String)> = Vec::new();

    for dir in search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name() else {
                    continue;
                };
                let name = name.to_string_lossy();
                if !name.starts_with("libpython3") {
                    continue;
                }
                if !name.ends_with(ext) {
                    continue;
                }
                candidates.push((
                    extract_python_minor(&name),
                    path.to_string_lossy().to_string(),
                ));
            }
        }
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().map(|(_, p)| p).collect()
}

fn find_active_python_library() -> Option<String> {
    // sysconfig's LIBDIR/LDLIBRARY join is not enough on its own:
    //   - on non-Framework builds LDLIBRARY can name the *static* .a
    //     import lib (exists on disk, but dlopen can never load it), and
    //   - on macOS Framework builds (e.g. actions/setup-python's images)
    //     the loadable image isn't a "libpythonX.Y.dylib" at all -- it's
    //     a bare file named `Python` living at the version prefix root.
    // Build every candidate this interpreter could plausibly expose and
    // take the first one that both exists and isn't a static archive.
    let script = r#"
import sys, os, sysconfig
ver = f'{sys.version_info.major}.{sys.version_info.minor}'
abiflags = sysconfig.get_config_var('ABIFLAGS') or ''
libdir = sysconfig.get_config_var('LIBDIR') or ''
ldlibrary = sysconfig.get_config_var('LDLIBRARY') or ''
ext = '.dylib' if sys.platform == 'darwin' else '.so'
candidates = []
if libdir and ldlibrary:
    candidates.append(os.path.join(libdir, ldlibrary))
if libdir:
    candidates.append(os.path.join(libdir, 'libpython' + ver + abiflags + ext))
    candidates.append(os.path.join(libdir, 'libpython' + ver + ext))
candidates.append(os.path.join(sys.base_prefix, 'Python'))
found = ''
for c in candidates:
    if c and not c.endswith('.a') and os.path.exists(c):
        found = c
        break
print(found)
"#;
    for cmd in &["python3", "python"] {
        if let Ok(output) = std::process::Command::new(cmd)
            .args(["-c", script])
            .output()
            && output.status.success()
        {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path_str.is_empty() {
                return Some(path_str);
            }
        }
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_initialize() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        crate::slab::tune_allocator();
        let mut handle: *mut c_void = std::ptr::null_mut();

        // Host process (R20): when loaded as a CPython extension module,
        // libpython is already loaded. Check via RTLD_DEFAULT first.
        #[cfg(not(target_os = "windows"))]
        let host_py_isinit = libc::dlsym(
            libc::RTLD_DEFAULT,
            b"Py_IsInitialized\0".as_ptr() as *const c_char,
        );
        #[cfg(target_os = "windows")]
        let host_py_isinit: *mut c_void =
            compat_dlsym(compat_dlopen_current_process(), "Py_IsInitialized");
        if !host_py_isinit.is_null() {
            let is_init: unsafe extern "C" fn() -> c_int = std::mem::transmute(host_py_isinit);
            if is_init() != 0 {
                // Python is already running; use the host process handle
                // to resolve symbols rather than loading a duplicate
                // libpython.
                handle = compat_dlopen_current_process();
            }
        }

        if handle.is_null()
            && let Ok(env_path) =
                std::env::var("OLIVE_PYTHON_PATH").or_else(|_| std::env::var("PYTHON_LIBRARY"))
        {
            handle = compat_dlopen(&env_path);
        }

        if handle.is_null()
            && let Some(detected_path) = find_active_python_library() {
                handle = compat_dlopen(&detected_path);
            }

        if handle.is_null() {
            // Dynamic directory scan: discovers any libpython3.X.so/.dylib on the
            // system and tries the highest version first. Supports any Python 3.x
            // without needing to update a hardcoded version list.
            for path in detect_python_libraries() {
                handle = compat_dlopen(&path);
                if !handle.is_null() {
                    break;
                }
            }

            // Final fallback: bare name lets the dynamic linker search its standard
            // paths (ld.so.cache, LD_LIBRARY_PATH, /usr/lib, etc.).
            #[cfg(target_os = "windows")]
            {
                for name in &[
                    "python3.dll",
                    "python312.dll",
                    "python311.dll",
                    "python310.dll",
                    "python39.dll",
                ] {
                    if !handle.is_null() {
                        break;
                    }
                    handle = compat_dlopen(name);
                }
            }
            #[cfg(not(target_os = "windows"))]
            if handle.is_null() {
                handle = compat_dlopen(if cfg!(target_os = "macos") {
                    "libpython3.dylib"
                } else {
                    "libpython3.so"
                });
            }
        }

        if handle.is_null() {
            eprintln!(
                "Warning: could not load libpython3 ({}). Python interop will not work.",
                compat_dl_error()
            );
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
        PY_UNICODE_AS_UTF8_AND_SIZE = compat_dlsym(handle, "PyUnicode_AsUTF8AndSize");
        PY_UNICODE_FROM_STRING_AND_SIZE = compat_dlsym(handle, "PyUnicode_FromStringAndSize");
        PY_LONG_FROM_LONG = compat_dlsym(handle, "PyLong_FromLong");
        PY_BOOL_FROM_LONG = compat_dlsym(handle, "PyBool_FromLong");
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
        PY_BYTES_FROM_OBJECT = compat_dlsym(handle, "PyBytes_FromObject");
        PY_DICT_KEYS = compat_dlsym(handle, "PyDict_Keys");
        PY_OBJECT_TYPE = compat_dlsym(handle, "PyObject_Type");
        PY_OBJECT_STR = compat_dlsym(handle, "PyObject_Str");
        PY_ERR_OCCURRED = compat_dlsym(handle, "PyErr_Occurred");
        PY_ERR_FETCH = compat_dlsym(handle, "PyErr_Fetch");
        PY_ERR_NORMALIZE_EXCEPTION = compat_dlsym(handle, "PyErr_NormalizeException");
        PY_ERR_CLEAR = compat_dlsym(handle, "PyErr_Clear");
        PY_ERR_PRINT = compat_dlsym(handle, "PyErr_Print");
        PY_IS_INITIALIZED = compat_dlsym(handle, "Py_IsInitialized");
        PY_MODULE_NEW = compat_dlsym(handle, "PyModule_New");
        PY_MODULE_ADD_OBJECT = compat_dlsym(handle, "PyModule_AddObject");
        PY_MODULE_CREATE2 = compat_dlsym(handle, "PyModule_Create2");
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
        PY_TUPLE_SIZE = compat_dlsym(handle, "PyTuple_Size");
        PY_TUPLE_TYPE = compat_dlsym(handle, "PyTuple_Type");
        PY_OBJECT_GET_ITER = compat_dlsym(handle, "PyObject_GetIter");
        PY_ITER_NEXT = compat_dlsym(handle, "PyIter_Next");
        PY_OBJECT_RICHCOMPAREBOOL = compat_dlsym(handle, "PyObject_RichCompareBool");
        PY_SLICE_NEW = compat_dlsym(handle, "PySlice_New");
        PY_CORO_CHECK_EXACT = compat_dlsym(handle, "PyCoro_CheckExact");
        PY_ITER_CHECK = compat_dlsym(handle, "PyIter_Check");
        PY_VECTORCALL = compat_dlsym(handle, "PyObject_Vectorcall");
        PY_VECTORCALL_METHOD = compat_dlsym(handle, "PyObject_VectorcallMethod");
        PY_OBJECT_GET_BUFFER = compat_dlsym(handle, "PyObject_GetBuffer");
        PY_BUFFER_RELEASE = compat_dlsym(handle, "PyBuffer_Release");
        PY_OBJECT_CHECK_BUFFER = compat_dlsym(handle, "PyObject_CheckBuffer");
        PY_UNICODE_INTERN_FROM_STRING = compat_dlsym(handle, "PyUnicode_InternFromString");
        PY_OBJECT_GET_ATTR = compat_dlsym(handle, "PyObject_GetAttr");
        PY_OBJECT_SET_ATTR = compat_dlsym(handle, "PyObject_SetAttr");
        PY_CAPSULE_NEW = compat_dlsym(handle, "PyCapsule_New");
        PY_CAPSULE_GET_POINTER = compat_dlsym(handle, "PyCapsule_GetPointer");
        PY_CAPSULE_IS_VALID = compat_dlsym(handle, "PyCapsule_IsValid");
        PY_CAPSULE_SET_NAME = compat_dlsym(handle, "PyCapsule_SetName");
        PY_TYPE_FROM_SPEC = compat_dlsym(handle, "PyType_FromSpec");
        PY_TYPE_GENERIC_ALLOC = compat_dlsym(handle, "PyType_GenericAlloc");
        PY_OBJECT_FREE = compat_dlsym(handle, "PyObject_Free");
        PY_CFUNCTION_NEW_EX = compat_dlsym(handle, "PyCFunction_NewEx");
        PY_ERR_SET_STRING = compat_dlsym(handle, "PyErr_SetString");
        PY_EVAL_ACQUIRE_THREAD = compat_dlsym(handle, "PyEval_AcquireThread");
        PY_EVAL_RELEASE_THREAD = compat_dlsym(handle, "PyEval_ReleaseThread");
        PY_NEW_INTERPRETER_FROM_CONFIG = compat_dlsym(handle, "Py_NewInterpreterFromConfig");
        PY_END_INTERPRETER = compat_dlsym(handle, "Py_EndInterpreter");
        PY_THREAD_STATE_NEW = compat_dlsym(handle, "PyThreadState_New");
        PY_THREAD_STATE_CLEAR = compat_dlsym(handle, "PyThreadState_Clear");
        PY_THREAD_STATE_DELETE = compat_dlsym(handle, "PyThreadState_Delete");
        PY_THREAD_STATE_SWAP = compat_dlsym(handle, "PyThreadState_Swap");
        PY_INTERPRETER_STATE_GET = compat_dlsym(handle, "PyInterpreterState_Get");
        PY_THREAD_STATE_GET = compat_dlsym(handle, "PyThreadState_Get");
        // Unlike `PyBool_Type`/`_Py_NoneStruct` (plain struct values, whose
        // own address already is the `PyObject*` we want), `PyExc_TypeError`
        // is declared `PyObject *PyExc_TypeError` in CPython -- a pointer
        // *variable*. `dlsym` gives that variable's address; one more read
        // gets the exception object it actually points to.
        let exc_type_error_slot: *const PyObject = compat_dlsym(handle, "PyExc_TypeError");
        PY_EXC_TYPE_ERROR = if exc_type_error_slot.is_null() {
            std::ptr::null_mut()
        } else {
            *exc_type_error_slot
        };

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
            PY_UNICODE_AS_UTF8_AND_SIZE = crate::python::python_noop::noop_as_utf8_and_size;
            PY_UNICODE_FROM_STRING_AND_SIZE =
                crate::python::python_noop::noop_from_string_and_size;
            HAS_STR_AND_SIZE.store(false, Ordering::SeqCst);
            PY_LONG_FROM_LONG = noop_from_long;
            PY_BOOL_FROM_LONG = noop_from_long;
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
            PY_IS_INITIALIZED = noop_is_initialized;
            PY_MODULE_NEW = noop_module_new;
            PY_MODULE_ADD_OBJECT = noop_module_add_object;
            PY_MODULE_CREATE2 = noop_module_create2;
            PY_CORO_CHECK_EXACT = noop_check_int;
            PY_ITER_CHECK = noop_check_int;
            PY_DICT_NEXT = noop_dict_next;
            PY_LIST_GET_ITEM = noop_getitem_idx;
            PY_TUPLE_GET_ITEM = noop_getitem_idx;
            PY_TUPLE_SIZE = noop_tuple_size;
            PY_ERR_PRINT = noop_err_print;
            PY_SLICE_NEW = crate::python::python_noop::noop_slice_new;
            PY_VECTORCALL = noop_vectorcall;
            PY_VECTORCALL_METHOD = noop_vectorcall;
            HAS_VECTORCALL.store(false, Ordering::SeqCst);
            PY_OBJECT_GET_BUFFER = noop_get_buffer;
            PY_BUFFER_RELEASE = noop_buffer_release;
            PY_OBJECT_CHECK_BUFFER = noop_check_int;
            HAS_BUFFER.store(false, Ordering::SeqCst);
            PY_UNICODE_INTERN_FROM_STRING = noop_from_string;
            PY_OBJECT_GET_ATTR = noop_getitem;
            PY_OBJECT_SET_ATTR = noop_setitem;
            HAS_INTERN.store(false, Ordering::SeqCst);
            PY_CAPSULE_NEW = crate::python::python_noop::noop_capsule_new;
            PY_CAPSULE_GET_POINTER = crate::python::python_noop::noop_capsule_get_pointer;
            PY_CAPSULE_IS_VALID = crate::python::python_noop::noop_capsule_name_check;
            PY_CAPSULE_SET_NAME = crate::python::python_noop::noop_capsule_name_check;
            PY_TYPE_FROM_SPEC = crate::python::python_noop::noop_type_from_spec;
            PY_TYPE_GENERIC_ALLOC = crate::python::python_noop::noop_type_generic_alloc;
            PY_OBJECT_FREE = crate::python::python_noop::noop_object_free;
            HAS_CAPSULE.store(false, Ordering::SeqCst);
            HAS_TYPE_FROMSPEC.store(false, Ordering::SeqCst);
            PY_CFUNCTION_NEW_EX = crate::python::python_noop::noop_cfunction_new_ex;
            PY_ERR_SET_STRING = crate::python::python_noop::noop_err_set_string;
            PY_EXC_TYPE_ERROR = std::ptr::null_mut();
            PY_EVAL_ACQUIRE_THREAD = crate::python::python_noop::noop_acquire_thread;
            PY_EVAL_RELEASE_THREAD = crate::python::python_noop::noop_release_thread;
            PY_NEW_INTERPRETER_FROM_CONFIG = crate::python::python_noop::noop_new_interpreter;
            PY_END_INTERPRETER = crate::python::python_noop::noop_end_interpreter;
            PY_THREAD_STATE_NEW = crate::python::python_noop::noop_thread_state_new;
            PY_THREAD_STATE_CLEAR = crate::python::python_noop::noop_thread_state_clear;
            PY_THREAD_STATE_DELETE = crate::python::python_noop::noop_thread_state_delete;
            PY_THREAD_STATE_SWAP = crate::python::python_noop::noop_thread_state_swap;
            PY_INTERPRETER_STATE_GET = crate::python::python_noop::noop_interpreter_state_get;
            PY_THREAD_STATE_GET = crate::python::python_noop::noop_thread_state_get;
            return;
        }

        // Missing on pre-3.9 or exotic builds; a test-only override lets the
        // suite exercise the tuple-call fallback on a build that does have it.
        let vectorcall_present = !std::mem::transmute::<_, *const ()>(PY_VECTORCALL).is_null()
            && !std::mem::transmute::<_, *const ()>(PY_VECTORCALL_METHOD).is_null();
        let vectorcall_disabled_for_test =
            std::env::var("OLIVE_PY_NO_VECTORCALL").as_deref() == Ok("1");
        HAS_VECTORCALL.store(
            vectorcall_present && !vectorcall_disabled_for_test,
            Ordering::SeqCst,
        );

        // Core CPython API since 2.x; only missing on an exotic or crippled
        // build. A test-only override exercises the GetAttrString fallback.
        let intern_present =
            !std::mem::transmute::<_, *const ()>(PY_UNICODE_INTERN_FROM_STRING).is_null()
                && !std::mem::transmute::<_, *const ()>(PY_OBJECT_GET_ATTR).is_null()
                && !std::mem::transmute::<_, *const ()>(PY_OBJECT_SET_ATTR).is_null();
        let intern_disabled_for_test = std::env::var("OLIVE_PY_NO_INTERN").as_deref() == Ok("1");
        HAS_INTERN.store(intern_present && !intern_disabled_for_test, Ordering::SeqCst);

        // Missing on exotic builds without the buffer protocol compiled in.
        // A test-only override exercises the per-element fallback on a build
        // that does have it.
        let buffer_present =
            !std::mem::transmute::<_, *const ()>(PY_OBJECT_GET_BUFFER).is_null()
                && !std::mem::transmute::<_, *const ()>(PY_BUFFER_RELEASE).is_null()
                && !std::mem::transmute::<_, *const ()>(PY_OBJECT_CHECK_BUFFER).is_null();
        let buffer_disabled_for_test = std::env::var("OLIVE_PY_NO_BUFFER").as_deref() == Ok("1");
        HAS_BUFFER.store(buffer_present && !buffer_disabled_for_test, Ordering::SeqCst);

        // Missing on exotic builds without capsule/dynamic-type support. A
        // test-only override exercises the copy-based fallback on a build
        // that does have it (R16).
        let capsule_present = !std::mem::transmute::<_, *const ()>(PY_CAPSULE_NEW).is_null()
            && !std::mem::transmute::<_, *const ()>(PY_CAPSULE_GET_POINTER).is_null()
            && !std::mem::transmute::<_, *const ()>(PY_CAPSULE_IS_VALID).is_null()
            && !std::mem::transmute::<_, *const ()>(PY_CAPSULE_SET_NAME).is_null();
        let capsule_disabled_for_test = std::env::var("OLIVE_PY_NO_CAPSULE").as_deref() == Ok("1");
        HAS_CAPSULE.store(capsule_present && !capsule_disabled_for_test, Ordering::SeqCst);

        let type_fromspec_present =
            !std::mem::transmute::<_, *const ()>(PY_TYPE_FROM_SPEC).is_null()
                && !std::mem::transmute::<_, *const ()>(PY_TYPE_GENERIC_ALLOC).is_null()
                && !std::mem::transmute::<_, *const ()>(PY_OBJECT_FREE).is_null();
        let type_fromspec_disabled_for_test =
            std::env::var("OLIVE_PY_NO_TYPE_FROMSPEC").as_deref() == Ok("1");
        HAS_TYPE_FROMSPEC.store(
            type_fromspec_present && !type_fromspec_disabled_for_test,
            Ordering::SeqCst,
        );

        // Present since CPython 3.3; missing only on exotic or crippled
        // builds. A test-only override exercises the strlen-based fallback
        // (and its embedded-NUL truncation) on a build that does have it.
        let str_and_size_present =
            !std::mem::transmute::<_, *const ()>(PY_UNICODE_FROM_STRING_AND_SIZE).is_null()
                && !std::mem::transmute::<_, *const ()>(PY_UNICODE_AS_UTF8_AND_SIZE).is_null();
        let str_and_size_disabled_for_test =
            std::env::var("OLIVE_PY_NO_STR_AND_SIZE").as_deref() == Ok("1");
        HAS_STR_AND_SIZE.store(
            str_and_size_present && !str_and_size_disabled_for_test,
            Ordering::SeqCst,
        );

        let already_initialized = {
            let is_init_ptr: *const () = PY_IS_INITIALIZED as *const ();
            !is_init_ptr.is_null()
                && is_init_ptr != (noop_is_initialized as *const ())
                && PY_IS_INITIALIZED() != 0
        };
        if !already_initialized {
            PY_INITIALIZE();

            PY_RUN_SIMPLE_STRING(
                b"import sys; sys.path.insert(0, '')\0".as_ptr() as *const c_char,
            );

            let init_ptr: *const () = PY_EVAL_INIT_THREADS as *const ();
            if !init_ptr.is_null() && init_ptr != (noop_initialize as *const ()) {
                PY_EVAL_INIT_THREADS();
            }
        }

        {
            let ver_obj = PY_SYS_GET_OBJECT(b"version_info\0".as_ptr() as *const c_char);
            if !ver_obj.is_null() {
                let major_key = CString::new("major").unwrap();
                let major_attr = PY_OBJECT_GET_ATTR_STRING(ver_obj, major_key.as_ptr());
                if !major_attr.is_null() {
                    let major = PY_LONG_AS_LONG(major_attr);
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

        let tb_mod = PY_IMPORT_IMPORT_MODULE(b"traceback\0".as_ptr() as *const c_char);
        if !tb_mod.is_null() {
            let fmt_fn =
                PY_OBJECT_GET_ATTR_STRING(tb_mod, b"format_exception\0".as_ptr() as *const c_char);
            if !fmt_fn.is_null() {
                PY_TRACEBACK_FORMAT_EXCEPTION = fmt_fn;
            }
            PY_DEC_REF(tb_mod);
        }

        if !already_initialized {
            // R21: initialize subinterpreter pool while main GIL is held.
            // Must happen before PY_EVAL_SAVE_THREAD releases the GIL.
            if std::env::var("OLIVE_PY_SUBINTERP").as_deref() == Ok("1") {
                crate::python::python_subinterp::pool_init();
            }

            let save_ptr: *const () = PY_EVAL_SAVE_THREAD as *const ();
            if !save_ptr.is_null() && save_ptr != (noop_save_thread as *const ()) {
                MAIN_THREAD_STATE = PY_EVAL_SAVE_THREAD();
            }
        }
        INITIALIZED.store(true, Ordering::SeqCst);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn olive_py_finalize() {
    unsafe {
        if INITIALIZED.load(Ordering::SeqCst) {
            let restore_ptr: *const () = PY_EVAL_RESTORE_THREAD as *const ();
            if !restore_ptr.is_null()
                && restore_ptr != (noop_restore_thread as *const ())
                && !MAIN_THREAD_STATE.is_null()
            {
                PY_EVAL_RESTORE_THREAD(MAIN_THREAD_STATE);
                // R21: finalize subinterpreter pool while main GIL is held.
                crate::python::python_subinterp::pool_finalize();
                MAIN_THREAD_STATE = std::ptr::null_mut();
            } else {
                let _gil = PY_GILSTATE_ENSURE();
                crate::python::python_subinterp::pool_finalize();
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
