use crate::tooling::registry::{self, PodVersion};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub fn pods_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".pit")
        .join("pods")
}

fn installed_path(name: &str, version: &str) -> PathBuf {
    pods_dir().join(name).join(version)
}

pub async fn download_and_install(pod: &PodVersion) -> Result<(), String> {
    let install_dir = installed_path(&pod.name, &pod.vers);

    if install_dir.exists() {
        return Ok(());
    }

    println!("\x1b[1;32m  Downloading\x1b[0m {}@{}", pod.name, pod.vers);

    let client = reqwest::Client::new();
    let bytes = match client
        .get(&pod.dl)
        .header("User-Agent", "pit/0.1.0")
        .send()
        .await
    {
        Ok(resp) => resp.bytes().await.map_err(|e| e.to_string())?.to_vec(),
        Err(e) => return Err(format!("download failed: {}", e)),
    };

    let mut hasher = blake3::Hasher::new();
    hasher.update(&bytes);
    let cksum = hasher.finalize().to_hex().to_string();

    if cksum != pod.cksum {
        return Err(format!(
            "checksum mismatch for {}: expected {}, got {}",
            pod.name, pod.cksum, cksum
        ));
    }

    let decompressed = zstd::decode_all(bytes.as_slice()).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(decompressed.as_slice());

    fs::create_dir_all(&install_dir).map_err(|e| e.to_string())?;

    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let raw_path = entry.path().map_err(|e| e.to_string())?;
        let stripped: PathBuf = raw_path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let dest = install_dir.join(&stripped);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entry.unpack(&dest).map_err(|e| e.to_string())?;
    }

    println!("\x1b[1;32m  Installed\x1b[0m {}@{}", pod.name, pod.vers);
    Ok(())
}

use crate::tooling::lockfile::{LockedPod, Lockfile, load_lockfile, save_lockfile};
use std::path::Path;

lazy_static::lazy_static! {
    static ref INSTALL_LOCKS: Arc<Mutex<HashMap<String, ()>>> = Arc::new(Mutex::new(HashMap::new()));
    static ref LOCKED_PODS: Arc<Mutex<Vec<LockedPod>>> = Arc::new(Mutex::new(Vec::new()));
}

pub async fn ensure_deps_installed(deps: &HashMap<String, String>) -> Result<(), String> {
    let mut futures = Vec::new();

    let lockfile = load_lockfile(Path::new("pit.lock"));
    let mut use_lockfile = false;
    if let Some(ref _lk) = lockfile {
        use_lockfile = true;
    }

    for (name, version_req) in deps {
        let mut pinned_version = version_req.clone();
        if use_lockfile {
            if let Some(lk) = &lockfile {
                if let Some(lp) = lk.pods.iter().find(|p| &p.name == name) {
                    pinned_version = lp.version.clone();
                }
            }
        }

        let install_dir = installed_path(name, &pinned_version);
        if install_dir.exists() {
            // Still need to track it for the final lockfile if we are regenerating it.
        }
        futures.push(install_one(
            name.clone(),
            pinned_version.clone(),
            lockfile.clone(),
        ));
    }
    for res in futures::future::join_all(futures).await {
        res?;
    }

    // Save lockfile
    let mut locked_pods = LOCKED_PODS.lock().await;
    locked_pods.sort_by(|a, b| a.name.cmp(&b.name));
    let new_lockfile = Lockfile {
        version: 1,
        pods: locked_pods.clone(),
    };
    save_lockfile(Path::new("pit.lock"), &new_lockfile)?;

    Ok(())
}

pub async fn install_all_deps(deps: &HashMap<String, String>) -> Result<(), String> {
    let mut futures = Vec::new();
    let lockfile = load_lockfile(Path::new("pit.lock"));
    for (name, version_req) in deps {
        futures.push(install_one(
            name.clone(),
            version_req.clone(),
            lockfile.clone(),
        ));
    }
    for res in futures::future::join_all(futures).await {
        res?;
    }
    Ok(())
}

use std::sync::Arc;
use tokio::sync::Mutex;

async fn install_one(
    name: String,
    version_req: String,
    lockfile: Option<Lockfile>,
) -> Result<(), String> {
    let lock_key = format!("{}@{}", name, version_req);
    {
        let mut locks = INSTALL_LOCKS.lock().await;
        if locks.contains_key(&lock_key) {
            return Ok(());
        }
        locks.insert(lock_key.clone(), ());
    }

    let versions = registry::fetch_versions(&name).await?;
    let pod = registry::resolve_version(&versions, &version_req)
        .ok_or_else(|| format!("no matching version for '{}@{}'", name, version_req))?
        .clone();

    download_and_install(&pod).await?;

    let locked_pod = LockedPod {
        name: pod.name.clone(),
        version: pod.vers.clone(),
        cksum: pod.cksum.clone(),
        dependencies: pod
            .deps
            .iter()
            .map(|d| format!("{} {}", d.name, d.req))
            .collect(),
    };

    {
        let mut locked_pods = LOCKED_PODS.lock().await;
        if !locked_pods
            .iter()
            .any(|p| p.name == locked_pod.name && p.version == locked_pod.version)
        {
            locked_pods.push(locked_pod);
        }
    }

    if !pod.deps.is_empty() {
        let sub_deps: HashMap<String, String> = pod
            .deps
            .iter()
            .map(|d| (d.name.clone(), d.req.clone()))
            .collect();
        let mut sub_futures = Vec::new();
        for (sub_name, sub_req) in sub_deps {
            sub_futures.push(Box::pin(install_one(sub_name, sub_req, lockfile.clone())));
        }
        for res in futures::future::join_all(sub_futures).await {
            res?;
        }
    }
    Ok(())
}

pub fn find_pod_path(pod_name: &str) -> Option<PathBuf> {
    let pod_base = pods_dir().join(pod_name);
    if !pod_base.exists() {
        return None;
    }

    // pick first installed version (mirrors cargo's global store approach)
    let pod_dir = fs::read_dir(&pod_base)
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            if e.path().is_dir() {
                Some(e.path())
            } else {
                None
            }
        })
        .next()?;

    let pod_toml = pod_dir.join("pit.toml");
    if pod_toml.exists()
        && let Ok(content) = fs::read_to_string(&pod_toml)
        && let Ok(val) = toml::from_str::<toml::Value>(&content)
        && let Some(entry) = val
            .get("pod")
            .and_then(|p| p.get("entry"))
            .and_then(|e| e.as_str())
    {
        let entry_path = pod_dir.join(entry);
        if entry_path.exists() {
            return Some(entry_path);
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
