use crate::tooling::registry::PodVersion;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

const REGISTRY_REPO: &str = "olive-language/pit-registry";

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
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("PIT_TOKEN"))
        .map_err(|_| "GITHUB_TOKEN or PIT_TOKEN env var required for publish".to_string())?;

    let gh = GhClient::new(token);

    let user_repo = resolve_user_repo()?;

    println!("\x1b[1;32m  Packaging\x1b[0m {}@{}", name, version);
    let archive = build_archive(name, version)?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(&archive);
    let cksum = hasher.finalize().to_hex().to_string();
    println!("\x1b[1;32m  Checksum\x1b[0m {}", &cksum[..16]);

    let release_id = create_release(&gh, &user_repo, name, version)?;
    let dl_url = upload_asset(&gh, &user_repo, release_id, name, archive)?;
    println!("\x1b[1;32m  Uploaded\x1b[0m {}", dl_url);

    let pod = PodVersion {
        name: name.to_string(),
        vers: version.to_string(),
        deps: vec![],
        cksum,
        dl: dl_url,
        yanked: false,
    };

    let pr_url = create_registry_pr(&gh, &pod)?;
    println!(
        "\x1b[1;32m  Published\x1b[0m {}@{} ; registry PR: {}",
        name, version, pr_url
    );
    Ok(())
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

    if let Some(rest) = url.strip_prefix("https://github.com/") {
        return Some(rest.to_string());
    }
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return Some(rest.to_string());
    }
    None
}

fn git_origin_url() -> Option<String> {
    let config = fs::read_to_string(".git/config").ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed == "[remote \"origin\"]" {
            in_origin = true;
        } else if in_origin && trimmed.starts_with("url = ") {
            return Some(trimmed.strip_prefix("url = ")?.to_string());
        } else if trimmed.starts_with('[') {
            in_origin = false;
        }
    }
    None
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

fn build_archive(name: &str, version: &str) -> Result<Vec<u8>, String> {
    let prefix = format!("{}-{}", name, version);
    let mut tar_bytes: Vec<u8> = Vec::new();

    {
        let mut builder = tar::Builder::new(&mut tar_bytes);

        let toml_bytes = fs::read("pit.toml").map_err(|_| "pit.toml not found")?;
        append_bytes(&mut builder, &toml_bytes, &format!("{}/pit.toml", prefix))?;

        if Path::new("src").exists() {
            append_dir(&mut builder, Path::new("src"), &format!("{}/src", prefix))?;
        }

        builder.finish().map_err(|e| e.to_string())?;
    }

    zstd::encode_all(tar_bytes.as_slice(), 3).map_err(|e| e.to_string())
}

fn append_bytes(
    builder: &mut tar::Builder<&mut Vec<u8>>,
    bytes: &[u8],
    path: &str,
) -> Result<(), String> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .map_err(|e| e.to_string())
}

fn append_dir(
    builder: &mut tar::Builder<&mut Vec<u8>>,
    src: &Path,
    tar_prefix: &str,
) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let tar_path = format!("{}/{}", tar_prefix, entry.file_name().to_string_lossy());
        if path.is_dir() {
            append_dir(builder, &path, &tar_path)?;
        } else {
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            append_bytes(builder, &bytes, &tar_path)?;
        }
    }
    Ok(())
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

    resp["id"]
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
    let asset_name = format!("{}.pit.zst", name);
    let url = format!(
        "https://uploads.github.com/repos/{}/releases/{}/assets?name={}",
        repo, release_id, asset_name
    );

    let resp: Value = gh
        .client
        .post(&url)
        .header("Authorization", format!("token {}", gh.token))
        .header("User-Agent", "pit/0.1.0")
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .send()
        .map_err(|e| format!("asset upload failed: {}", e))?
        .json()
        .map_err(|e| e.to_string())?;

    resp["browser_download_url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("upload failed: {}", resp))
}

