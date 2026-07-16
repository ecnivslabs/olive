use std::ffi::CString;
use std::os::raw::c_void;

pub unsafe fn compat_dlopen_current_process() -> *mut c_void {
    unsafe {
        #[cfg(target_os = "windows")]
        {
            // On Windows, use GetModuleHandle(NULL) for the current process
            extern "system" {
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
