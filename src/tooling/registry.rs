use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const MAX_REGISTRY_BYTES: usize = 4 * 1024 * 1024;

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

    let configured = if let Ok(content) = fs::read_to_string(config_path)
        && let Ok(config) = toml::from_str::<PitConfig>(&content)
        && let Some(reg) = config.registry
        && let Some(url) = reg.url
    {
        url
    } else {
        "https://raw.githubusercontent.com/ecnivslabs/pit-registry/master".to_string()
    };
    configured.trim_end_matches('/').to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistryCache {
    registry: String,
    body: String,
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
    /// Present only when this pod version declares `[native]`. Absent for
    /// every pod published before this field existed, and for pods that
    /// don't ship a native library at all, in which case the field is
    /// omitted from the serialized line rather than written as `null` so
    /// existing registry entries are untouched byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dep {
    pub name: String,
    pub req: String,
}

/// A pod's native library, one prebuilt artifact per target it publishes for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeSpec {
    pub lib: String,
    /// Keyed by target (see `tooling::target::SUPPORTED`). A `BTreeMap`
    /// rather than a `HashMap` so the serialized registry line has a
    /// deterministic key order and a stable, reviewable diff.
    pub artifacts: BTreeMap<String, NativeArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeArtifact {
    pub file: String,
    pub url: String,
    pub cksum: String,
    /// MSVC import library, present only for `windows-*` targets: `link.exe`
    /// cannot link directly against a `.dll`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implib: Option<NativeFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeFile {
    pub file: String,
    pub url: String,
    pub cksum: String,
}

fn validate_component(value: &str, label: &str, allow_plus: bool) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 255
        || value == "."
        || value == ".."
        || value.contains("..")
        || value.contains("@{")
    {
        return Err(format!("invalid {label}: {value:?}"));
    }

    let valid_char = |ch: char| {
        if ch.is_ascii() {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '_' | '-' | '.' | '@')
                || (allow_plus && ch == '+')
        } else {
            !ch.is_control() && !matches!(ch, '/' | '\\' | '?' | '#' | '%')
        }
    };
    if !value.chars().all(valid_char) || value.ends_with('.') || value.ends_with(' ') {
        return Err(format!("invalid {label}: {value:?}"));
    }

    if crate::tooling::windows_reserved_stem(value) {
        return Err(format!("invalid {label}: {value:?}"));
    }

    Ok(())
}

pub(crate) fn validate_pod_name(name: &str) -> Result<(), String> {
    validate_component(name, "pod name", false)
}

fn validate_version(version: &str) -> Result<(), String> {
    validate_component(version, "pod version", true)
}

fn validate_filename(filename: &str) -> Result<(), String> {
    validate_component(filename, "native filename", false)
}

pub(crate) fn validate_pod_coordinates(name: &str, version: &str) -> Result<(), String> {
    validate_pod_name(name)?;
    validate_version(version)
}

fn validate_pod_version(version: &PodVersion) -> Result<(), String> {
    validate_pod_name(&version.name)?;
    validate_version(&version.vers)?;
    for dependency in &version.deps {
        validate_pod_name(&dependency.name)?;
    }
    if let Some(native) = &version.native {
        validate_filename(&native.lib)?;
        for artifact in native.artifacts.values() {
            validate_filename(&artifact.file)?;
            if let Some(import_library) = &artifact.implib {
                validate_filename(&import_library.file)?;
            }
        }
    }
    Ok(())
}

fn registry_url(name: &str) -> Result<String, String> {
    validate_pod_name(name)?;
    let prefix = name.chars().take(2).collect::<String>();
    let base = get_registry_base();
    Ok(format!("{}/{}/{}", base, prefix, name))
}

fn cache_path(name: &str) -> PathBuf {
    let digest = blake3::hash(name.as_bytes()).to_hex().to_string();
    dirs::home_dir()
        .expect("no home dir")
        .join(".pit")
        .join("cache")
        .join("registry")
        .join(format!("{digest}.json"))
}

fn read_cached(name: &str) -> Result<String, String> {
    let path = cache_path(name);
    let content = fs::read(&path)
        .map_err(|error| format!("cache read failed for '{}': {error}", path.display()))?;
    let cache: RegistryCache = serde_json::from_slice(&content)
        .map_err(|error| format!("invalid registry cache {}: {error}", path.display()))?;
    let expected = get_registry_base();
    if cache.registry != expected {
        return Err(format!(
            "registry cache for '{name}' belongs to {}, not {expected}",
            cache.registry
        ));
    }
    Ok(cache.body)
}

