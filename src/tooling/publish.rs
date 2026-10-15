use crate::tooling::manifest::Config;
use crate::tooling::registry::{
    Dep, NativeArtifact, NativeFile, NativeSpec, PodVersion, validate_pod_coordinates,
};
use crate::tooling::target;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

mod registry;

const MAX_ARCHIVE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TAR_BYTES: usize = 512 * 1024 * 1024;

const REGISTRY_REPO: &str = "ecnivslabs/pit-registry";

struct GhClient {
    token: String,
    client: reqwest::blocking::Client,
}

impl GhClient {
    fn new(token: String) -> Self {
        Self {
            token,
            client: reqwest::blocking::Client::new(),
        }
    }

    fn get(&self, url: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .get(url)
            .header("Authorization", format!("token {}", self.token))
            .header("User-Agent", "pit/0.1.0")
            .header("Accept", "application/vnd.github.v3+json")
    }

    fn post(&self, url: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .post(url)
            .header("Authorization", format!("token {}", self.token))
            .header("User-Agent", "pit/0.1.0")
            .header("Accept", "application/vnd.github.v3+json")
    }

    fn put(&self, url: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .put(url)
            .header("Authorization", format!("token {}", self.token))
            .header("User-Agent", "pit/0.1.0")
            .header("Accept", "application/vnd.github.v3+json")
    }
}

pub fn publish(name: &str, version: &str) -> Result<(), String> {
    validate_pod_coordinates(name, version)?;
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("PIT_TOKEN"))
        .map_err(|_| "GITHUB_TOKEN or PIT_TOKEN env var required for publish".to_string())?;

    let gh = GhClient::new(token);

    let source = registry::resolve_source(&gh, &resolve_user_repo()?)?;
    let registry = registry::prepare_publish(&gh, name, version, &source)?;
    let user_repo = &source.repository;
    check_uncommitted_changes();

    println!("\x1b[1;32m  Packaging\x1b[0m {}@{}", name, version);
    let manifest = read_manifest()?;
    let archive = build_archive(name, version, &manifest)?;

    if archive.len() > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "error: pod archive is {:.1} MB\n  native libraries belong in release assets, not the .pit.zst\n  declare [native] in pit.toml, or trim [pod].include",
            archive.len() as f64 / (1024.0 * 1024.0)
        ));
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(&archive);
    let cksum = hasher.finalize().to_hex().to_string();
    println!("\x1b[1;32m  Checksum\x1b[0m {}", &cksum[..16]);

    let release_id = create_release(&gh, user_repo, name, version)?;
    let dl_url = upload_asset(&gh, user_repo, release_id, name, archive)?;
    println!("\x1b[1;32m  Uploaded\x1b[0m {}", dl_url);

    let native = upload_native_artifacts(&gh, user_repo, release_id, &manifest)?;

    push_git_ref_and_tag(name, version);

    let mut deps: Vec<Dep> = manifest
        .dependencies
        .iter()
        .map(|(dep_name, req)| Dep {
            name: dep_name.clone(),
            req: req.clone(),
        })
        .collect();
    deps.sort_by(|a, b| a.name.cmp(&b.name));

    let pod = PodVersion {
        name: name.to_string(),
        vers: version.to_string(),
        deps,
        cksum,
        dl: dl_url,
        yanked: false,
        olive_req: Some(env!("CARGO_PKG_VERSION").to_string()),
        native,
    };

    let pr_url = registry::create_registry_pr(&gh, &pod, &source, &registry)?;
    println!(
        "\x1b[1;32m  Published\x1b[0m {}@{} ; registry PR: {}",
        name, version, pr_url
    );
    Ok(())
}

fn check_uncommitted_changes() {
    if let Ok(out) = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !stdout.trim().is_empty() && std::env::var("PIT_ALLOW_DIRTY").is_err() {
            eprintln!("\x1b[1;33mwarning:\x1b[0m uncommitted changes in git repository");
        }
    }
}

