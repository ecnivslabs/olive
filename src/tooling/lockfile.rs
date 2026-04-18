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
