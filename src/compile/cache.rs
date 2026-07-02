use super::linker::{compute_source_hash, ensure_dir};
use super::loader::{self, collect_source_files};
use std::{collections::HashSet, fs, path::Path};

const MANIFEST_PATH: &str = "grove/cache/manifest.json";

pub struct BuildTarget {
    pub binary_path: String,
    hash: u64,
    mode_key: &'static str,
}

impl BuildTarget {
    /// Same identity `pgo::path_for_hash` keys a profile by.
    pub fn hash(&self) -> u64 {
        self.hash
    }
}

fn mode_key(release: bool) -> &'static str {
    if release { "release" } else { "debug" }
}

fn binary_name(entry: &str) -> String {
    loader::pod_name().unwrap_or_else(|| {
        Path::new(entry)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "program".to_string())
    })
}

pub fn resolve_binary_path(entry: &str, release: bool) -> String {
    let ext = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };
    format!("grove/{}/{}{}", mode_key(release), binary_name(entry), ext)
}

/// Hashes the entry's source files (plus any referenced Python files and
/// `pit.toml`) and resolves the target binary path/cache key for this
/// entry+mode. Also returns the Python files so callers can invalidate
/// their stale `.pyc` bytecode before a rebuild.
pub fn prepare(entry: &str, release: bool) -> (BuildTarget, Vec<String>) {
    let mut collected = Vec::new();
    let mut py_files = Vec::new();
    let mut visited = HashSet::new();
    collect_source_files(entry, &mut collected, &mut py_files, &mut visited);

    let mut all_files = collected;
    all_files.extend(py_files.iter().cloned());
    if let Ok(p) = fs::canonicalize("pit.toml") {
        all_files.push(p.to_string_lossy().into_owned());
    }
    let hash = compute_source_hash(&all_files);

    let target = BuildTarget {
        binary_path: resolve_binary_path(entry, release),
        hash,
        mode_key: mode_key(release),
    };
    (target, py_files)
}

pub fn is_fresh(target: &BuildTarget) -> bool {
    if !Path::new(&target.binary_path).exists() {
        return false;
    }
    let Ok(text) = fs::read_to_string(MANIFEST_PATH) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    root[target.mode_key]["hash"].as_u64() == Some(target.hash)
}

pub fn record(target: &BuildTarget) {
    ensure_dir("grove/cache");
    let mut root = fs::read_to_string(MANIFEST_PATH)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    root[target.mode_key] = serde_json::json!({ "hash": target.hash });
    let _ = fs::write(MANIFEST_PATH, root.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::utils::CWD_LOCK;

    struct ScratchDir {
        prev: std::path::PathBuf,
        dir: std::path::PathBuf,
    }

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(name);
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let prev = std::env::current_dir().unwrap();
            std::env::set_current_dir(&dir).unwrap();
            Self { prev, dir }
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.prev).unwrap();
            let _ = fs::remove_dir_all(&self.dir);
            loader::clear_pod_meta();
        }
    }

    #[test]
    fn resolve_binary_path_falls_back_to_file_stem_when_no_pod() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_cache_test_stem");
        loader::clear_pod_meta();
        assert_eq!(
            resolve_binary_path("src/main.liv", false),
            "grove/debug/main"
        );
    }

    #[test]
    fn resolve_binary_path_uses_pod_name_when_set() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_cache_test_podname");
        loader::set_pod_meta(loader::PodMeta {
            name: "myproj".to_string(),
            version: "0.1.0".to_string(),
            author: String::new(),
        });
        assert_eq!(
            resolve_binary_path("src/main.liv", false),
            "grove/debug/myproj"
        );
    }

    #[test]
    fn resolve_binary_path_differs_between_debug_and_release() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_cache_test_modes");
        loader::set_pod_meta(loader::PodMeta {
            name: "myproj".to_string(),
            version: "0.1.0".to_string(),
            author: String::new(),
        });
        assert_eq!(resolve_binary_path("x", false), "grove/debug/myproj");
        assert_eq!(resolve_binary_path("x", true), "grove/release/myproj");
    }

    fn target(binary_path: &str, hash: u64, mode_key: &'static str) -> BuildTarget {
        BuildTarget {
            binary_path: binary_path.to_string(),
            hash,
            mode_key,
        }
    }

    #[test]
    fn is_fresh_false_when_manifest_missing() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_cache_test_no_manifest");
        fs::create_dir_all("grove/debug").unwrap();
        fs::write("grove/debug/myproj", b"binary").unwrap();
        let t = target("grove/debug/myproj", 42, "debug");
        assert!(!is_fresh(&t));
    }

    #[test]
    fn is_fresh_false_when_binary_missing_even_if_hash_matches() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_cache_test_no_binary");
        let t = target("grove/debug/myproj", 42, "debug");
        record(&t);
        assert!(!is_fresh(&t));
    }

    #[test]
    fn is_fresh_true_when_hash_matches_and_binary_exists() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_cache_test_fresh");
        fs::create_dir_all("grove/debug").unwrap();
        fs::write("grove/debug/myproj", b"binary").unwrap();
        let t = target("grove/debug/myproj", 42, "debug");
        record(&t);
        assert!(is_fresh(&t));
    }

    #[test]
    fn record_preserves_other_mode_key() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_cache_test_dual_mode");
        let release_target = target("grove/release/myproj", 111, "release");
        record(&release_target);
        let debug_target = target("grove/debug/myproj", 222, "debug");
        record(&debug_target);

        let text = fs::read_to_string(MANIFEST_PATH).unwrap();
        let root: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(root["release"]["hash"].as_u64(), Some(111));
        assert_eq!(root["debug"]["hash"].as_u64(), Some(222));
    }
}