fn read_manifest() -> Result<Config, String> {
    let content = fs::read_to_string("pit.toml").map_err(|_| "pit.toml not found")?;
    let config: Config = toml::from_str(&content).map_err(|e| format!("invalid pit.toml: {e}"))?;
    let root = std::env::current_dir().map_err(|e| format!("cannot resolve project root: {e}"))?;
    crate::tooling::manifest::validate_pod_files(&config, &root)?;
    Ok(config)
}

fn push_git_ref_and_tag(name: &str, version: &str) {
    if std::env::var("CI").is_ok() {
        return;
    }
    let tag_name = format!("v{}", version);
    let _ = std::process::Command::new("git")
        .args([
            "tag",
            "-a",
            &tag_name,
            "-m",
            &format!("Release {} v{}", name, version),
        ])
        .output();
    let res = std::process::Command::new("git")
        .args(["push", "origin", "HEAD", "--tags"])
        .output();
    if let Ok(out) = res
        && out.status.success()
    {
        println!("\x1b[1;32m  Pushed\x1b[0m git branch and tag {}", tag_name);
    }
}

fn resolve_user_repo() -> Result<String, String> {
    git_origin_url()
        .and_then(|url| parse_github_repo(&url))
        .ok_or_else(|| {
            "cannot determine GitHub repository - add a git remote pointing to GitHub".to_string()
        })
}

fn parse_github_repo(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches(".git");
    let repository = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    registry::valid_repository_name(repository).then(|| repository.to_string())
}

fn git_origin_url() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|url| url.trim().to_string())
}

fn get_current_user(gh: &GhClient) -> Result<String, String> {
    let resp: Value = gh
        .get("https://api.github.com/user")
        .send()
        .map_err(|e| format!("auth failed: {}", e))?
        .json()
        .map_err(|e| e.to_string())?;

    resp["login"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "could not get GitHub user login".to_string())
}

fn build_archive(name: &str, version: &str, manifest: &Config) -> Result<Vec<u8>, String> {
    let prefix = format!("{}-{}", name, version);
    let mut tar_bytes: Vec<u8> = Vec::new();

    {
        let mut includes = HashSet::new();
        if let Some(pod) = &manifest.pod {
            for entry in &pod.include {
                let relative = crate::tooling::manifest::safe_relative_path(entry, "pod include")?;
                let normalized = relative.to_string_lossy().replace('\\', "/");
                if normalized == "pit.toml"
                    || normalized == "README.md"
                    || normalized == "LICENSE"
                    || normalized == "src"
                    || normalized.starts_with("src/")
                {
                    return Err(format!(
                        "error: [pod].include '{entry}' overlaps files already packed by pit"
                    ));
                }
                if !includes.insert(normalized.clone()) {
                    return Err(format!(
                        "error: [pod].include contains duplicate path '{normalized}'"
                    ));
                }
            }
        }

        let mut builder = tar::Builder::new(&mut tar_bytes);

        let toml_bytes = fs::read("pit.toml").map_err(|_| "pit.toml not found")?;
        append_bytes(&mut builder, &toml_bytes, &format!("{}/pit.toml", prefix))?;

        if Path::new("src").exists() {
            append_liv_tree(&mut builder, Path::new("src"), &format!("{}/src", prefix))?;
        }

        for candidate in ["README.md", "LICENSE"] {
            if Path::new(candidate).is_file() {
                let bytes =
                    fs::read(candidate).map_err(|e| format!("could not read {candidate}: {e}"))?;
                append_bytes(&mut builder, &bytes, &format!("{prefix}/{candidate}"))?;
            }
        }

        if let Some(pod) = &manifest.pod {
            for entry in &pod.include {
                append_include(&mut builder, &prefix, entry)?;
            }
        }

        builder.finish().map_err(|e| e.to_string())?;
    }

    if tar_bytes.len() > MAX_TAR_BYTES {
        return Err(format!(
            "pod source data exceeds the {} byte uncompressed limit",
            MAX_TAR_BYTES
        ));
    }

    zstd::encode_all(tar_bytes.as_slice(), 3).map_err(|e| e.to_string())
}

