//! Disk I/O for PGO profiles: `grove/profile/<hash>.json`, hash shared with `compile::cache`.

use crate::codegen::cranelift::profile::Profile;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub(crate) fn path_for_hash(hash: u64) -> String {
    format!("grove/profile/{hash:x}.json")
}

/// Mirrors `tier_up.rs`'s `TIER_UP_THRESHOLD` in spirit, kept separate so retuning one doesn't retune the other.
const PGO_HOT_CALL_THRESHOLD: i64 = 1000;

/// Fed into `mir::Optimizer::new_with_hot_functions` for PGO-guided inlining.
pub(crate) fn hot_functions(profile: &Profile) -> HashSet<String> {
    profile
        .functions
        .iter()
        .filter(|(_, fp)| fp.hotcount >= PGO_HOT_CALL_THRESHOLD)
        .map(|(name, _)| name.clone())
        .collect()
}

/// Best-effort: a failed write must never affect the program's own exit code
/// or output, since this always runs after the user's program has already
/// finished running.
pub(crate) fn write(profile: &Profile, path: &str) {
    if let Some(parent) = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(profile) {
        let _ = fs::write(path, json);
    }
}

pub(crate) fn load(path: &str) -> Option<Profile> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
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
        }
    }

    #[test]
    fn write_then_load_round_trips() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_pgo_test_round_trip");

        let mut profile = Profile::default();
        profile.functions.insert(
            "f".to_string(),
            crate::codegen::cranelift::profile::FunctionProfile {
                hotcount: 42,
                any_add_sites: vec![8, 3],
            },
        );
        let path = path_for_hash(0xdead_beef);
        write(&profile, &path);
        let restored = load(&path).unwrap();
        assert_eq!(restored.functions["f"].hotcount, 42);
        assert_eq!(restored.functions["f"].any_add_sites, vec![8, 3]);
    }

    #[test]
    fn load_missing_file_returns_none() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_pgo_test_missing");
        assert!(load(&path_for_hash(1)).is_none());
    }

    #[test]
    fn load_corrupt_json_returns_none() {
        let _lock = CWD_LOCK.lock().unwrap();
        let _scratch = ScratchDir::new("olive_pgo_test_corrupt");
        let path = path_for_hash(2);
        fs::create_dir_all("grove/profile").unwrap();
        fs::write(&path, b"not json").unwrap();
        assert!(load(&path).is_none());
    }

    #[test]
    fn path_for_hash_is_stable() {
        assert_eq!(path_for_hash(255), path_for_hash(255));
        assert_ne!(path_for_hash(1), path_for_hash(2));
    }
}
