use rustc_hash::FxHasher;
use std::{
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process,
};

pub type FfiLibInfo = (
    String,
    String,
    Vec<crate::parser::ast::FfiFnSig>,
    Vec<crate::parser::ast::FfiStructDef>,
    Vec<crate::parser::ast::FfiVarDef>,
);

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
        if let Ok(meta) = fs::metadata(path)
            && let Ok(mtime) = meta.modified()
        {
            mtime.hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub fn find_library_dir() -> Option<PathBuf> {
    let lib_name = libloading::library_filename("olive_std");
    // Prefer the std lib sitting next to the running compiler: in a dev build
    // that is target/<profile>/, which always matches the compiler that just
    // built it — so adding a runtime symbol can never link against a stale copy.
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
    // Installed layouts: grove/<profile> matching this binary's profile, then
    // the system library directories.
    let grove_dirs: &[&str] = if cfg!(debug_assertions) {
        &["grove/debug", "grove/release"]
    } else {
        &["grove/release", "grove/debug"]
    };
    for dir in grove_dirs {
        let path = Path::new(dir);
        if path.join(&lib_name).exists() {
            return Some(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
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

pub fn link_object(obj_path: &str, out: &str, native_libs: &[FfiLibInfo]) {
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

pub fn collect_native_libs(program: &crate::parser::Program) -> Vec<FfiLibInfo> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::span::Span;

    #[test]
    fn compute_source_hash_deterministic() {
        let files = vec!["a.liv".to_string(), "b.liv".to_string()];
        let h1 = compute_source_hash(&files);
        let h2 = compute_source_hash(&files);
        assert_eq!(h1, h2);
    }

    #[test]
    fn compute_source_hash_differs_for_different_inputs() {
        let a = vec!["x.liv".to_string()];
        let b = vec!["y.liv".to_string()];
        assert_ne!(compute_source_hash(&a), compute_source_hash(&b));
    }

    #[test]
    fn compute_source_hash_sorted() {
        let a = vec!["b.liv".to_string(), "a.liv".to_string()];
        let b = vec!["a.liv".to_string(), "b.liv".to_string()];
        assert_eq!(compute_source_hash(&a), compute_source_hash(&b));
    }

    #[test]
    fn ensure_dir_creates_directory() {
        let dir = std::env::temp_dir().join("olive_test_ensure_dir");
        let path = dir.to_str().unwrap().to_string();
        ensure_dir(&path);
        assert!(dir.exists());
        assert!(dir.is_dir());
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn ensure_dir_creates_nested() {
        let dir = std::env::temp_dir().join("olive_test_ensure_nested/a/b/c");
        let path = dir.to_str().unwrap().to_string();
        ensure_dir(&path);
        assert!(dir.exists());
        std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn exec_binary_true() {
        assert_eq!(exec_binary("true"), 0);
    }

    #[test]
    fn exec_binary_false() {
        assert_eq!(exec_binary("false"), 1);
    }

    #[test]
    fn exec_binary_nonexistent() {
        assert_eq!(exec_binary("nonexistent_command_xyz_123"), 1);
    }

    #[test]
    fn collect_native_libs_empty() {
        let program = parser::Program { stmts: vec![] };
        assert!(collect_native_libs(&program).is_empty());
    }

    #[test]
    fn collect_native_libs_single() {
        let program = parser::Program {
            stmts: vec![parser::Stmt {
                kind: parser::StmtKind::NativeImport {
                    path: "/usr/lib/libfoo.so".to_string(),
                    alias: "foo".to_string(),
                    functions: vec![],
                    structs: vec![],
                    vars: vec![],
                    consts: vec![],
                    block_safe: false,
                },
                span: Span {
                    file_id: 0,
                    line: 0,
                    col: 0,
                    start: 0,
                    end: 0,
                },
            }],
        };
        let libs = collect_native_libs(&program);
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].0, "foo");
        assert_eq!(libs[0].1, "/usr/lib/libfoo.so");
    }

    #[test]
    fn collect_native_libs_multiple() {
        let program = parser::Program {
            stmts: vec![
                parser::Stmt {
                    kind: parser::StmtKind::NativeImport {
                        path: "libz".to_string(),
                        alias: "z".to_string(),
                        functions: vec![],
                        structs: vec![],
                        vars: vec![],
                        consts: vec![],
                        block_safe: false,
                    },
                    span: Span {
                        file_id: 0,
                        line: 0,
                        col: 0,
                        start: 0,
                        end: 0,
                    },
                },
                parser::Stmt {
                    kind: parser::StmtKind::NativeImport {
                        path: "libpng".to_string(),
                        alias: "png".to_string(),
                        functions: vec![],
                        structs: vec![],
                        vars: vec![],
                        consts: vec![],
                        block_safe: true,
                    },
                    span: Span {
                        file_id: 0,
                        line: 0,
                        col: 0,
                        start: 0,
                        end: 0,
                    },
                },
            ],
        };
        let libs = collect_native_libs(&program);
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0].0, "z");
        assert_eq!(libs[1].0, "png");
    }

    #[test]
    fn collect_native_libs_skips_non_native() {
        let program = parser::Program {
            stmts: vec![
                parser::Stmt {
                    kind: parser::StmtKind::Pass,
                    span: Span {
                        file_id: 0,
                        line: 0,
                        col: 0,
                        start: 0,
                        end: 0,
                    },
                },
                parser::Stmt {
                    kind: parser::StmtKind::NativeImport {
                        path: "libfoo".to_string(),
                        alias: "foo".to_string(),
                        functions: vec![],
                        structs: vec![],
                        vars: vec![],
                        consts: vec![],
                        block_safe: false,
                    },
                    span: Span {
                        file_id: 0,
                        line: 0,
                        col: 0,
                        start: 0,
                        end: 0,
                    },
                },
            ],
        };
        let libs = collect_native_libs(&program);
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].0, "foo");
    }
}
