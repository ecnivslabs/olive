use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
struct PitConfig {
    registry: Option<RegistryConfig>,
}

#[derive(Deserialize)]
struct RegistryConfig {
    url: Option<String>,
}

fn get_registry_base() -> String {
    let config_path = dirs::home_dir()
        .expect("no home dir")
        .join(".pit")
        .join("config.toml");

    if let Ok(content) = fs::read_to_string(config_path) {
        if let Ok(config) = toml::from_str::<PitConfig>(&content) {
            if let Some(reg) = config.registry {
                if let Some(url) = reg.url {
                    return url;
                }
            }
        }
    }
    "https://raw.githubusercontent.com/olive-language/pit-registry/main".to_string()
}

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
    #[serde(default)]
    pub olive_req: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dep {
    pub name: String,
    pub req: String,
}

fn registry_url(name: &str) -> String {
    let prefix = &name[..name.len().min(2)];
    let base = get_registry_base();
    format!("{}/{}/{}", base, prefix, name)
}

fn cache_path(name: &str) -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".pit")
        .join("cache")
        .join("registry")
        .join(name)
}

pub async fn fetch_versions(name: &str, offline: bool) -> Result<Vec<PodVersion>, String> {
    let cache = cache_path(name);

    if offline {
        if cache.exists() {
            let body =
                fs::read_to_string(&cache).map_err(|e| format!("cache read failed: {}", e))?;
            return parse_versions(&body);
        } else {
            return Err(format!("offline mode: pod '{}' not found in cache", name));
        }
    }

    let url = registry_url(name);
    let client = reqwest::Client::new();
    let body = match client
        .get(&url)
        .header("User-Agent", "pit/0.1.0")
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status() == 404 {
                return Err(format!("pod '{}' not found in registry", name));
            }
            resp.text().await.map_err(|e| e.to_string())?
        }
        Err(e) => {
            if cache.exists() {
                let cached_body = fs::read_to_string(&cache).map_err(|ce| ce.to_string())?;
                return parse_versions(&cached_body);
            }
            return Err(format!("registry fetch failed: {}", e));
        }
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
    let current_olive_vers = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| semver::Version::new(0, 1, 0));

    let req_parsed = if req == "*" || req == "latest" {
        semver::VersionReq::STAR
    } else {
        semver::VersionReq::parse(req)
            .unwrap_or_else(|_| semver::VersionReq::parse("0.0.0").unwrap())
    };

    versions.iter().rev().find(|v| {
        if v.yanked {
            return false;
        }

        let mut matches_olive = true;
        if let Some(ref oreq) = v.olive_req {
            if let Ok(olive_req_parsed) = semver::VersionReq::parse(oreq) {
                matches_olive = olive_req_parsed.matches(&current_olive_vers);
            } else {
                matches_olive = false;
            }
        }

        if let Ok(v_parsed) = semver::Version::parse(&v.vers) {
            matches_olive && (req == "*" || req == "latest" || req_parsed.matches(&v_parsed))
        } else {
            matches_olive && (req == "*" || req == "latest" || v.vers == req)
        }
    })
}
