use rustc_hash::FxHasher;
use std::{
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    process,
};

/// All information the linker and JIT need about one native import.
#[derive(Clone)]
pub struct NativeLibRef {
    pub alias: String,
    /// Resolved path as rewritten by `loader::load_and_parse_collecting`.
    /// Absolute for pod-native libs; may be bare-name or path-ref otherwise.
    pub path: String,
    /// True when `path` was resolved by `loader::resolve_pod_native`, meaning
    /// it lives under a pod's `native/` directory. The linker stages it next to
    /// the output and emits an `$ORIGIN`-relative rpath so the output binary is
    /// relocatable. The JIT turns a failed dlopen into a hard error.
    pub from_pod: bool,
    pub functions: Vec<crate::parser::ast::FfiFnSig>,
    pub structs: Vec<crate::parser::ast::FfiStructDef>,
    pub vars: Vec<crate::parser::ast::FfiVarDef>,
}

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
        // Test binaries and some cargo layouts live in `deps/`, which also
        // holds the std lib artifacts.
        let deps_dir = exe_dir.join("deps");
        if deps_dir.join(lib_name).exists() {
            return Some(deps_dir);
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
    system_library_dirs()
        .into_iter()
        .find(|dir| dir.join(lib_name).exists())
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

/// Standard system library directories, plus Homebrew's prefix on macOS
/// (SIP locks `/usr/lib` down to Apple-shipped dylibs, so third-party and
/// even some open-source libs land under Homebrew instead).
fn system_library_dirs() -> Vec<PathBuf> {
    let dirs = vec![
        PathBuf::from("/usr/local/lib"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/lib"),
    ];
    #[cfg(target_os = "macos")]
    let dirs = {
        let mut dirs = dirs;
        if let Ok(output) = process::Command::new("brew").arg("--prefix").output()
            && output.status.success()
        {
            let prefix = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !prefix.is_empty() {
                dirs.push(PathBuf::from(prefix).join("lib"));
            }
        }
        dirs.push(PathBuf::from("/opt/homebrew/lib"));
        dirs
    };
    dirs
}

fn is_standard_lib_dir(dir: &Path) -> bool {
    matches!(
        dir.to_str().unwrap_or(""),
        "/lib" | "/usr/lib" | "/usr/local/lib"
    )
}

/// The architecture tag `ldconfig -p` prints for the host, e.g. `x86-64` for
/// x86_64 or `AArch64` for aarch64. An entry whose tag doesn't match the host
/// is for a different word size or architecture and would fail to link.
#[cfg(target_os = "linux")]
fn ldconfig_arch_tag() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86-64"
    } else if cfg!(target_arch = "aarch64") {
        "AArch64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        ""
    }
}

/// Matches one line of `ldconfig -p` output against `name`, e.g.:
///   libzstd.so.1 (libc6,x86-64) => /usr/lib/libzstd.so.1
/// The soname sits before the parenthesized ABI/arch tag, which an earlier
/// version of this function left attached to the soname it compared against
/// `name`, so it never matched. An entry whose tag doesn't list `want_arch`
/// is for a different word size or architecture and is skipped, since linking
/// against it would fail anyway.
fn match_ldconfig_line(line: &str, name: &str, want_arch: &str) -> Option<PathBuf> {
    let (lhs, rhs) = line.trim().split_once(" => ")?;
    let (soname, tag) = match lhs.split_once(" (") {
        Some((soname, tag)) => (soname.trim(), tag.trim_end_matches(')')),
        None => (lhs.trim(), ""),
    };
    if soname != name {
        return None;
    }
    if !want_arch.is_empty() && !tag.is_empty() && !tag.split(',').any(|f| f.trim() == want_arch) {
        return None;
    }
    Path::new(rhs.trim()).parent().map(|d| d.to_path_buf())
}

/// Resolves a bare library filename (not a stem) to the directory containing
/// it, so an exact versioned name (`libc.so.6`) links without needing a
/// `-dev` symlink installed. Linux additionally consults `ldconfig`'s cache,
/// the authoritative soname -> path map, which covers multiarch subdirectories
/// `system_library_dirs` doesn't scan directly.
fn resolve_exact_library(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = process::Command::new("ldconfig").arg("-p").output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            let want_arch = ldconfig_arch_tag();
            for line in text.lines() {
                if let Some(dir) = match_ldconfig_line(line, name, want_arch) {
                    return Some(dir);
                }
            }
        }
    }
    system_library_dirs()
        .into_iter()
        .find(|dir| dir.join(name).exists())
}

