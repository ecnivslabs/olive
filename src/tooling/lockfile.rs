use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Lockfile {
    pub version: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pods: Vec<LockedPod>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            version: 1,
            pods: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LockedPod {
    pub name: String,
    pub version: String,
    pub cksum: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    /// Target key -> blake3 of that target's native artifact, for every
    /// target the pod publishes (not just the host's), so a committed
    /// pit.lock verifies on every platform a team builds on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_implib: BTreeMap<String, String>,
}

pub fn load_lockfile_checked(path: &Path) -> Result<Option<Lockfile>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read lockfile {}: {error}", path.display()))?;
    let lockfile: Lockfile = toml::from_str(&content)
        .map_err(|error| format!("Invalid lockfile {}: {error}", path.display()))?;
    if lockfile.version != 1 {
        return Err(format!(
            "Unsupported lockfile version {} in {}",
            lockfile.version,
            path.display()
        ));
    }
    let mut names = HashSet::new();
    for pod in &lockfile.pods {
        if !names.insert(pod.name.as_str()) {
            return Err(format!(
                "Invalid lockfile {}: duplicate pod '{}'",
                path.display(),
                pod.name
            ));
        }
        crate::tooling::registry::validate_pod_name(&pod.name)?;
        crate::tooling::registry::validate_version(&pod.version).map_err(|error| {
            format!(
                "Invalid lockfile {}: pod '{}' has unsafe version '{}': {error}",
                path.display(),
                pod.name,
                pod.version
            )
        })?;
    }
    Ok(Some(lockfile))
}

#[allow(dead_code)]
pub fn load_lockfile(path: &Path) -> Option<Lockfile> {
    load_lockfile_checked(path).ok().flatten()
}

pub fn save_lockfile(path: &Path, lockfile: &Lockfile) -> Result<(), String> {
    let content =
        toml::to_string(lockfile).map_err(|e| format!("Failed to serialize lockfile: {e}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "lockfile path has no parent directory".to_string())?;
    for _ in 0..16 {
        let nonce = rand::random::<u64>();
        let temp = parent.join(format!(".pit.lock-{nonce:x}.tmp"));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Failed to create lockfile temp: {error}")),
        };
        if let Err(error) = file
            .write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
        {
            let _ = fs::remove_file(&temp);
            return Err(format!("Failed to write lockfile: {error}"));
        }
        return fs::rename(&temp, path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            format!("Failed to replace lockfile: {error}")
        });
    }
    Err("Failed to allocate a unique lockfile temp file".to_string())
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
            native: BTreeMap::new(),
            native_implib: BTreeMap::new(),
        }
    }

    #[test]
    fn lockfile_default_empty() {
        let lf = Lockfile::default();
        assert_eq!(lf.version, 1);
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
    fn checked_load_rejects_bad_version_and_duplicate_pods() {
        let dir = std::env::temp_dir().join("olive_lockfile_test_strict");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pit.lock");

        fs::write(&path, "version = 2\n").unwrap();
        assert!(load_lockfile_checked(&path).is_err());

        fs::write(
            &path,
            concat!(
                "version = 1\n",
                "[[pods]]\nname = \"dup\"\nversion = \"1.0.0\"\ncksum = \"a\"\n",
                "[[pods]]\nname = \"dup\"\nversion = \"1.0.0\"\ncksum = \"b\"\n"
            ),
        )
        .unwrap();
        assert!(load_lockfile_checked(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn save_lockfile_does_not_follow_fixed_temp_symlink() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join("olive_lockfile_test_symlink");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("pit.lock");
        let victim = dir.join("victim.txt");
        fs::write(&victim, b"unchanged").unwrap();
        symlink(&victim, dir.join("pit.lock.tmp")).unwrap();

        let lockfile = Lockfile {
            version: 1,
            pods: vec![make_pod("safe", "1.0.0", "checksum", &[])],
        };
        save_lockfile(&lock, &lockfile).unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"unchanged");
        assert_eq!(load_lockfile_checked(&lock).unwrap().unwrap().pods.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_fails_on_bad_path() {
        let result = save_lockfile(Path::new("/nonexistent/dir/lock"), &Lockfile::default());
        assert!(result.is_err());
    }
}
