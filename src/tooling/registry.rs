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

    if let Ok(content) = fs::read_to_string(config_path)
        && let Ok(config) = toml::from_str::<PitConfig>(&content)
        && let Some(reg) = config.registry
        && let Some(url) = reg.url
    {
        return url;
    }
    "https://raw.githubusercontent.com/ecnivs-labs/pit-registry/main".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_version(name: &str, vers: &str, cksum: &str, yanked: bool) -> PodVersion {
        PodVersion {
            name: name.to_string(),
            vers: vers.to_string(),
            deps: vec![],
            cksum: cksum.to_string(),
            dl: format!("https://example.com/{name}/{vers}.pit.zst"),
            yanked,
            olive_req: None,
        }
    }

    fn make_version_with_req(
        name: &str,
        vers: &str,
        cksum: &str,
        yanked: bool,
        olive_req: Option<&str>,
    ) -> PodVersion {
        let mut v = make_version(name, vers, cksum, yanked);
        v.olive_req = olive_req.map(|s| s.to_string());
        v
    }

    #[test]
    fn parse_versions_single_line() {
        let line =
            r#"{"name":"test","vers":"1.0.0","cksum":"abc","dl":"https://e.com/t","yanked":false}"#;
        let versions = parse_versions(line).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].name, "test");
        assert_eq!(versions[0].vers, "1.0.0");
    }

    #[test]
    fn parse_versions_multiple_lines() {
        let data = r#"{"name":"a","vers":"1.0.0","cksum":"abc","dl":"","yanked":false}
{"name":"b","vers":"2.0.0","cksum":"def","dl":"","yanked":false}"#;
        let versions = parse_versions(data).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].name, "a");
        assert_eq!(versions[1].name, "b");
    }

    #[test]
    fn parse_versions_skips_empty_lines() {
        let data = r#"{"name":"a","vers":"1.0.0","cksum":"abc","dl":"","yanked":false}

{"name":"b","vers":"2.0.0","cksum":"def","dl":"","yanked":false}"#;
        let versions = parse_versions(data).unwrap();
        assert_eq!(versions.len(), 2);
    }

    #[test]
    fn parse_versions_empty_string() {
        let versions = parse_versions("").unwrap();
        assert!(versions.is_empty());
    }

    #[test]
    fn parse_versions_whitespace_only() {
        let versions = parse_versions("  \n  \n  ").unwrap();
        assert!(versions.is_empty());
    }

    #[test]
    fn parse_versions_invalid_json() {
        let result = parse_versions("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_version_exact_match() {
        let versions = vec![
            make_version("test", "1.0.0", "abc", false),
            make_version("test", "2.0.0", "def", false),
        ];
        let resolved = resolve_version(&versions, "1.0.0");
        assert_eq!(resolved.unwrap().vers, "1.0.0");
    }

    #[test]
    fn resolve_version_wildcard_returns_latest() {
        let versions = vec![
            make_version("test", "1.0.0", "abc", false),
            make_version("test", "2.0.0", "def", false),
        ];
        let resolved = resolve_version(&versions, "*");
        assert_eq!(resolved.unwrap().vers, "2.0.0");
    }

    #[test]
    fn resolve_version_latest_returns_last_non_yanked() {
        let versions = vec![
            make_version("test", "1.0.0", "abc", false),
            make_version("test", "2.0.0", "def", false),
            make_version("test", "3.0.0", "ghi", false),
        ];
        let resolved = resolve_version(&versions, "latest");
        assert_eq!(resolved.unwrap().vers, "3.0.0");
    }

    #[test]
    fn resolve_version_skips_yanked() {
        let versions = vec![
            make_version("test", "1.0.0", "abc", false),
            make_version("test", "2.0.0", "def", true),
            make_version("test", "3.0.0", "ghi", false),
        ];
        let resolved = resolve_version(&versions, "*");
        assert_eq!(resolved.unwrap().vers, "3.0.0");
    }

    #[test]
    fn resolve_version_all_yanked_returns_none() {
        let versions = vec![
            make_version("test", "1.0.0", "abc", true),
            make_version("test", "2.0.0", "def", true),
        ];
        assert!(resolve_version(&versions, "*").is_none());
    }

    #[test]
    fn resolve_version_no_match_returns_none() {
        let versions = vec![make_version("test", "1.0.0", "abc", false)];
        assert!(resolve_version(&versions, "3.0.0").is_none());
    }

    #[test]
    fn resolve_version_empty_list() {
        assert!(resolve_version(&[], "*").is_none());
    }

    #[test]
    fn resolve_version_with_matching_olive_req() {
        let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let req = format!(">={}.{}.0", current.major, current.minor);
        let versions = vec![
            make_version_with_req("test", "1.0.0", "abc", false, Some(&req)),
            make_version_with_req("test", "2.0.0", "def", false, Some(">=99.0.0")),
        ];
        let resolved = resolve_version(&versions, "*");
        assert_eq!(resolved.unwrap().vers, "1.0.0");
    }

    #[test]
    fn resolve_version_with_incompatible_olive_req() {
        let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let req = format!(">={}.0.0", current.major + 1);
        let versions = vec![make_version_with_req(
            "test",
            "1.0.0",
            "abc",
            false,
            Some(&req),
        )];
        assert!(resolve_version(&versions, "*").is_none());
    }

    #[test]
    fn resolve_version_with_invalid_olive_req_excludes() {
        let versions = vec![make_version_with_req(
            "test",
            "1.0.0",
            "abc",
            false,
            Some("not-a-version"),
        )];
        assert!(resolve_version(&versions, "*").is_none());
    }

    #[test]
    fn registry_url_two_char_prefix() {
        let url = registry_url("mypod");
        assert!(url.contains("/my/mypod"));
    }

    #[test]
    fn registry_url_single_char_name() {
        let url = registry_url("a");
        assert!(url.contains("/a/a"));
    }

    #[test]
    fn pod_version_roundtrip() {
        let pod = make_version("test", "1.0.0", "cksum123", false);
        let json = serde_json::to_string(&pod).unwrap();
        let deser: PodVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.name, "test");
        assert_eq!(deser.vers, "1.0.0");
        assert_eq!(deser.cksum, "cksum123");
        assert!(!deser.yanked);
    }

    #[test]
    fn pod_version_with_deps_roundtrip() {
        let mut pod = make_version("test", "1.0.0", "cksum", false);
        pod.deps = vec![Dep {
            name: "dep1".to_string(),
            req: ">=1.0".to_string(),
        }];
        let json = serde_json::to_string(&pod).unwrap();
        let deser: PodVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.deps.len(), 1);
        assert_eq!(deser.deps[0].name, "dep1");
    }

    #[test]
    fn pod_version_default_yanked_false() {
        let json = r#"{"name":"x","vers":"0.1.0","cksum":"a","dl":"","deps":[]}"#;
        let pod: PodVersion = serde_json::from_str(json).unwrap();
        assert!(!pod.yanked);
        assert!(pod.olive_req.is_none());
    }

    #[test]
    fn dep_roundtrip() {
        let dep = Dep {
            name: "mylib".to_string(),
            req: "^1.2".to_string(),
        };
        let json = serde_json::to_string(&dep).unwrap();
        let deser: Dep = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.name, "mylib");
        assert_eq!(deser.req, "^1.2");
    }
}
