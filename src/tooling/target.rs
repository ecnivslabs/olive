//! Single source of truth for the (os, arch) keys pit uses to name prebuilt
//! native artifacts, for both its own toolchain releases (`pit upgrade`,
//! `olive_std`) and pod-provided native libraries (`[native]` in `pit.toml`).

/// Every (os, arch) combination pit currently builds and publishes artifacts
/// for. A pod's `[native]` table defaults to shipping all of these.
pub const SUPPORTED: [&str; 5] = [
    "linux-x86_64",
    "linux-aarch64",
    "macos-x86_64",
    "macos-aarch64",
    "windows-x86_64",
];

/// The target key for the machine running this code, or `None` on a platform
/// pit has no artifact naming convention for.
pub fn host() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("macos-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-aarch64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else {
        None
    }
}

/// Shared-library file extension for a target key, without the leading dot.
pub fn dylib_ext(key: &str) -> Option<&'static str> {
    match key.split('-').next()? {
        "linux" => Some("so"),
        "macos" => Some("dylib"),
        "windows" => Some("dll"),
        _ => None,
    }
}

/// Release-asset filename for a pod's native library on the given target,
/// e.g. `asset_name("tokenizer", "linux-x86_64")` -> `libtokenizer-linux-x86_64.so`.
pub fn asset_name(lib: &str, key: &str) -> Option<String> {
    Some(format!("lib{lib}-{key}.{}", dylib_ext(key)?))
}

/// The name a pod's native library is installed under locally, once the
/// target suffix has been stripped, e.g. `libtokenizer.so` on Linux,
/// `libtokenizer.dylib` on macOS, `libtokenizer.dll` on Windows. Resolved for
/// the host, since this is only ever used to name a file already downloaded
/// or built for this machine.
pub fn local_name(lib: &str) -> Option<String> {
    Some(format!("lib{lib}.{}", dylib_ext(host()?)?))
}

/// MSVC import library asset name for a Windows target, e.g.
/// `libtokenizer-windows-x86_64.dll.lib`. `None` for every non-Windows key,
/// since only the MSVC linker needs a separate import library.
pub fn implib_asset_name(lib: &str, key: &str) -> Option<String> {
    key.starts_with("windows-")
        .then(|| format!("lib{lib}-{key}.dll.lib"))
}

/// Local name for the installed import library, only under the MSVC ABI.
pub fn local_implib_name(lib: &str) -> Option<String> {
    cfg!(target_env = "msvc").then(|| format!("lib{lib}.dll.lib"))
}

/// Filename a build tool actually produces for a `cdylib` named `lib` on this
/// host. This is deliberately not always the same as [`local_name`]: cargo
/// emits `tokenizer.dll` on Windows (no `lib` prefix), not `libtokenizer.dll`.
pub fn built_name(lib: &str) -> String {
    libloading::library_filename(lib)
        .to_string_lossy()
        .into_owned()
}

/// Import library filename a build tool produces alongside a `cdylib` under
/// the MSVC ABI, e.g. `tokenizer.dll.lib` for a library named `tokenizer`.
pub fn built_implib_name(lib: &str) -> Option<String> {
    cfg!(target_env = "msvc").then(|| format!("{lib}.dll.lib"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_keys_all_resolve_a_dylib_extension() {
        for key in SUPPORTED {
            assert!(dylib_ext(key).is_some(), "no extension for {key}");
        }
    }

    #[test]
    fn asset_name_matches_upgrade_rs_naming_for_every_target() {
        assert_eq!(
            asset_name("olive_std", "linux-x86_64").unwrap(),
            "libolive_std-linux-x86_64.so"
        );
        assert_eq!(
            asset_name("olive_std", "linux-aarch64").unwrap(),
            "libolive_std-linux-aarch64.so"
        );
        assert_eq!(
            asset_name("olive_std", "macos-x86_64").unwrap(),
            "libolive_std-macos-x86_64.dylib"
        );
        assert_eq!(
            asset_name("olive_std", "macos-aarch64").unwrap(),
            "libolive_std-macos-aarch64.dylib"
        );
        assert_eq!(
            asset_name("olive_std", "windows-x86_64").unwrap(),
            "libolive_std-windows-x86_64.dll"
        );
    }

    #[test]
    fn asset_name_rejects_unknown_target() {
        assert_eq!(asset_name("olive_std", "freebsd-riscv64"), None);
    }

    #[test]
    fn implib_asset_name_only_for_windows() {
        assert_eq!(
            implib_asset_name("tokenizer", "windows-x86_64").unwrap(),
            "libtokenizer-windows-x86_64.dll.lib"
        );
        assert_eq!(implib_asset_name("tokenizer", "linux-x86_64"), None);
        assert_eq!(implib_asset_name("tokenizer", "macos-aarch64"), None);
    }

    #[test]
    fn host_returns_a_supported_key_on_this_platform() {
        if let Some(key) = host() {
            assert!(SUPPORTED.contains(&key));
        }
    }

    #[test]
    fn local_name_uses_host_extension() {
        let Some(key) = host() else { return };
        let ext = dylib_ext(key).unwrap();
        assert_eq!(
            local_name("tokenizer").unwrap(),
            format!("libtokenizer.{ext}")
        );
    }

    #[test]
    fn built_name_matches_library_filename() {
        assert_eq!(
            built_name("tokenizer"),
            libloading::library_filename("tokenizer")
                .to_string_lossy()
                .into_owned()
        );
    }
}
