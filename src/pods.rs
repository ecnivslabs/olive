use crate::registry::{self, PodVersion};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
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

pub fn download_and_install(pod: &PodVersion) -> Result<(), String> {
    let install_dir = installed_path(&pod.name, &pod.vers);

    if install_dir.exists() {
        return Ok(());
    }

    println!("\x1b[1;32m  Downloading\x1b[0m {}@{}", pod.name, pod.vers);

    let bytes = match ureq::get(&pod.dl)
        .set("User-Agent", "pit/0.1.0")
        .call()
    {
        Ok(resp) => {
            let mut buf = Vec::new();
            resp.into_reader()
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            buf
        }
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

pub fn ensure_deps_installed(deps: &HashMap<String, String>) -> Result<(), String> {
    for (name, version_req) in deps {
        let install_dir = installed_path(name, version_req);
        if install_dir.exists() {
            continue;
        }
        install_one(name, version_req)?;
    }
    Ok(())
}

pub fn install_all_deps(deps: &HashMap<String, String>) -> Result<(), String> {
    for (name, version_req) in deps {
        install_one(name, version_req)?;
    }
    Ok(())
}

fn install_one(name: &str, version_req: &str) -> Result<(), String> {
    let versions = registry::fetch_versions(name)?;
    let pod = registry::resolve_version(&versions, version_req)
        .ok_or_else(|| format!("no matching version for '{}@{}'", name, version_req))?
        .clone();

    download_and_install(&pod)?;

    if !pod.deps.is_empty() {
        let sub_deps: HashMap<String, String> = pod
            .deps
            .iter()
            .map(|d| (d.name.clone(), d.req.clone()))
            .collect();
        install_all_deps(&sub_deps)?;
    }
    Ok(())
}

pub fn find_pod_path(pod_name: &str) -> Option<PathBuf> {
    let pod_base = pods_dir().join(pod_name);
    if !pod_base.exists() {
        return None;
    }

    // pick first installed version (mirrors cargo's global store approach)
    let pod_dir = fs::read_dir(&pod_base).ok()?.filter_map(|e| {
        let e = e.ok()?;
        if e.path().is_dir() { Some(e.path()) } else { None }
    }).next()?;

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