/// Strips a `lib` prefix and a shared-library suffix (`.so[.N...]`,
/// `.dylib`, `.dll`) to recover the stem `-l` expects, e.g. `libm.so` -> `m`.
fn library_stem(name: &str) -> Option<&str> {
    let base = name.strip_prefix("lib").unwrap_or(name);
    [".so", ".dylib", ".dll"]
        .iter()
        .find_map(|ext| base.find(ext).map(|i| &base[..i]))
}

/// Link arg for a bare (no path separator) imported native lib name whose
/// exact file couldn't be located: falls back to plain `-l<stem>` linking,
/// which every platform's linker resolves via its own default search path.
fn lib_link_arg(name: &str) -> String {
    match library_stem(name) {
        Some(stem) if !stem.is_empty() => format!("-l{stem}"),
        _ => format!("-l{name}"),
    }
}

pub fn link_object(obj_path: &str, out: &str, native_libs: &[NativeLibRef]) {
    link_object_impl(obj_path, out, native_libs, false, None)
}

/// Link a Python extension module. `module_name` selects the `PyInit_<name>`
/// entry point MSVC must export: unlike GNU ld, `link.exe /DLL` exports
/// nothing unless a symbol is marked `dllexport` or named with `/EXPORT`,
/// and `/OPT:REF` would otherwise discard the init function as unreferenced.
pub fn link_shared_object(
    obj_path: &str,
    out: &str,
    native_libs: &[NativeLibRef],
    module_name: Option<&str>,
) {
    let export = module_name.map(|name| format!("PyInit_{name}"));
    link_object_impl(obj_path, out, native_libs, true, export.as_deref())
}

fn is_gnu_link(cmd_name: &str) -> bool {
    if let Ok(output) = std::process::Command::new(cmd_name)
        .arg("--version")
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.contains("GNU") || text.contains("coreutils") {
            return true;
        }
        let text_err = String::from_utf8_lossy(&output.stderr);
        if text_err.contains("GNU") || text_err.contains("coreutils") {
            return true;
        }
    }
    false
}