fn append_liv_tree(
    builder: &mut tar::Builder<&mut Vec<u8>>,
    src: &Path,
    tar_prefix: &str,
) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing to publish symlink outside archive: {}",
                path.display()
            ));
        }
        let tar_path = format!("{}/{}", tar_prefix, entry.file_name().to_string_lossy());
        if metadata.is_dir() {
            append_liv_tree(builder, &path, &tar_path)?;
        } else if metadata.is_file() && path.extension().is_some_and(|ext| ext == "liv") {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            append_bytes(builder, &bytes, &tar_path)?;
        } else if !metadata.is_file() {
            return Err(format!("unsupported archive input: {}", path.display()));
        }
    }
    Ok(())
}

fn append_include(
    builder: &mut tar::Builder<&mut Vec<u8>>,
    prefix: &str,
    entry: &str,
) -> Result<(), String> {
    let relative = crate::tooling::manifest::safe_relative_path(entry, "pod include")?;
    let first = relative
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .is_some_and(|name| name.eq_ignore_ascii_case("native"));
    if first {
        return Err(
            "error: [pod].include may not list native/\n  native libraries belong in release assets, not the .pit.zst"
                .to_string(),
        );
    }
    let path = Path::new(entry);
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| format!("error: [pod].include entry '{entry}' does not exist"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("error: [pod].include entry '{entry}' is a symlink"));
    }
    let tar_path = format!("{prefix}/{}", relative.to_string_lossy().replace('\\', "/"));
    if metadata.is_dir() {
        append_include_dir(builder, path, &tar_path)?;
    } else if metadata.is_file() {
        let bytes = fs::read(path).map_err(|e| format!("could not read {entry}: {e}"))?;
        append_bytes(builder, &bytes, &tar_path)?;
    } else {
        return Err(format!("error: unsupported include entry '{entry}'"));
    }
    Ok(())
}

fn append_include_dir(
    builder: &mut tar::Builder<&mut Vec<u8>>,
    src: &Path,
    tar_prefix: &str,
) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "refusing to publish symlink outside archive: {}",
                path.display()
            ));
        }
        let tar_path = format!("{}/{}", tar_prefix, entry.file_name().to_string_lossy());
        if metadata.is_dir() {
            append_include_dir(builder, &path, &tar_path)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            append_bytes(builder, &bytes, &tar_path)?;
        } else {
            return Err(format!("unsupported archive input: {}", path.display()));
        }
    }
    Ok(())
}

/// Expected artifact filenames for every declared target: the library file
/// plus the MSVC import library on Windows targets.
fn expected_native_files(
    manifest: &Config,
) -> Result<Vec<(String, String, Option<String>)>, String> {
    let Some(native) = &manifest.native else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for key in native.target_keys() {
        let file = target::asset_name(&native.lib, &key).ok_or_else(|| {
            format!("error: pit has no artifact naming convention for target '{key}'")
        })?;
        let implib = target::implib_asset_name(&native.lib, &key);
        out.push((key, file, implib));
    }
    Ok(out)
}

