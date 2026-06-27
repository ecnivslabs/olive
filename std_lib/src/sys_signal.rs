use crate::python::python_bindings::{
    PY_GILSTATE_ENSURE, PY_GILSTATE_RELEASE, PY_RUN_SIMPLE_STRING,
};
use crate::string::olive_str_from_ptr;
use std::ffi::CString;

#[unsafe(no_mangle)]
pub extern "C" fn olive_signal_install_sigint(msg_ptr: i64) -> i64 {
    let msg = if msg_ptr == 0 {
        String::new()
    } else {
        olive_str_from_ptr(msg_ptr)
    };
    let escaped = msg
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    // os.write(fd, bytes) is async-signal-safe; print/sys.stdout go through
    // BufferedWriter which raises RuntimeError on reentrant calls (e.g. Ctrl+C
    // fired while the main loop is mid-write).
    let code = format!(
        "import signal as _s, os as _o\ndef __sigint_h(_sig, _frame):\n    _o.write(1, b'\\n{escaped}\\n')\n    _o._exit(0)\n_s.signal(_s.SIGINT, __sigint_h)\n"
    );
    if let Ok(cstr) = CString::new(code) {
        let gstate = unsafe { PY_GILSTATE_ENSURE() };
        unsafe { PY_RUN_SIMPLE_STRING(cstr.as_ptr()) };
        unsafe { PY_GILSTATE_RELEASE(gstate) };
    }
    0
}
