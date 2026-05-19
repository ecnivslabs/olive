use crate::*;
use std::ffi::CString;
use std::os::raw::c_void;

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