/// Uploads whatever finished `native/*` files are sitting next to the pod.
/// These come for free from the author's own dev loop (`pit build` stages
/// them), so publish itself stays instant: no building, no waiting, no extra
/// calls when there is nothing to upload. Targets without a file are simply
/// absent from the map; consumers build those from the published sources.
fn upload_native_artifacts(
    gh: &GhClient,
    repo: &str,
    release_id: u64,
    manifest: &Config,
) -> Result<Option<NativeSpec>, String> {
    let Some(native) = &manifest.native else {
        return Ok(None);
    };
    let mut artifacts = BTreeMap::new();
    let mut pending = Vec::new();
    let host_key = target::host();
    let host_local = target::local_name(&native.lib);
    for (key, file, implib_file) in expected_native_files(manifest)? {
        // The dev loop stages the host build under its plain local name
        // (`native/libtokenizer.so`), so accept that file for the host target
        // directly: the author's own machine feeds publish with zero extra steps.
        let path = Path::new("native").join(&file);
        let path = if !path.is_file()
            && Some(key.as_str()) == host_key
            && let Some(ref local) = host_local
            && Path::new("native").join(local).is_file()
        {
            Path::new("native").join(local)
        } else {
            path
        };
        if !path.is_file() {
            pending.push(format!("{key} (native/{file})"));
            continue;
        }
        if let Some(ref implib_name) = implib_file
            && !Path::new("native").join(implib_name).is_file()
        {
            pending.push(format!("{key} (native/{implib_name})"));
            continue;
        }
        let bytes =
            fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let cksum = blake3::hash(&bytes).to_hex().to_string();
        let url = upload_named_asset(gh, repo, release_id, &file, bytes)?;
        println!("\x1b[1;32m  Uploaded\x1b[0m {url}");
        let implib = if let Some(implib_name) = implib_file {
            let implib_path = Path::new("native").join(&implib_name);
            let implib_bytes = fs::read(&implib_path)
                .map_err(|e| format!("could not read {}: {e}", implib_path.display()))?;
            let implib_cksum = blake3::hash(&implib_bytes).to_hex().to_string();
            let implib_url = upload_named_asset(gh, repo, release_id, &implib_name, implib_bytes)?;
            println!("\x1b[1;32m  Uploaded\x1b[0m {implib_url}");
            Some(NativeFile {
                file: implib_name,
                url: implib_url,
                cksum: implib_cksum,
            })
        } else {
            None
        };
        artifacts.insert(
            key,
            NativeArtifact {
                file,
                url,
                cksum,
                implib,
            },
        );
    }
    if !pending.is_empty() {
        println!(
            "\x1b[1;33mnote:\x1b[0m no prebuilt artifact for {}; consumers there build from the published sources",
            pending.join(", ")
        );
    }
    if artifacts.is_empty() {
        println!(
            "\x1b[1;33mnote:\x1b[0m publishing without prebuilt artifacts; consumers build from source"
        );
        return Ok(None);
    }
    Ok(Some(NativeSpec {
        lib: native.lib.clone(),
        artifacts,
    }))
}

fn append_bytes(
    builder: &mut tar::Builder<&mut Vec<u8>>,
    bytes: &[u8],
    path: &str,
) -> Result<(), String> {
    let output = builder.get_mut();
    if output.len().saturating_add(bytes.len()).saturating_add(512) > MAX_TAR_BYTES {
        return Err(format!(
            "pod source data exceeds the {} byte uncompressed limit",
            MAX_TAR_BYTES
        ));
    }
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .map_err(|e| e.to_string())
}

fn create_release(gh: &GhClient, repo: &str, name: &str, version: &str) -> Result<u64, String> {
    let tag = format!("{}-{}", name, version);
    let url = format!("https://api.github.com/repos/{}/releases", repo);

    let resp: Value = gh
        .post(&url)
        .json(&json!({
            "tag_name": tag,
            "name": format!("{} v{}", name, version),
            "draft": false,
            "prerelease": false,
        }))
        .send()
        .map_err(|e| format!("create release failed: {}", e))?
        .json()
        .map_err(|e| e.to_string())?;

    if let Some(id) = resp["id"].as_u64() {
        return Ok(id);
    }

    // Release already exists, so fetch it by tag.
    let existing: Value = gh
        .get(&format!(
            "https://api.github.com/repos/{}/releases/tags/{}",
            repo, tag
        ))
        .send()
        .map_err(|e| format!("fetch existing release failed: {}", e))?
        .json()
        .map_err(|e| e.to_string())?;

    existing["id"]
        .as_u64()
        .ok_or_else(|| format!("unexpected GitHub response: {}", resp))
}

fn upload_asset(
    gh: &GhClient,
    repo: &str,
    release_id: u64,
    name: &str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    upload_named_asset(gh, repo, release_id, &format!("{name}.pit.zst"), bytes)
}

