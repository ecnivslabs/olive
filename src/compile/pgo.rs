//! Disk I/O for PGO profiles: `grove/profile/<hash>.json`, hash shared with `compile::cache`.

use crate::codegen::cranelift::profile::Profile;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub(crate) fn path_for_hash(hash: u64) -> String {
    format!("grove/profile/{hash:x}.json")
}

/// Shared by every AOT-producing path and the JIT entry path, so "does a
/// profile for this source already exist" is answered one way everywhere.
pub(crate) fn auto_detect(hash: u64) -> Option<String> {
    let path = path_for_hash(hash);
    Path::new(&path).exists().then_some(path)
}

/// Mirrors `tier_up.rs`'s `TIER_UP_THRESHOLD` in spirit, kept separate so retuning one doesn't retune the other.
/// Real loop-driven vs. incidental call counts separate by 3+ orders of magnitude
/// (`threshold_separates_loop_calls_from_incidental_calls_by_wide_margin`,
/// benchmark/results/tier_sweep.md) -- low-sensitivity, 1000 sits well inside the gap.
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

    /// `PGO_HOT_CALL_THRESHOLD` gates inliner effort, not latency directly --
    /// a hyperfine sweep can't isolate its effect the way `TIER_UP_THRESHOLD`/
    /// `ANY_SITE_SAMPLE_WINDOW` can (see `benchmark/results/tier_sweep.md`).
    /// Real evidence instead: a callee invoked inside a loop vs. one invoked
    /// a handful of setup-style times lands on opposite sides of 1000 by
    /// three orders of magnitude, not a handful of calls -- this is a
    /// low-sensitivity threshold, moving it within a wide band changes
    /// nothing for either real-world shape.
    #[test]
    fn threshold_separates_loop_calls_from_incidental_calls_by_wide_margin() {
        use crate::codegen::cranelift::profile::Profile;
        use crate::test_utils::{call_i64_1, compile_minimal};

        let hot_src = concat!(
            "fn callee(n: i64) -> i64:\n",
            "    return n + 1\n",
            "\n",
            "fn driver(n: i64) -> i64:\n",
            "    let mut i = 0\n",
            "    let mut r = 0\n",
            "    while i < n:\n",
            "        r = callee(i)\n",
            "        i = i + 1\n",
            "    return r\n",
        );
        let mut hot_cg = compile_minimal(hot_src);
        call_i64_1(&mut hot_cg, "driver", 5000);
        let hot_profile: Profile = hot_cg.export_profile();
        let hot_count = hot_profile.functions["callee"].hotcount;

        let cold_src = concat!(
            "fn callee(n: i64) -> i64:\n",
            "    return n + 1\n",
            "\n",
            "fn driver(n: i64) -> i64:\n",
            "    let a = callee(n)\n",
            "    let b = callee(a)\n",
            "    let c = callee(b)\n",
            "    return c\n",
        );
        let mut cold_cg = compile_minimal(cold_src);
        call_i64_1(&mut cold_cg, "driver", 1);
        let cold_profile: Profile = cold_cg.export_profile();
        let cold_count = cold_profile.functions["callee"].hotcount;

        assert!(hot_count >= PGO_HOT_CALL_THRESHOLD * 3);
        assert!(cold_count * 100 < PGO_HOT_CALL_THRESHOLD);
        assert!(hot_functions(&hot_profile).contains("callee"));
        assert!(!hot_functions(&cold_profile).contains("callee"));
    }
}
