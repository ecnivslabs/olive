use crate::commands::utils::workspace_root;
use crate::tooling::installer;
use crate::tooling::lockfile::{LockedPod, Lockfile, load_lockfile, save_lockfile};
use crate::tooling::solver;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug)]
pub enum PodError {
    Solver(solver::SolverError),
    Install(installer::InstallError),
    Io(std::io::Error),
    Lockfile(String),
}

impl std::fmt::Display for PodError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PodError::Solver(e) => write!(f, "{}", e),
            PodError::Install(e) => write!(f, "{}", e),
            PodError::Io(e) => write!(f, "I/O error: {}", e),
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
    let lockfile = load_lockfile(&lock_path);

    let mut locked_state = HashMap::new();
    if let Some(lk) = &lockfile {
        for pod in &lk.pods {
            let is_unlocked = match &unlocked {
                Some(u) if u.is_empty() => true,
                Some(u) => u.contains(&pod.name),
                None => false,
            };
            if !is_unlocked {
                locked_state.insert(pod.name.clone(), pod.version.clone());
            }
        }
    }

    let resolved_pods = solver::resolve_tree(deps, Some(locked_state), offline)
        .await
        .map_err(PodError::Solver)?;

    let mut futures = Vec::new();
    for pod in &resolved_pods {
        let final_dir = installed_path(&pod.name, &pod.vers);
        futures.push(installer::install_pod_atomic(pod, final_dir));
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
        })
        .collect();

    locked_pods.sort_by(|a, b| a.name.cmp(&b.name));

    let new_lockfile = Lockfile {
        version: 1,
        pods: locked_pods,
    };

    let tmp_lock = workspace.join("pit.lock.tmp");
    save_lockfile(&tmp_lock, &new_lockfile).map_err(PodError::Lockfile)?;
    fs::rename(tmp_lock, lock_path).map_err(PodError::Io)?;

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
    if let Some(lockfile) = load_lockfile(&workspace.join("pit.lock")) {
        if let Some(pod) = lockfile.pods.iter().find(|p| p.name == pod_name) {
            resolved_version = Some(pod.version.clone());
        }
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
    if pod_toml.exists() {
        if let Ok(content) = fs::read_to_string(&pod_toml) {
            if let Ok(val) = toml::from_str::<toml::Value>(&content) {
                if let Some(entry) = val
                    .get("pod")
                    .and_then(|p| p.get("entry"))
                    .and_then(|e| e.as_str())
                {
                    let entry_path = pod_dir.join(entry);
                    if entry_path.exists() {
                        return Some(entry_path);
                    }
                }
            }
        }
    }

    let candidates = [
        pod_dir.join(format!("{}.liv", pod_name)),
        pod_dir.join("lib.liv"),
        pod_dir.join("src").join(format!("{}.liv", pod_name)),
        pod_dir.join("src").join("lib.liv"),
    ];
    candidates.into_iter().find(|p| p.exists())
}