fn release_asset_name(
    existing: &[Value],
    requested: &str,
    data: &[u8],
) -> Result<(String, Option<String>), String> {
    if !existing
        .iter()
        .any(|asset| asset["name"].as_str() == Some(requested))
    {
        return Ok((requested.to_string(), None));
    }
    let base = format!("olive-{}", blake3::hash(data).to_hex());
    for suffix in 0..=existing.len() {
        let alternate = if suffix == 0 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        let Some(asset) = existing
            .iter()
            .find(|asset| asset["name"].as_str() == Some(alternate.as_str()))
        else {
            return Ok((alternate, None));
        };
        if asset["state"] == "uploaded" {
            let url = asset["browser_download_url"]
                .as_str()
                .ok_or_else(|| format!("release asset {alternate} has no download URL"))?;
            return Ok((alternate, Some(url.to_string())));
        }
    }
    Err("release asset names are exhausted".to_string())
}

fn upload_named_asset(
    gh: &GhClient,
    repo: &str,
    release_id: u64,
    asset_name: &str,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let mut existing = Vec::new();
    for page in 1..=1000 {
        let assets_url = format!(
            "https://api.github.com/repos/{repo}/releases/{release_id}/assets?per_page=100&page={page}"
        );
        let response = gh
            .get(&assets_url)
            .send()
            .map_err(|error| format!("could not list release assets: {error}"))?
            .error_for_status()
            .map_err(|error| format!("could not list release assets: {error}"))?;
        let page_assets: Vec<Value> = response
            .json()
            .map_err(|error| format!("invalid release asset list: {error}"))?;
        let count = page_assets.len();
        existing.extend(page_assets);
        if count < 100 {
            break;
        }
        if page == 1000 {
            return Err("release asset list exceeds 100,000 entries".to_string());
        }
    }
    let (upload_name, existing_url) = release_asset_name(&existing, asset_name, &bytes)?;
    if let Some(url) = existing_url {
        return Ok(url);
    }

    let upload_url = format!(
        "https://uploads.github.com/repos/{}/releases/{}/assets?name={}",
        repo, release_id, upload_name
    );

    let resp: Value = gh
        .client
        .post(&upload_url)
        .header("Authorization", format!("token {}", gh.token))
        .header("User-Agent", "pit/0.1.0")
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .send()
        .map_err(|e| format!("asset upload failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("asset upload failed: {}", e))?
        .json()
        .map_err(|e| e.to_string())?;

    resp["browser_download_url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("upload failed: {}", resp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn republish_keeps_old_asset_and_uses_content_address() {
        let requested = "example.pit.zst";
        let old = json!({"name": requested, "browser_download_url": "https://example.com/old"});
        let (alternate, reused) =
            release_asset_name(std::slice::from_ref(&old), requested, b"new").unwrap();
        assert!(alternate.starts_with("olive-"));
        assert_ne!(alternate, requested);
        assert!(reused.is_none());
        let new = json!({"name": alternate, "state": "uploaded", "browser_download_url": "https://example.com/new"});
        let (again, reused) = release_asset_name(&[old, new], requested, b"new").unwrap();
        assert_eq!(again, alternate);
        assert_eq!(reused.as_deref(), Some("https://example.com/new"));
    }

    #[test]
    fn republish_skips_incomplete_asset() {
        let requested = "example.pit.zst";
        let old = json!({"name": requested, "state": "uploaded"});
        let (alternate, _) =
            release_asset_name(std::slice::from_ref(&old), requested, b"new").unwrap();
        let incomplete = json!({"name": alternate, "state": "starter", "browser_download_url": "https://example.com/incomplete"});
        let (next, reused) = release_asset_name(&[old, incomplete], requested, b"new").unwrap();
        assert_ne!(next, alternate);
        assert!(next.ends_with("-1"));
        assert!(reused.is_none());
    }

    #[test]
    fn parse_github_repo_https() {
        assert_eq!(
            parse_github_repo("https://github.com/user/repo.git"),
            Some("user/repo".to_string())
        );
    }

    #[test]
    fn parse_github_repo_ssh() {
        assert_eq!(
            parse_github_repo("git@github.com:user/repo.git"),
            Some("user/repo".to_string())
        );
    }

    #[test]
    fn parse_github_repo_no_git_suffix() {
        assert_eq!(
            parse_github_repo("https://github.com/user/repo"),
            Some("user/repo".to_string())
        );
    }

    #[test]
    fn parse_github_repo_non_github() {
        assert_eq!(parse_github_repo("https://gitlab.com/user/repo"), None);
    }

    #[test]
    fn parse_github_repo_invalid() {
        assert_eq!(parse_github_repo("not-a-url"), None);
    }

    #[test]
    fn parse_github_repo_empty() {
        assert_eq!(parse_github_repo(""), None);
    }

    fn tar_names(bytes: &[u8]) -> Vec<String> {
        let decoded = zstd::decode_all(bytes).unwrap();
        let mut archive = tar::Archive::new(decoded.as_slice());
        archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn liv_tree_packs_only_liv_files() {
        let dir = std::env::temp_dir().join("olive_publish_liv_only");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src").join("sub")).unwrap();
        std::fs::write(dir.join("src").join("lib.liv"), "fn main():\n    pass").unwrap();
        std::fs::write(dir.join("src").join("engine.rs"), "rust").unwrap();
        std::fs::write(dir.join("src").join("sub").join("mod.liv"), "x").unwrap();
        std::fs::write(dir.join("src").join("sub").join("helper.rs"), "y").unwrap();

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            append_liv_tree(&mut builder, &dir.join("src"), "pkg-1.0/src").unwrap();
            builder.finish().unwrap();
        }
        let names = tar_names(&zstd::encode_all(tar_bytes.as_slice(), 3).unwrap());
        assert!(names.iter().any(|n| n.ends_with("src/lib.liv")));
        assert!(names.iter().any(|n| n.ends_with("src/sub/mod.liv")));
        assert!(!names.iter().any(|n| n.ends_with(".rs")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn include_rejects_native_directory() {
        let mut tar_bytes = Vec::new();
        let mut builder = tar::Builder::new(&mut tar_bytes);
        assert!(append_include(&mut builder, "pkg-1.0", "native").is_err());
        assert!(append_include(&mut builder, "pkg-1.0", "native/libfoo.so").is_err());
    }

    #[test]
    fn include_missing_entry_errors() {
        let mut tar_bytes = Vec::new();
        let mut builder = tar::Builder::new(&mut tar_bytes);
        assert!(append_include(&mut builder, "pkg-1.0", "does-not-exist.txt").is_err());
    }

    #[test]
    fn include_dir_packs_everything_under_it() {
        let dir = std::env::temp_dir().join("olive_publish_include_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("engine").join("sub")).unwrap();
        std::fs::write(dir.join("engine").join("lib.rs"), "rust").unwrap();
        std::fs::write(dir.join("engine").join("sub").join("mod.rs"), "more").unwrap();
        std::fs::write(dir.join("engine").join("notes.txt"), "txt").unwrap();

        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            append_include(&mut builder, "pkg-1.0", "engine").unwrap();
            builder.finish().unwrap();
        }
        std::env::set_current_dir(&cwd).unwrap();
        let names = tar_names(&zstd::encode_all(tar_bytes.as_slice(), 3).unwrap());
        assert!(names.iter().any(|n| n.ends_with("engine/lib.rs")));
        assert!(names.iter().any(|n| n.ends_with("engine/sub/mod.rs")));
        assert!(names.iter().any(|n| n.ends_with("engine/notes.txt")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn include_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join("olive_publish_include_symlink");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let outside = dir.join("outside.txt");
        std::fs::write(&outside, b"secret").unwrap();
        symlink(&outside, dir.join("linked.txt")).unwrap();

        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let mut tar_bytes = Vec::new();
        let result = {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            append_include(&mut builder, "pkg-1.0", "linked.txt")
        };
        std::env::set_current_dir(cwd).unwrap();
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
