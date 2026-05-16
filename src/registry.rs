use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const REGISTRY_BASE: &str = "https://raw.githubusercontent.com/olive-language/pit-registry/main";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodVersion {
    pub name: String,
    pub vers: String,
    #[serde(default)]
    pub deps: Vec<Dep>,
    pub cksum: String,
    pub dl: String,
    #[serde(default)]
    pub yanked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dep {
    pub name: String,
    pub req: String,
}

fn registry_url(name: &str) -> String {
    let prefix = &name[..name.len().min(2)];
    format!("{}/{}/{}", REGISTRY_BASE, prefix, name)
}

fn cache_path(name: &str) -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".pit")
        .join("cache")
        .join("registry")
        .join(name)
}

pub fn fetch_versions(name: &str) -> Result<Vec<PodVersion>, String> {
    let url = registry_url(name);
    let body = match ureq::get(&url).set("User-Agent", "pit/0.1.0").call() {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(404, _)) => {
            return Err(format!("pod '{}' not found in registry", name));
        }
        Err(e) => return Err(format!("registry fetch failed: {}", e)),
    };

    let cache = cache_path(name);
    if let Some(parent) = cache.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&cache, &body);

    parse_versions(&body)
}

fn parse_versions(body: &str) -> Result<Vec<PodVersion>, String> {
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|e| e.to_string()))
        .collect()
}

pub fn resolve_version<'a>(versions: &'a [PodVersion], req: &str) -> Option<&'a PodVersion> {
    if req == "*" || req == "latest" {
        versions.iter().rfind(|v| !v.yanked)
    } else {
        versions.iter().find(|v| v.vers == req && !v.yanked)
    }
}