fn write_cached(name: &str, body: &str) -> Result<(), String> {
    let path = cache_path(name);
    let parent = path
        .parent()
        .ok_or_else(|| "registry cache path has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create registry cache directory: {error}"))?;
    let encoded = serde_json::to_vec(&RegistryCache {
        registry: get_registry_base(),
        body: body.to_string(),
    })
    .map_err(|error| error.to_string())?;
    for _ in 0..16 {
        let temp = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name().unwrap().to_string_lossy(),
            rand::random::<u64>()
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create registry cache temp: {error}")),
        };
        if let Err(error) = file.write_all(&encoded).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temp);
            return Err(format!("cannot write registry cache: {error}"));
        }
        return fs::rename(&temp, &path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            format!("cannot replace registry cache: {error}")
        });
    }
    Err("cannot allocate a unique registry cache temp file".to_string())
}

pub async fn fetch_versions(name: &str, offline: bool) -> Result<Vec<PodVersion>, String> {
    validate_pod_name(name)?;
    if offline {
        let body = read_cached(name).map_err(|_| {
            format!("offline mode: pod '{name}' not found in matching registry cache")
        })?;
        return parse_versions_for(&body, name);
    }

    let url = registry_url(name)?;
    let client = reqwest::Client::new();
    let mut response = match client
        .get(&url)
        .header("User-Agent", "pit/0.1.0")
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let cached = read_cached(name)
                .map_err(|_| format!("registry fetch failed for '{name}': {error}"))?;
            return parse_versions_for(&cached, name);
        }
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("pod '{name}' not found in registry"));
    }
    if !response.status().is_success() {
        return Err(format!(
            "registry returned HTTP {} for pod '{name}'",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_REGISTRY_BYTES as u64)
    {
        return Err(format!("registry metadata for '{name}' exceeds 4 MiB"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("registry read failed for '{name}': {error}"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_REGISTRY_BYTES {
            return Err(format!("registry metadata for '{name}' exceeds 4 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(bytes)
        .map_err(|_| format!("registry metadata for '{name}' is not UTF-8"))?;
    let versions = parse_versions_for(&body, name)?;
    write_cached(name, &body)?;
    Ok(versions)
}

fn parse_versions(body: &str) -> Result<Vec<PodVersion>, String> {
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let version: PodVersion = serde_json::from_str(line).map_err(|e| e.to_string())?;
            validate_pod_version(&version)?;
            Ok(version)
        })
        .collect()
}

fn parse_versions_for(body: &str, name: &str) -> Result<Vec<PodVersion>, String> {
    let versions = parse_versions(body)?;
    if versions.iter().any(|version| version.name != name) {
        return Err(format!(
            "registry returned a version for a different pod than {name:?}"
        ));
    }
    Ok(versions)
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
            native: None,
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
        let url = registry_url("mypod").unwrap();
        assert!(url.contains("/my/mypod"));
    }

    #[test]
    fn registry_url_single_char_name() {
        let url = registry_url("a").unwrap();
        assert!(url.contains("/a/a"));
    }

    #[test]
    fn registry_url_handles_multibyte_name_without_panicking() {
        let url = std::panic::catch_unwind(|| registry_url("€"))
            .unwrap()
            .unwrap();
        assert!(url.ends_with("/€/€"));
    }

    #[test]
    fn parse_versions_rejects_path_traversal_name() {
        let line = r#"{"name":"../escape","vers":"1.0.0","cksum":"a","dl":""}"#;
        assert!(parse_versions(line).is_err());
    }

    #[test]
    fn parse_versions_rejects_path_traversal_version() {
        let line = r#"{"name":"safe","vers":"../../escape","cksum":"a","dl":""}"#;
        assert!(parse_versions(line).is_err());
    }

    #[test]
    fn parse_versions_rejects_unsafe_native_filename() {
        let line = r#"{"name":"safe","vers":"1.0.0","cksum":"a","dl":"","native":{"lib":"safe","artifacts":{"linux-x86_64":{"file":"../../escape.so","url":"https://example.com/a","cksum":"b"}}}}"#;
        assert!(parse_versions(line).is_err());
    }

    #[test]
    fn parse_versions_for_rejects_name_mismatch() {
        let line = r#"{"name":"other","vers":"1.0.0","cksum":"a","dl":""}"#;
        assert!(parse_versions_for(line, "safe").is_err());
    }

    #[test]
    fn registry_url_handles_combining_and_emoji_names() {
        for name in ["e\u{301}", "東京", "😀"] {
            let result = std::panic::catch_unwind(|| registry_url(name));
            assert!(result.is_ok(), "panic for {name}");
            assert!(result.unwrap().is_ok(), "invalid name {name}");
        }
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
        assert!(pod.native.is_none());
    }

    #[test]
    fn pod_version_without_native_serializes_identically_to_before_the_field_existed() {
        let pod = make_version("test", "1.0.0", "cksum123", false);
        let json = serde_json::to_string(&pod).unwrap();
        assert!(
            !json.contains("native"),
            "a non-native pod's line must not gain a native key: {json}"
        );
    }

    #[test]
    fn pod_version_with_native_round_trips() {
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            "linux-x86_64".to_string(),
            NativeArtifact {
                file: "libtokenizer-linux-x86_64.so".to_string(),
                url: "https://example.com/libtokenizer-linux-x86_64.so".to_string(),
                cksum: "abc".to_string(),
                implib: None,
            },
        );
        artifacts.insert(
            "windows-x86_64".to_string(),
            NativeArtifact {
                file: "libtokenizer-windows-x86_64.dll".to_string(),
                url: "https://example.com/libtokenizer-windows-x86_64.dll".to_string(),
                cksum: "def".to_string(),
                implib: Some(NativeFile {
                    file: "libtokenizer-windows-x86_64.dll.lib".to_string(),
                    url: "https://example.com/libtokenizer-windows-x86_64.dll.lib".to_string(),
                    cksum: "ghi".to_string(),
                }),
            },
        );
        let mut pod = make_version("tokenizer", "0.3.0", "cksum", false);
        pod.native = Some(NativeSpec {
            lib: "tokenizer".to_string(),
            artifacts,
        });

        let json = serde_json::to_string(&pod).unwrap();
        let deser: PodVersion = serde_json::from_str(&json).unwrap();
        let native = deser.native.unwrap();
        assert_eq!(native.lib, "tokenizer");
        assert_eq!(native.artifacts.len(), 2);
        assert!(native.artifacts["windows-x86_64"].implib.is_some());
        assert!(native.artifacts["linux-x86_64"].implib.is_none());
    }

    #[test]
    fn native_spec_artifact_key_order_is_deterministic() {
        let mut artifacts = BTreeMap::new();
        for key in crate::tooling::target::SUPPORTED.iter().rev() {
            artifacts.insert(
                key.to_string(),
                NativeArtifact {
                    file: format!("libx-{key}.so"),
                    url: format!("https://example.com/libx-{key}.so"),
                    cksum: "c".to_string(),
                    implib: None,
                },
            );
        }
        let spec = NativeSpec {
            lib: "x".to_string(),
            artifacts,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let mut sorted_keys: Vec<&str> = crate::tooling::target::SUPPORTED.to_vec();
        sorted_keys.sort();
        let positions: Vec<usize> = sorted_keys
            .iter()
            .map(|k| json.find(&format!("\"{k}\"")).unwrap())
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "artifact keys must serialize in sorted order: {json}"
        );
    }

    #[test]
    fn cache_paths_are_exact_name_digests() {
        let euro = cache_path("€");
        let combining = cache_path("e\u{301}");
        assert_ne!(euro, combining);
        assert!(
            euro.extension()
                .is_some_and(|extension| extension == "json")
        );
        assert!(
            euro.file_stem()
                .is_some_and(|stem| { stem.to_str().is_some_and(|value| value.len() == 64) })
        );
    }

    #[test]
    fn registry_cache_envelope_round_trips() {
        let cache = RegistryCache {
            registry: "https://registry.example/root".to_string(),
            body: "metadata".to_string(),
        };
        let encoded = serde_json::to_vec(&cache).unwrap();
        let decoded: RegistryCache = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.registry, cache.registry);
        assert_eq!(decoded.body, cache.body);
    }

    #[test]
    fn windows_device_aliases_are_rejected() {
        for name in ["CON", "com1", "LPT9", "COM¹", "lpt³"] {
            assert!(validate_pod_name(name).is_err(), "accepted {name}");
        }
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
