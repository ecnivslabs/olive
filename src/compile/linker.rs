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

/// Static archive name for `olive_std` on this target: `libolive_std.a` almost
/// everywhere `staticlib` outputs land (Unix, macOS, and Windows-GNU), `.lib`
/// only under the MSVC toolchain.
fn static_library_filename() -> String {
    if cfg!(target_env = "msvc") {
        "olive_std.lib".to_string()
    } else {
        "libolive_std.a".to_string()
    }
}

fn find_library_named(lib_name: &str) -> Option<PathBuf> {
    // Prefer the std lib sitting next to the running compiler: in a dev build
    // that is target/<profile>/, which always matches the compiler that just
    // built it, so adding a runtime symbol can never link against a stale copy.
    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        if exe_dir.join(lib_name).exists() {
            return Some(exe_dir.to_path_buf());
        }
        if let Some(parent) = exe_dir.parent() {
            let lib_dir = parent.join("lib");
            if lib_dir.join(lib_name).exists() {
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
        if path.join(lib_name).exists() {
            return Some(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        }
    }
    for dir in &["/usr/local/lib", "/usr/lib", "/lib"] {
        let path = Path::new(dir);
        if path.join(lib_name).exists() {
            return Some(path.to_path_buf());
        }
    }
    None
}

pub fn find_library_dir() -> Option<PathBuf> {
    find_library_named(&libloading::library_filename("olive_std").to_string_lossy())
}

/// Directory containing the static `olive_std` archive, if one has been built.
/// AOT links against this when present so the shipped binary needs no
/// `liblive_std` shared object on disk at runtime; falls back to dynamic
/// linking (`find_library_dir`) for layouts that only have the `.so`/`.dylib`.
pub fn find_static_library_dir() -> Option<PathBuf> {
    find_library_named(&static_library_filename())
}

/// Link arg for an imported native lib: bare stem `m` -> `-lm`, but a full
/// `.so` name (e.g. versioned `libc.so.6`) needs `-l:` exact-name linking,
/// since the stem form would seek the nonexistent `liblibc.so.6.so`.
fn lib_link_arg(name: &str) -> String {
    if name.contains(".so") {
        format!("-l:{}", name)
    } else {
        format!("-l{}", name)
    }
}

pub fn link_object(obj_path: &str, out: &str, native_libs: &[FfiLibInfo]) {
    let static_dir = find_static_library_dir();
    let used_static_link = static_dir.is_some();
    let mut cmd = std::process::Command::new("cc");

    cmd.arg(obj_path);

    if let Some(dir) = static_dir {
        // rustc builds with function/data sections by default, so the linker can
        // drop archive members nothing reaches -- without this flag every object
        // file in libolive_std.a that supplies any referenced symbol comes in
        // whole, dragging in every *other* function bundled into that same
        // codegen unit too (a `print(fib(28))` binary was 85MB before this).
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        cmd.arg("-Wl,--gc-sections");
        #[cfg(target_os = "macos")]
        cmd.arg("-Wl,-dead_strip");

        // Link the archive by exact path: the runtime's own code lands directly in
        // the output binary, no `liblive_std` shared object needed on disk to run it.
        cmd.arg(dir.join(static_library_filename()));
        // `rustc` normally supplies these automatically when it drives the final
        // link; invoking `cc` directly on the archive doesn't, so the transitive
        // system libs Rust's std/deps pull in (libm for f64 intrinsics, threading,
        // dynamic loading, POSIX extensions) need to be named explicitly.
        #[cfg(target_os = "linux")]
        for sys_lib in ["-lm", "-lpthread", "-ldl", "-lrt", "-lutil"] {
            cmd.arg(sys_lib);
        }
        #[cfg(target_os = "macos")]
        for sys_lib in ["-lm", "-lpthread", "-ldl"] {
            cmd.arg(sys_lib);
        }
        // GNU ld (MinGW's `cc`) wants `-l` flags, not bare MSVC `.lib` names.
        #[cfg(all(target_os = "windows", target_env = "msvc"))]
        for sys_lib in ["ws2_32.lib", "userenv.lib", "bcrypt.lib", "ntdll.lib"] {
            cmd.arg(sys_lib);
        }
        #[cfg(all(target_os = "windows", not(target_env = "msvc")))]
        for sys_lib in ["-lws2_32", "-luserenv", "-lbcrypt", "-lntdll"] {
            cmd.arg(sys_lib);
        }
    } else if let Some(ref dir) = find_library_dir() {
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
        // Path refs (`./libfoo.so`, `/opt/lib/bar.so`) link directly, relative
        // ones resolved against cwd; bare names go through the `-l` forms.
        let is_path_ref = path.contains('/') || path.contains('\\');
        if is_path_ref {
            let resolved = if lib_path.is_absolute() {
                lib_path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map(|d| d.join(lib_path))
                    .unwrap_or_else(|_| lib_path.to_path_buf())
            };
            cmd.arg(&resolved);
            if let Some(dir) = resolved.parent() {
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
            cmd.arg(lib_link_arg(path));
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

    if used_static_link {
        strip_debuginfo(out);
    }
}

/// The workspace release profile keeps line-number debuginfo (`debug = 1`) and
/// `strip = true` only covers artifacts Cargo itself produces (the .a is one,
/// this final binary -- built by invoking `cc` directly -- is not). Statically
/// linking pulls the whole reachable slice of libolive_std's debuginfo in too;
/// stripping it after the fact took a `print(fib(20))` binary from 67MB to
/// 349KB with no behavior change. Dynamic-linked output doesn't carry this cost
/// (the .so's debuginfo stays out of the caller's binary), so callers only
/// invoke this after a static link. No-op on platforms without a `strip` tool.
#[cfg(unix)]
fn strip_debuginfo(out: &str) {
    let _ = std::process::Command::new("strip").arg(out).status();
}

#[cfg(not(unix))]
fn strip_debuginfo(_out: &str) {}

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
    fn lib_link_arg_bare_stem() {
        assert_eq!(lib_link_arg("m"), "-lm");
        assert_eq!(lib_link_arg("z"), "-lz");
    }

    #[test]
    fn lib_link_arg_versioned_so() {
        assert_eq!(lib_link_arg("libc.so.6"), "-l:libc.so.6");
        assert_eq!(lib_link_arg("libfoo.so"), "-l:libfoo.so");
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
    fn static_library_filename_matches_platform() {
        let name = static_library_filename();
        if cfg!(target_env = "msvc") {
            assert_eq!(name, "olive_std.lib");
        } else {
            assert_eq!(name, "libolive_std.a");
        }
    }

    #[test]
    fn find_static_library_dir_finds_the_dev_build_archive() {
        // This test binary itself was built alongside libolive_std.a (staticlib
        // crate-type), so the exe-adjacent lookup in find_library_named should
        // locate it without needing any grove/ staging.
        let dir = find_static_library_dir();
        assert!(
            dir.is_some(),
            "expected to find libolive_std.a next to the test binary"
        );
        assert!(dir.unwrap().join(static_library_filename()).exists());
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
