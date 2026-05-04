use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Lockfile {
    pub version: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pods: Vec<LockedPod>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LockedPod {
    pub name: String,
    pub version: String,
    pub cksum: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

pub fn load_lockfile(path: &Path) -> Option<Lockfile> {
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

pub fn save_lockfile(path: &Path, lockfile: &Lockfile) -> Result<(), String> {
    let content =
        toml::to_string(lockfile).map_err(|e| format!("Failed to serialize lockfile: {}", e))?;
    fs::write(path, content).map_err(|e| format!("Failed to write lockfile: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pod(name: &str, version: &str, cksum: &str, deps: &[&str]) -> LockedPod {
        LockedPod {
            name: name.to_string(),
            version: version.to_string(),
            cksum: cksum.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn lockfile_default_empty() {
        let lf = Lockfile::default();
        assert_eq!(lf.version, 0);
        assert!(lf.pods.is_empty());
    }

    #[test]
    fn lockfile_roundtrip_toml() {
        let lf = Lockfile {
            version: 1,
            pods: vec![
                make_pod("pkg_a", "1.0.0", "abc123", &["pkg_b >=0.5"]),
                make_pod("pkg_b", "0.5.2", "def456", &[]),
            ],
        };
        let toml_str = toml::to_string(&lf).unwrap();
        let deserialized: Lockfile = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.version, 1);
        assert_eq!(deserialized.pods.len(), 2);
        assert_eq!(deserialized.pods[0].name, "pkg_a");
        assert_eq!(deserialized.pods[0].version, "1.0.0");
        assert_eq!(deserialized.pods[0].cksum, "abc123");
        assert_eq!(deserialized.pods[0].dependencies.len(), 1);
    }

    #[test]
    fn lockfile_empty_pods_skips_in_toml() {
        let lf = Lockfile {
            version: 1,
            pods: vec![],
        };
        let toml_str = toml::to_string(&lf).unwrap();
        assert!(!toml_str.contains("pods"));
    }

    #[test]
    fn locked_pod_equality() {
        let a = make_pod("x", "1.0", "cksum1", &["y"]);
        let b = make_pod("x", "1.0", "cksum1", &["y"]);
        let c = make_pod("x", "2.0", "cksum1", &["y"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("olive_lockfile_test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("pit.lock");

        let lf = Lockfile {
            version: 1,
            pods: vec![make_pod("test_pkg", "0.1.0", "xyz", &[])],
        };

        save_lockfile(&path, &lf).unwrap();
        let loaded = load_lockfile(&path).unwrap();

        assert_eq!(loaded.version, lf.version);
        assert_eq!(loaded.pods.len(), lf.pods.len());
        assert_eq!(loaded.pods[0].name, "test_pkg");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let path = Path::new("/nonexistent/path/pit.lock");
        assert!(load_lockfile(path).is_none());
    }

    #[test]
    fn load_invalid_toml_returns_none() {
        let dir = std::env::temp_dir().join("olive_lockfile_test_invalid");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("pit.lock");
        fs::write(&path, "invalid toml content {{{").unwrap();
        assert!(load_lockfile(&path).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn locked_pod_empty_deps_skipped() {
        let pod = make_pod("standalone", "1.0", "cksum", &[]);
        let toml_str = toml::to_string(&pod).unwrap();
        assert!(!toml_str.contains("dependencies"));
    }

    #[test]
    fn save_fails_on_bad_path() {
        let result = save_lockfile(Path::new("/nonexistent/dir/lock"), &Lockfile::default());
        assert!(result.is_err());
    }
}
