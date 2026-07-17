use cranelift_jit::{JITBuilder, JITModule};

use super::{ASYNC_RUNTIME_SYMS, CraneliftCodegen, SYMBOL_MAP};

impl CraneliftCodegen<JITModule> {
    /// Resolves a runtime symbol by name for the debugger (fault-stop
    /// reporting, value-formatting calls): `dlsym(RTLD_DEFAULT)` first, then
    /// the libraries retained in `_libs`, because libloading opens olive_std
    /// `RTLD_LOCAL` and bare `RTLD_DEFAULT` can miss it.
    pub fn runtime_symbol(&self, name: &str) -> Option<*const u8> {
        let c_name = std::ffi::CString::new(name).ok()?;
        #[cfg(target_family = "unix")]
        {
            let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c_name.as_ptr()) };
            if !p.is_null() {
                return Some(p as *const u8);
            }
        }
        for lib in &self._libs {
            if let Ok(sym) =
                unsafe { lib.get::<unsafe extern "C" fn()>(c_name.as_bytes_with_nul()) }
            {
                return Some(*sym as *const u8);
            }
        }
        None
    }
}

pub(super) fn register_runtime_symbols(
    builder: &mut JITBuilder,
    needed: &std::collections::HashSet<&str>,
    has_async: bool,
    has_c_structs: bool,
) -> Option<libloading::Library> {
    let lib_name = libloading::library_filename("olive_std");
    let mut paths = Vec::new();

    // Search the profile that matches this compiler build first: a debug test
    // run must not pick up a stale `target/release` runtime (and vice versa), or
    // the JIT links against an out-of-date `libolive_std` and silently
    // mis-handles values whose ABI changed (e.g. a boxed-scalar kind the old
    // copy doesn't know).
    let target_dirs: [&str; 2] = if cfg!(debug_assertions) {
        ["target/debug", "target/release"]
    } else {
        ["target/release", "target/debug"]
    };

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let base = std::path::PathBuf::from(manifest_dir);
        let mut cur = Some(base.as_path());
        while let Some(p) = cur {
            for dir in target_dirs {
                paths.push(p.join(dir).join(&lib_name));
            }
            cur = p.parent();
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut cur = Some(cwd.as_path());
        while let Some(p) = cur {
            for dir in target_dirs {
                paths.push(p.join(dir).join(&lib_name));
            }
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

    for dir in target_dirs {
        paths.push(std::path::PathBuf::from(dir).join(&lib_name));
    }
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
        if path.exists()
            && let Ok(l) = unsafe { libloading::Library::new(path) }
        {
            loaded_lib = Some(l);
            break;
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
                let p = libc::dlsym(libc::RTLD_DEFAULT, c_name.as_ptr() as *const _);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_map_contains_essential_symbols() {
        let names: std::collections::HashSet<&str> = SYMBOL_MAP.iter().map(|(n, _)| *n).collect();
        assert!(names.contains("__olive_alloc"));
        assert!(names.contains("__olive_panic"));
        assert!(names.contains("__olive_free"));
    }

    #[test]
    fn test_symbol_map_names_are_null_terminated() {
        for (_, bytes) in SYMBOL_MAP {
            assert!(
                bytes.ends_with(b"\0"),
                "symbol {:?} not null-terminated",
                bytes
            );
        }
    }

    #[test]
    fn test_async_runtime_syms_contain_core_symbols() {
        assert!(ASYNC_RUNTIME_SYMS.contains(&"__olive_make_future"));
        assert!(ASYNC_RUNTIME_SYMS.contains(&"__olive_await"));
        assert!(ASYNC_RUNTIME_SYMS.contains(&"__olive_spawn_task"));
        assert!(ASYNC_RUNTIME_SYMS.contains(&"__olive_sm_poll"));
    }

    #[test]
    fn test_register_runtime_symbols_does_not_panic() {
        use cranelift::prelude::settings::Configurable;
        let mut flag_builder = cranelift::prelude::settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        let isa_builder = cranelift_native::builder().unwrap();
        let isa = isa_builder
            .finish(cranelift::prelude::settings::Flags::new(flag_builder))
            .unwrap();
        let mut builder =
            cranelift_jit::JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let needed = std::collections::HashSet::new();
        let _ = register_runtime_symbols(&mut builder, &needed, false, false);
    }
}