fn which_exists(cmd_name: &str) -> bool {
    std::process::Command::new(cmd_name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn get_msvc_linker_cmd() -> (std::process::Command, bool) {
    #[cfg(windows)]
    {
        let target = if cfg!(target_arch = "x86_64") {
            "x86_64-pc-windows-msvc"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64-pc-windows-msvc"
        } else {
            "i686-pc-windows-msvc"
        };
        if let Some(tool) = cc::windows_registry::find_tool(target, "link.exe") {
            return (tool.to_command(), true);
        }
    }
    if is_gnu_link("link.exe") {
        if which_exists("lld-link.exe") || which_exists("lld-link") {
            return (std::process::Command::new("lld-link"), true);
        }
        if which_exists("gcc.exe") || which_exists("gcc") {
            return (std::process::Command::new("gcc"), false);
        }
        if which_exists("clang.exe") || which_exists("clang") {
            return (std::process::Command::new("clang"), false);
        }
    }
    (std::process::Command::new("link.exe"), true)
}

fn link_object_impl(
    obj_path: &str,
    out: &str,
    native_libs: &[NativeLibRef],
    shared: bool,
    shared_export: Option<&str>,
) {
    let static_dir = find_static_library_dir();
    let used_static_link = static_dir.is_some();
    let is_msvc_env = cfg!(target_env = "msvc");

    let (mut cmd, is_msvc) = if is_msvc_env {
        let (mut c, is_msvc_style) = get_msvc_linker_cmd();
        if is_msvc_style {
            c.arg("/NOLOGO");
            c.arg(format!("/OUT:{out}"));
            c.arg(obj_path);

            if shared {
                c.arg("/DLL");
                if let Some(export) = shared_export {
                    c.arg(format!("/EXPORT:{export}"));
                }
            }

            if let Some(dir) = static_dir {
                c.arg("/OPT:REF");
                c.arg("/OPT:ICF");
                c.arg(dir.join(static_library_filename()));
                for sys_lib in [
                    "ws2_32.lib",
                    "userenv.lib",
                    "bcrypt.lib",
                    "ntdll.lib",
                    "advapi32.lib",
                    "iphlpapi.lib",
                ] {
                    c.arg(sys_lib);
                }
            } else if let Some(ref dir) = find_library_dir() {
                c.arg(format!("/LIBPATH:{}", dir.display()));
                c.arg("olive_std.lib");
            } else {
                c.arg("olive_std.lib");
            }
            (c, true)
        } else {
            c.arg(obj_path);
            if shared {
                c.arg("-shared");
            }
            if let Some(dir) = static_dir {
                c.arg("-Wl,--gc-sections");
                c.arg(dir.join(static_library_filename()));
                for sys_lib in ["-lws2_32", "-luserenv", "-lbcrypt", "-lntdll"] {
                    c.arg(sys_lib);
                }
            } else if let Some(ref dir) = find_library_dir() {
                c.arg("-L");
                c.arg(dir);
                c.arg("-lolive_std");
            } else {
                c.arg("-lolive_std");
            }
            (c, false)
        }
    } else {
        let mut c = std::process::Command::new("cc");

        c.arg(obj_path);

        if shared {
            c.arg("-shared");
        }

        if let Some(dir) = static_dir {
            // rustc builds with function/data sections by default, so the linker can
            // drop archive members nothing reaches -- without this flag every object
            // file in libolive_std.a that supplies any referenced symbol comes in
            // whole, dragging in every *other* function bundled into that same
            // codegen unit too (a `print(fib(28))` binary was 85MB before this).
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            c.arg("-Wl,--gc-sections");
            #[cfg(target_os = "macos")]
            c.arg("-Wl,-dead_strip");

            // Link the archive by exact path: the runtime's own code lands directly in
            // the output binary, no `liblive_std` shared object needed on disk to run it.
            c.arg(dir.join(static_library_filename()));
            // `rustc` normally supplies these automatically when it drives the final
            // link; invoking `cc` directly on the archive doesn't, so the transitive
            // system libs Rust's std/deps pull in (libm for f64 intrinsics, threading,
            // dynamic loading, POSIX extensions) need to be named explicitly.
            #[cfg(target_os = "linux")]
            for sys_lib in [
                "-lm",
                "-lpthread",
                "-ldl",
                "-lrt",
                "-lutil",
                "-lcrypto",
                "-lssl",
            ] {
                c.arg(sys_lib);
            }
            // `tungstenite`'s native-tls backend pulls Security and
            // CoreFoundation symbols out of the static archive; `rustc`
            // supplies these frameworks automatically when it drives the
            // link, so name them explicitly here too.
            #[cfg(target_os = "macos")]
            for sys_lib in [
                "-lm",
                "-lpthread",
                "-ldl",
                "-framework",
                "CoreFoundation",
                "-framework",
                "Security",
            ] {
                c.arg(sys_lib);
            }
            // `cc` is never MSVC's link.exe here (MSVC ships no binary named `cc`) --
            // it's always MinGW's gcc/ld, even when rustc itself targets MSVC. GNU ld
            // wants `-l` flags, not bare MSVC `.lib` names.
            #[cfg(target_os = "windows")]
            for sys_lib in ["-lws2_32", "-luserenv", "-lbcrypt", "-lntdll"] {
                c.arg(sys_lib);
            }
        } else if let Some(ref dir) = find_library_dir() {
            c.arg("-L");
            c.arg(dir);
            c.arg("-lolive_std");
            #[cfg(not(target_os = "windows"))]
            c.arg(format!("-Wl,-rpath,{}", dir.display()));
        } else {
            c.arg("-lolive_std");
        }
        (c, false)
    };

    let out_dir = Path::new(out).parent().unwrap_or(Path::new("."));
    let mut unresolved_fallbacks: Vec<String> = Vec::new();

    for lib in native_libs {
        let path = &lib.path;
        let lib_path = Path::new(path.as_str());
        let is_path_ref = path.contains('/') || path.contains('\\');

        if lib.from_pod {
            // Pod-native: stage beside the output so the binary is relocatable,
            // then use $ORIGIN / @loader_path so it finds the copy at runtime.
            match stage_beside_output(lib_path, out_dir) {
                Ok(staged) => {
                    if is_msvc {
                        let implib = staged.with_extension("dll.lib");
                        if !implib.exists() {
                            eprintln!(
                                "error: cannot link native library '{}' with the MSVC toolchain",
                                lib.alias
                            );
                            eprintln!(
                                "  the DLL is at {} but its import library is missing",
                                staged.display()
                            );
                            eprintln!("  expected: {}", implib.display());
                            eprintln!(
                                "  reinstall the pod with `pit update {}`; if that does not help,",
                                lib.alias
                            );
                            eprintln!(
                                "  the pod did not publish an import library for windows-x86_64"
                            );
                            process::exit(1);
                        }
                        cmd.arg(&implib);
                    } else {
                        let filename = staged
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let out_abs = if out_dir.is_absolute() {
                            out_dir.to_path_buf()
                        } else {
                            std::env::current_dir()
                                .map(|d| d.join(out_dir))
                                .unwrap_or_else(|_| out_dir.to_path_buf())
                        };
                        cmd.arg(format!("-L{}", out_abs.display()));
                        if cfg!(target_os = "macos") {
                            cmd.arg(lib_link_arg(&filename));
                            cmd.arg("-Wl,-rpath,@loader_path");
                        } else {
                            cmd.arg(format!("-l:{filename}"));
                            cmd.arg("-Wl,-rpath,$ORIGIN");
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "error: could not stage native library '{}': {e}",
                        lib_path.display()
                    );
                    process::exit(1);
                }
            }
        } else if is_path_ref {
            // Explicit path ref (./libfoo.so or /opt/lib/bar.so).
            let resolved = if lib_path.is_absolute() {
                lib_path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map(|d| d.join(lib_path))
                    .unwrap_or_else(|_| lib_path.to_path_buf())
            };
            cmd.arg(&resolved);
            if let Some(dir) = resolved.parent()
                && !is_standard_lib_dir(dir)
                && !is_msvc
            {
                #[cfg(not(target_os = "windows"))]
                cmd.arg(format!("-Wl,-rpath,{}", dir.display()));
            }
        } else if let Some(dir) = resolve_exact_library(path) {
            cmd.arg(dir.join(path));
            if !is_standard_lib_dir(&dir) && !is_msvc {
                #[cfg(not(target_os = "windows"))]
                cmd.arg(format!("-Wl,-rpath,{}", dir.display()));
            }
        } else if is_msvc {
            cmd.arg(path);
        } else if cfg!(target_os = "macos") {
            // ld64 has no equivalent to GNU ld's `-l:exact-name` linking; the
            // stem fallback is the only portable option left once the file
            // search above comes up empty.
            unresolved_fallbacks.push(path.clone());
            cmd.arg(lib_link_arg(path));
        } else if path.contains(".so") {
            // GNU ld / MinGW ld's exact-name linking as a last resort for a
            // versioned soname neither `ldconfig` nor the search dirs found.
            unresolved_fallbacks.push(path.clone());
            cmd.arg(format!("-l:{path}"));
        } else {
            unresolved_fallbacks.push(path.clone());
            cmd.arg(lib_link_arg(path));
        }
    }

    if !is_msvc {
        cmd.arg("-o");
        cmd.arg(out);
    }

    let status = cmd.status().unwrap_or_else(|e| {
        let name = if is_msvc { "link.exe" } else { "cc" };
        eprintln!("error: could not invoke {name}: {e}");
        process::exit(1);
    });

    fs::remove_file(obj_path).ok();

    if !status.success() {
        for name in &unresolved_fallbacks {
            eprintln!(
                "error: native library '{}' was not found on this system",
                name
            );
            eprintln!(
                "  the linker fell back to `-l:{name}` / `-l<stem>` and could not resolve it"
            );
            eprintln!(
                "  if this library comes from a pod, that pod may not publish a native artifact"
            );
            eprintln!("  for this platform; check with `pit search <podname>`");
        }
        eprintln!("error: linking failed");
        eprintln!("  linker command: {:?}", cmd);
        process::exit(1);
    }

    if used_static_link && std::env::var_os("OLIVE_KEEP_DEBUGINFO").is_none() {
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

/// Copies `src` into `out_dir`, preserving the filename. The copy is
/// content-hash gated: if the destination already has the same bytes, the
/// file is left untouched (mtime unchanged, AOT cache stays valid). Returns
/// the destination path so the linker can reference it.
fn stage_beside_output(src: &Path, out_dir: &Path) -> io::Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no filename"))?;
    let dest = out_dir.join(name);

    let src_bytes = fs::read(src)?;

    // Skip the write when the destination is already byte-identical.
    if dest.is_file()
        && let Ok(existing) = fs::read(&dest)
        && existing == src_bytes
    {
        return Ok(dest);
    }

    fs::create_dir_all(out_dir)?;
    fs::write(&dest, &src_bytes)?;
    Ok(dest)
}

pub fn collect_native_libs(program: &crate::parser::Program) -> Vec<NativeLibRef> {
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
                let from_pod = super::loader::is_pod_native_lib(path);
                Some(NativeLibRef {
                    alias: alias.clone(),
                    path: path.clone(),
                    from_pod,
                    functions: functions.clone(),
                    structs: structs.clone(),
                    vars: vars.clone(),
                })
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
    fn match_ldconfig_line_matches_soname_with_arch_tag() {
        let line = "\tlibzstd.so.1 (libc6,x86-64) => /usr/lib/libzstd.so.1";
        let dir = match_ldconfig_line(line, "libzstd.so.1", "x86-64");
        assert_eq!(dir, Some(PathBuf::from("/usr/lib")));
    }

    #[test]
    fn match_ldconfig_line_matches_extended_abi_tag() {
        let line =
            "\tlibc.so.6 (libc6,x86-64, OS ABI: Linux 3.2.0) => /lib/x86_64-linux-gnu/libc.so.6";
        let dir = match_ldconfig_line(line, "libc.so.6", "x86-64");
        assert_eq!(dir, Some(PathBuf::from("/lib/x86_64-linux-gnu")));
    }

    #[test]
    fn match_ldconfig_line_rejects_mismatched_arch() {
        let line = "\tlibfoo.so.1 (libc6) => /usr/lib32/libfoo.so.1";
        assert_eq!(match_ldconfig_line(line, "libfoo.so.1", "x86-64"), None);
    }

    #[test]
    fn match_ldconfig_line_rejects_different_soname() {
        let line = "\tlibzstd.so.1 (libc6,x86-64) => /usr/lib/libzstd.so.1";
        assert_eq!(match_ldconfig_line(line, "libzstd.so", "x86-64"), None);
    }

    #[test]
    fn match_ldconfig_line_ignores_arch_when_not_requested() {
        let line = "\tlibfoo.so.1 (libc6) => /usr/lib32/libfoo.so.1";
        let dir = match_ldconfig_line(line, "libfoo.so.1", "");
        assert_eq!(dir, Some(PathBuf::from("/usr/lib32")));
    }

    #[test]
    fn match_ldconfig_line_handles_missing_tag() {
        let line = "\tlibfoo.so.1 => /usr/lib/libfoo.so.1";
        let dir = match_ldconfig_line(line, "libfoo.so.1", "x86-64");
        assert_eq!(dir, Some(PathBuf::from("/usr/lib")));
    }

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
    fn lib_link_arg_strips_shared_lib_suffix() {
        assert_eq!(lib_link_arg("libc.so.6"), "-lc");
        assert_eq!(lib_link_arg("libfoo.so"), "-lfoo");
        assert_eq!(lib_link_arg("libfoo.dylib"), "-lfoo");
    }

    #[test]
    fn library_stem_extracts_name() {
        assert_eq!(library_stem("libm.so"), Some("m"));
        assert_eq!(library_stem("libc.so.6"), Some("c"));
        assert_eq!(library_stem("libfoo.dylib"), Some("foo"));
        assert_eq!(library_stem("z"), None);
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
        assert_eq!(libs[0].alias, "foo");
        assert_eq!(libs[0].path, "/usr/lib/libfoo.so");
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
        assert_eq!(libs[0].alias, "z");
        assert_eq!(libs[1].alias, "png");
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
        assert_eq!(libs[0].alias, "foo");
    }
    #[test]
    fn stage_beside_output_copies_file() {
        let tmp = std::env::temp_dir().join("olive_test_stage");
        std::fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("libtest.so");
        let out_dir = tmp.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(&src, b"fake lib").unwrap();

        let dest = stage_beside_output(&src, &out_dir).unwrap();
        assert_eq!(dest, out_dir.join("libtest.so"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"fake lib");

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn stage_beside_output_skips_identical() {
        let tmp = std::env::temp_dir().join("olive_test_stage_skip");
        std::fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("libtest.so");
        let out_dir = tmp.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(&src, b"same bytes").unwrap();

        let dest = stage_beside_output(&src, &out_dir).unwrap();
        let mtime1 = std::fs::metadata(&dest).unwrap().modified().unwrap();

        // Call again with identical content; dest mtime must not change.
        std::thread::sleep(std::time::Duration::from_millis(10));
        stage_beside_output(&src, &out_dir).unwrap();
        let mtime2 = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "mtime changed on identical re-stage");

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn stage_beside_output_error_on_missing_source() {
        let tmp = std::env::temp_dir().join("olive_test_stage_err");
        std::fs::create_dir_all(&tmp).unwrap();
        let missing = tmp.join("nonexistent.so");
        let out_dir = tmp.join("out");
        assert!(stage_beside_output(&missing, &out_dir).is_err());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
