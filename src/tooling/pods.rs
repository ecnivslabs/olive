use crate::commands::utils::workspace_root;
use crate::tooling::installer;
use crate::tooling::lockfile::{LockedPod, Lockfile, load_lockfile_checked, save_lockfile};
use crate::tooling::solver;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug)]
pub enum PodError {
    Solver(solver::SolverError),
    Install(installer::InstallError),
    Lockfile(String),
}

impl std::fmt::Display for PodError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PodError::Solver(e) => write!(f, "{}", e),
            PodError::Install(e) => write!(f, "{}", e),
            PodError::Lockfile(msg) => write!(f, "Lockfile error: {}", msg),
        }
    }
}
impl std::error::Error for PodError {}

pub fn pods_dir() -> PathBuf {
    let current_olive_vers = env!("CARGO_PKG_VERSION");
    let major_minor = current_olive_vers
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".");

    dirs::home_dir()
        .expect("no home dir")
        .join(".pit")
        .join("pods")
        .join(format!("olive-v{}", major_minor))
}

pub fn installed_path(name: &str, version: &str) -> PathBuf {
    pods_dir().join(name).join(version)
}

pub async fn ensure_deps_installed(
    deps: &HashMap<String, String>,
    unlocked: Option<Vec<String>>,
    offline: bool,
) -> Result<(), PodError> {
    let workspace = workspace_root();
    let lock_path = workspace.join("pit.lock");
    let lockfile = load_lockfile_checked(&lock_path).map_err(PodError::Lockfile)?;

    let mut locked_state = HashMap::new();
    let mut locked_archives: HashMap<String, String> = HashMap::new();
    let mut locked_native: HashMap<String, std::collections::BTreeMap<String, String>> =
        HashMap::new();
    let mut locked_native_implib: HashMap<String, std::collections::BTreeMap<String, String>> =
        HashMap::new();
    if let Some(lk) = &lockfile {
        for pod in &lk.pods {
            let is_unlocked = match &unlocked {
                Some(u) if u.is_empty() => true,
                Some(u) => u.contains(&pod.name),
                None => false,
            };
            if !is_unlocked {
                locked_state.insert(pod.name.clone(), pod.version.clone());
                locked_archives.insert(pod.name.clone(), pod.cksum.clone());
                if !pod.native.is_empty() {
                    locked_native.insert(pod.name.clone(), pod.native.clone());
                }
                if !pod.native_implib.is_empty() {
                    locked_native_implib.insert(pod.name.clone(), pod.native_implib.clone());
                }
            }
        }
    }

    let resolved_pods = solver::resolve_tree(deps, Some(locked_state), offline)
        .await
        .map_err(PodError::Solver)?;

    let mut futures = Vec::new();
    for pod in &resolved_pods {
        if let Some(locked_cksum) = locked_archives.get(&pod.name)
            && locked_cksum != &pod.cksum
        {
            return Err(PodError::Lockfile(format!(
                "registry archive for {}@{} changed checksum (locked {}, found {})",
                pod.name, pod.vers, locked_cksum, pod.cksum
            )));
        }
        let final_dir = installed_path(&pod.name, &pod.vers);
        let locked = locked_native.get(&pod.name);
        let locked_implib = locked_native_implib.get(&pod.name);
        futures.push(installer::install_pod_atomic(
            pod,
            final_dir,
            locked,
            locked_implib,
            offline,
        ));
    }

    for res in futures::future::join_all(futures).await {
        res.map_err(PodError::Install)?;
    }

    let mut locked_pods: Vec<LockedPod> = resolved_pods
        .into_iter()
        .map(|pod| LockedPod {
            name: pod.name.clone(),
            version: pod.vers.clone(),
            cksum: pod.cksum.clone(),
            dependencies: pod
                .deps
                .into_iter()
                .map(|d| format!("{} {}", d.name, d.req))
                .collect(),
            native: pod
                .native
                .as_ref()
                .map(|spec| {
                    spec.artifacts
                        .iter()
                        .map(|(key, artifact)| (key.clone(), artifact.cksum.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            native_implib: pod
                .native
                .as_ref()
                .map(|spec| {
                    spec.artifacts
                        .iter()
                        .filter_map(|(key, artifact)| {
                            artifact
                                .implib
                                .as_ref()
                                .map(|file| (key.clone(), file.cksum.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();

    locked_pods.sort_by(|a, b| a.name.cmp(&b.name));

    let new_lockfile = Lockfile {
        version: 1,
        pods: locked_pods,
    };

    save_lockfile(&lock_path, &new_lockfile).map_err(PodError::Lockfile)?;

    Ok(())
}

pub async fn install_all_deps(
    deps: &HashMap<String, String>,
    unlocked: Option<Vec<String>>,
    offline: bool,
) -> Result<(), PodError> {
    ensure_deps_installed(deps, unlocked, offline).await
}

pub fn find_pod_path(pod_name: &str) -> Option<PathBuf> {
    let pod_base = pods_dir().join(pod_name);
    if !pod_base.exists() {
        return None;
    }

    let mut resolved_version = None;
    let workspace = workspace_root();
    if let Some(lockfile) = load_lockfile_checked(&workspace.join("pit.lock"))
        .ok()
        .flatten()
        && let Some(pod) = lockfile.pods.iter().find(|p| p.name == pod_name)
    {
        resolved_version = Some(pod.version.clone());
    }

    let pod_dir = if let Some(vers) = resolved_version {
        let path = pod_base.join(vers);
        if path.exists() {
            path
        } else {
            return None;
        }
    } else {
        let mut highest = None;
        let mut highest_v = None;

        if let Ok(entries) = fs::read_dir(&pod_base) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let dirname = entry.file_name().to_string_lossy().to_string();
                    if let Ok(v) = semver::Version::parse(&dirname) {
                        if let Some(ref hv) = highest_v {
                            if v > *hv {
                                highest_v = Some(v.clone());
                                highest = Some(entry.path());
                            }
                        } else {
                            highest_v = Some(v);
                            highest = Some(entry.path());
                        }
                    }
                }
            }
        }
        highest?
    };

    let pod_toml = pod_dir.join("pit.toml");
    if pod_toml.exists()
        && let Ok(content) = fs::read_to_string(&pod_toml)
        && let Ok(val) = toml::from_str::<toml::Value>(&content)
        && let Some(entry) = val
            .get("pod")
            .and_then(|p| p.get("entry"))
            .and_then(|e| e.as_str())
        && let Ok(entry_path) =
            crate::tooling::manifest::resolve_file_within(&pod_dir, entry, "pod entry")
    {
        return Some(entry_path);
    }

    let candidates = [
        pod_dir.join(format!("{}.liv", pod_name)),
        pod_dir.join("lib.liv"),
        pod_dir.join("mod.liv"),
        pod_dir.join("src").join(format!("{}.liv", pod_name)),
        pod_dir.join("src").join("lib.liv"),
        pod_dir.join("src").join("mod.liv"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pods_dir_contains_pit_and_pods() {
        let dir = pods_dir();
        let s = dir.to_string_lossy();
        assert!(s.contains(".pit"));
        assert!(s.contains("pods"));
    }

    #[test]
    fn pods_dir_contains_olive_version_prefix() {
        let dir = pods_dir();
        let s = dir.to_string_lossy();
        assert!(s.contains("olive-v"));
    }

    #[test]
    fn installed_path_joins_name_and_version() {
        let path = installed_path("my_pkg", "1.2.3");
        let s = path.to_string_lossy();
        assert!(s.contains("my_pkg"));
        assert!(s.contains("1.2.3"));
    }

    #[test]
    fn pod_error_solver_display() {
        let e = PodError::Solver(solver::SolverError::Unresolvable("bad deps".into()));
        let msg = format!("{e}");
        assert!(msg.contains("dependency resolution failed"));
    }

    #[test]
    fn pod_error_install_display() {
        let e = PodError::Install(installer::InstallError::Download("timeout".into()));
        let msg = format!("{e}");
        assert!(msg.contains("download failed"));
    }

    #[test]
    fn pod_error_lockfile_display() {
        let e = PodError::Lockfile("corrupt lock".into());
        let msg = format!("{e}");
        assert!(msg.contains("Lockfile error"));
    }
}
