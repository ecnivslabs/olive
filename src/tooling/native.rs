use crate::tooling::manifest::Native;
use crate::tooling::target;
use std::path::Path;

fn blake3_file(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|data| blake3::hash(&data).to_hex().to_string())
}

fn copy_if_different(src: &Path, dest: &Path) -> Result<bool, String> {
    let src_hash = blake3_file(src)
        .ok_or_else(|| format!("could not read built library at {}", src.display()))?;
    if let Some(dest_hash) = blake3_file(dest)
        && dest_hash == src_hash
    {
        return Ok(false);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    std::fs::copy(src, dest).map_err(|e| {
        format!(
            "could not copy {} to {}: {e}",
            src.display(),
            dest.display()
        )
    })?;
    Ok(true)
}

/// Builds the current project's native library and stages it into `native/`.
///
/// Runs only for the root project's own manifest. Never for an installed pod
/// under `~/.pit/pods/`, where executing a build command would turn `pit add`
/// into arbitrary code execution.
pub fn ensure_built(native: &Native) -> Result<(), String> {
    let argv = native.build_argv();
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| "invalid [native] build command: empty argv".to_string())?;
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .map_err(|e| {
            let mut msg = format!(
                "error: could not run the [native] build command '{}': {e}",
                argv.join(" ")
            );
            if program == "cargo" {
                msg.push_str("\n  help: is 'cargo' installed and on PATH?");
            }
            msg
        })?;
    if !status.success() {
        return Err(format!(
            "error: [native] build command failed (exit {}): {}",
            status.code().unwrap_or(-1),
            argv.join(" ")
        ));
    }

    let built = target::built_name(&native.lib);
    let src = Path::new(native.artifact_dir()).join(&built);
    if !src.is_file() {
        return Err(format!(
            "error: the [native] build command did not produce the expected library\n  expected: {}\n  set [native].dir in pit.toml if your build writes somewhere else",
            src.display()
        ));
    }

    let local = target::local_name(&native.lib).ok_or_else(|| {
        "error: pit has no artifact naming convention for this platform".to_string()
    })?;
    let dest = Path::new("native").join(&local);
    copy_if_different(&src, &dest)?;

    if let (Some(built_implib), Some(local_implib)) = (
        target::built_implib_name(&native.lib),
        target::local_implib_name(&native.lib),
    ) {
        let src_implib = Path::new(native.artifact_dir()).join(&built_implib);
        let dest_implib = Path::new("native").join(&local_implib);
        if src_implib.is_file() {
            copy_if_different(&src_implib, &dest_implib)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_if_different_copies_new_file() {
        let dir = std::env::temp_dir().join("olive_native_test_copy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("libfoo.so");
        let dest = dir.join("native").join("libfoo.so");
        std::fs::write(&src, b"bytes-v1").unwrap();
        assert!(copy_if_different(&src, &dest).unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), b"bytes-v1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_if_different_skips_identical_content() {
        let dir = std::env::temp_dir().join("olive_native_test_skip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("libfoo.so");
        let dest = dir.join("native").join("libfoo.so");
        std::fs::write(&src, b"same").unwrap();
        assert!(copy_if_different(&src, &dest).unwrap());
        let mtime1 = std::fs::metadata(&dest).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(!copy_if_different(&src, &dest).unwrap());
        let mtime2 = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_if_different_rewrites_changed_content() {
        let dir = std::env::temp_dir().join("olive_native_test_rewrite");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("libfoo.so");
        let dest = dir.join("native").join("libfoo.so");
        std::fs::write(&src, b"v1").unwrap();
        assert!(copy_if_different(&src, &dest).unwrap());
        std::fs::write(&src, b"v2").unwrap();
        assert!(copy_if_different(&src, &dest).unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), b"v2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_if_different_errors_on_missing_source() {
        let dir = std::env::temp_dir().join("olive_native_test_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("nonexistent.so");
        let dest = dir.join("native").join("libfoo.so");
        assert!(copy_if_different(&missing, &dest).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