fn ensure_fork(gh: &GhClient, user: &str) -> Result<String, String> {
    let fork_repo = format!("{}/pit-registry", user);

    if gh
        .get(&format!("https://api.github.com/repos/{}", fork_repo))
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
    {
        return Ok(fork_repo);
    }

    println!(
        "\x1b[1;32m    Forking\x1b[0m {} → {}",
        REGISTRY_REPO, fork_repo
    );

    gh.post(&format!(
        "https://api.github.com/repos/{}/forks",
        REGISTRY_REPO
    ))
    .send()
    .map_err(|e| format!("fork failed: {}", e))?;

    for _ in 0..15 {
        thread::sleep(Duration::from_secs(2));
        if gh
            .get(&format!("https://api.github.com/repos/{}", fork_repo))
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return Ok(fork_repo);
        }
    }

    Err(format!(
        "fork not ready after 30s; check https://github.com/{} and retry",
        fork_repo
    ))
}

fn create_registry_pr(gh: &GhClient, pod: &PodVersion) -> Result<String, String> {
    let user = get_current_user(gh)?;
    let fork_repo = ensure_fork(gh, &user)?;

    let prefix = &pod.name[..pod.name.len().min(2)];
    let file_path = format!("{}/{}", prefix, pod.name);
    let branch = format!("add-{}-{}", pod.name, pod.vers);

    let (current_sha_on_fork, current_content) = match gh
        .get(&format!(
            "https://api.github.com/repos/{}/contents/{}",
            fork_repo, file_path
        ))
        .send()
    {
        Ok(resp) => {
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                (None, String::new())
            } else {
                let val: Value = resp.json().map_err(|e| e.to_string())?;
                let sha = val["sha"].as_str().unwrap_or("").to_string();
                let content = val["content"]
                    .as_str()
                    .map(|c| {
                        let cleaned = c.replace('\n', "");
                        String::from_utf8(B64.decode(cleaned).unwrap_or_default())
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                (Some(sha), content)
            }
        }
        Err(e) => return Err(format!("registry read failed: {}", e)),
    };

    let new_line = serde_json::to_string(pod).map_err(|e| e.to_string())?;
    let new_content = if current_content.trim().is_empty() {
        new_line
    } else {
        format!("{}\n{}", current_content.trim_end(), new_line)
    };

    let fork_main: Value = gh
        .get(&format!(
            "https://api.github.com/repos/{}/git/refs/heads/main",
            fork_repo
        ))
        .send()
        .map_err(|e| format!("get fork main failed: {}", e))?
        .json()
        .map_err(|e| e.to_string())?;

    let base_sha = fork_main["object"]["sha"]
        .as_str()
        .ok_or("could not get fork main SHA")?;

    gh.post(&format!(
        "https://api.github.com/repos/{}/git/refs",
        fork_repo
    ))
    .json(&json!({
        "ref": format!("refs/heads/{}", branch),
        "sha": base_sha,
    }))
    .send()
    .map_err(|e| format!("create branch failed: {}", e))?;

    let fork_file_url = format!(
        "https://api.github.com/repos/{}/contents/{}",
        fork_repo, file_path
    );
    let encoded = B64.encode(new_content.as_bytes());
    let mut update_body = json!({
        "message": format!("add {}@{}", pod.name, pod.vers),
        "content": encoded,
        "branch": branch,
    });
    if let Some(sha) = current_sha_on_fork {
        update_body["sha"] = json!(sha);
    }

    gh.put(&fork_file_url)
        .json(&update_body)
        .send()
        .map_err(|e| format!("registry update failed: {}", e))?;

    let pr_resp: Value = gh
        .post(&format!(
            "https://api.github.com/repos/{}/pulls",
            REGISTRY_REPO
        ))
        .json(&json!({
            "title": format!("Add {}@{}", pod.name, pod.vers),
            "body": format!(
                "New pod: **{}** version `{}`\n\nPublished via `pit publish`.",
                pod.name, pod.vers
            ),
            "head": format!("{}:{}", user, branch),
            "base": "main",
        }))
        .send()
        .map_err(|e| format!("create PR failed: {}", e))?
        .json()
        .map_err(|e| e.to_string())?;

    pr_resp["html_url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("PR created but no URL in response: {}", pr_resp))
}
