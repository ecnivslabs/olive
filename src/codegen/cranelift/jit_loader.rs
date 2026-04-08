use cranelift_jit::JITBuilder;

use super::{ASYNC_RUNTIME_SYMS, SYMBOL_MAP};

#[cfg(target_family = "unix")]
unsafe extern "C" {
    fn dlsym(
        handle: *mut std::ffi::c_void,
        symbol: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
}

pub(super) fn register_runtime_symbols(
    builder: &mut JITBuilder,
    needed: &std::collections::HashSet<&str>,
    has_async: bool,
    has_c_structs: bool,
) -> Option<libloading::Library> {
    let lib_name = libloading::library_filename("olive_std");
    let mut paths = Vec::new();

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let base = std::path::PathBuf::from(manifest_dir);
        let mut cur = Some(base.as_path());
        while let Some(p) = cur {
            paths.push(p.join("target/release").join(&lib_name));
            paths.push(p.join("target/debug").join(&lib_name));
            cur = p.parent();
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut cur = Some(cwd.as_path());
        while let Some(p) = cur {
            paths.push(p.join("target/release").join(&lib_name));
            paths.push(p.join("target/debug").join(&lib_name));
            cur = p.parent();
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        let mut cur = Some(exe_path.as_path());
        while let Some(p) = cur {
            paths.push(p.join(&lib_name));
            paths.push(p.join("deps").join(&lib_name));
            if let Some(parent) = p.parent() {
                paths.push(parent.join(&lib_name));
                paths.push(parent.join("lib").join(&lib_name));
            }
            cur = p.parent();
        }
    }

    paths.push(std::path::PathBuf::from("target/release").join(&lib_name));
    paths.push(std::path::PathBuf::from("target/debug").join(&lib_name));
    paths.push(std::path::PathBuf::from("/usr/local/lib").join(&lib_name));
    paths.push(std::path::PathBuf::from("/usr/lib").join(&lib_name));
    paths.push(std::path::PathBuf::from("/lib").join(&lib_name));

    let mut unique_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        let key = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        if seen.insert(key.clone()) {
            unique_paths.push(key);
        }
    }

    let mut loaded_lib = None;
    for path in &unique_paths {
        if path.exists() {
            if let Ok(l) = unsafe { libloading::Library::new(path) } {
                loaded_lib = Some(l);
                break;
            }
        }
    }

    unsafe {
        for &(jit_name, c_name) in SYMBOL_MAP {
            let is_async_needed = has_async && ASYNC_RUNTIME_SYMS.contains(&jit_name);
            let needed_for_c = (jit_name == "__olive_alloc" || jit_name == "__olive_free_c_struct")
                && has_c_structs;
            if !needed.contains(jit_name) && !is_async_needed && !needed_for_c {
                continue;
            }

            #[cfg(target_family = "unix")]
            let ptr = {
                let p = dlsym(std::ptr::null_mut(), c_name.as_ptr() as *const _);
                if p.is_null() {
                    loaded_lib
                        .as_ref()
                        .and_then(|lib| lib.get::<unsafe extern "C" fn()>(c_name).ok())
                        .map(|f| *f as *mut std::ffi::c_void)
                        .unwrap_or(std::ptr::null_mut())
                } else {
                    p
                }
            };

            #[cfg(not(target_family = "unix"))]
            let ptr = loaded_lib
                .as_ref()
                .and_then(|lib| lib.get::<unsafe extern "C" fn()>(c_name).ok())
                .map(|f| *f as *mut std::ffi::c_void)
                .unwrap_or(std::ptr::null_mut());

            if !ptr.is_null() {
                builder.symbol(jit_name, ptr as *const u8);
            } else {
                eprintln!(
                    "warning: could not resolve runtime symbol '{}' (c_name: '{}')",
                    jit_name,
                    String::from_utf8_lossy(c_name)
                );
            }
        }
    }

    loaded_lib
}
