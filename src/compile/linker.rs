use rustc_hash::FxHasher;
use std::{
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process,
};

pub fn exec_binary(path: &str) -> i32 {
    std::process::Command::new(path)
        .status()
        .map(|s| s.code().unwrap_or(1))
        .unwrap_or(1)
}

pub fn compute_source_hash(files: &[String]) -> u64 {
    let mut sorted = files.to_vec();
    sorted.sort();
    let mut hasher = FxHasher::default();
    for path in &sorted {
        path.hash(&mut hasher);
        if let Ok(content) = fs::read(path) {
            content.hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub fn find_library_dir() -> Option<PathBuf> {
    let lib_name = libloading::library_filename("olive_std");
    for dir in &["grove/release", "grove/debug"] {
        let path = Path::new(dir);
        if path.join(&lib_name).exists() {
            return Some(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        }
    }
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        if exe_dir.join(&lib_name).exists() {
            return Some(exe_dir.to_path_buf());
        }
        if let Some(parent) = exe_dir.parent() {
            let lib_dir = parent.join("lib");
            if lib_dir.join(&lib_name).exists() {
                return Some(lib_dir);
            }
        }
    }
    for dir in &["/usr/local/lib", "/usr/lib", "/lib"] {
        let path = Path::new(dir);
        if path.join(&lib_name).exists() {
            return Some(path.to_path_buf());
        }
    }
    None
}

pub fn link_object(
    obj_path: &str,
    out: &str,
    native_libs: &[(
        String,
        String,
        Vec<crate::parser::ast::FfiFnSig>,
        Vec<crate::parser::ast::FfiStructDef>,
        Vec<crate::parser::ast::FfiVarDef>,
    )],
) {
    let lib_dir = find_library_dir();
    let mut cmd = std::process::Command::new("cc");

    cmd.arg(obj_path);

    if let Some(ref dir) = lib_dir {
        cmd.arg("-L");
        cmd.arg(dir);
        cmd.arg("-lolive_std");
        #[cfg(not(target_os = "windows"))]
        cmd.arg(format!("-Wl,-rpath,{}", dir.display()));
    } else {
        cmd.arg("-lolive_std");
    }

    for (_, path, _, _, _) in native_libs {
        let lib_path = Path::new(path.as_str());
        if lib_path.is_absolute() && lib_path.exists() {
            cmd.arg(path);
            if let Some(dir) = lib_path.parent() {
                let standard = matches!(
                    dir.to_str().unwrap_or(""),
                    "/lib" | "/usr/lib" | "/usr/local/lib"
                );
                if !standard {
                    #[cfg(not(target_os = "windows"))]
                    cmd.arg(format!("-Wl,-rpath,{}", dir.display()));
                }
            }
        } else {
            cmd.arg(format!("-l{}", path));
        }
    }

    cmd.arg("-o");
    cmd.arg(out);

    let status = cmd.status().unwrap_or_else(|e| {
        eprintln!("error: could not invoke cc: {e}");
        process::exit(1);
    });

    fs::remove_file(obj_path).ok();

    if !status.success() {
        eprintln!("error: linking failed");
        process::exit(1);
    }
}

pub fn ensure_dir(path: &str) {
    fs::create_dir_all(path).unwrap_or_else(|e| {
        eprintln!("error: could not create directory {path}: {e}");
        process::exit(1);
    });
}

pub fn collect_native_libs(
    program: &crate::parser::Program,
) -> Vec<(
    String,
    String,
    Vec<crate::parser::ast::FfiFnSig>,
    Vec<crate::parser::ast::FfiStructDef>,
    Vec<crate::parser::ast::FfiVarDef>,
)> {
    program
        .stmts
        .iter()
        .filter_map(|s| {
            if let crate::parser::StmtKind::NativeImport {
                path,
                alias,
                functions,
                structs,
                vars,
                ..
            } = &s.kind
            {
                Some((
                    alias.clone(),
                    path.clone(),
                    functions.clone(),
                    structs.clone(),
                    vars.clone(),
                ))
            } else {
                None
            }
        })
        .collect()
}
