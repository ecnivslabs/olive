use std::ffi::CString;
use std::os::raw::c_void;

pub unsafe fn compat_dlopen_current_process() -> *mut c_void {
    unsafe {
        #[cfg(target_os = "windows")]
        {
            // On Windows, use GetModuleHandle(NULL) for the current process
            unsafe extern "system" {
                fn GetModuleHandleA(lpModuleName: *const u8) -> *mut c_void;
            }
            GetModuleHandleA(std::ptr::null())
        }
        #[cfg(not(target_os = "windows"))]
        {
            libc::dlopen(std::ptr::null(), libc::RTLD_NOW | libc::RTLD_LOCAL)
        }
    }
}

pub unsafe fn compat_dlopen(name: &str) -> *mut c_void {
    unsafe {
        let cname = CString::new(name).unwrap();
        #[cfg(target_os = "windows")]
        {
            super::LoadLibraryA(cname.as_ptr() as *const u8)
        }
        #[cfg(not(target_os = "windows"))]
        {
            libc::dlopen(cname.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL)
        }
    }
}

/// Last dynamic-loader error, for diagnostics when every load attempt fails.
/// Unix reports this via `dlerror()`, not `errno`; Windows via the
/// thread-local Win32 last-error code set by `LoadLibraryA`.
pub unsafe fn compat_dl_error() -> String {
    #[cfg(target_os = "windows")]
    {
        std::io::Error::last_os_error().to_string()
    }
    #[cfg(not(target_os = "windows"))]
    unsafe {
        let msg = libc::dlerror();
        if msg.is_null() {
            "unknown error".to_string()
        } else {
            std::ffi::CStr::from_ptr(msg).to_string_lossy().into_owned()
        }
    }
}

pub unsafe fn compat_dlsym<T>(handle: *mut c_void, name: &str) -> T {
    unsafe {
        let cname = CString::new(name).unwrap();
        #[cfg(target_os = "windows")]
        let sym = { super::GetProcAddress(handle, cname.as_ptr() as *const u8) };
        #[cfg(not(target_os = "windows"))]
        let sym = { libc::dlsym(handle, cname.as_ptr()) };
        std::mem::transmute_copy(&sym)
    }
}
