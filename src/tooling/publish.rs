use crate::tooling::registry::PodVersion;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

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
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("PIT_TOKEN"))
        .map_err(|_| "GITHUB_TOKEN or PIT_TOKEN env var required for publish".to_string())?;

    let gh = GhClient::new(token);

    let user_repo = resolve_user_repo()?;
    check_uncommitted_changes();

    println!("\x1b[1;32m  Packaging\x1b[0m {}@{}", name, version);
    let archive = build_archive(name, version)?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(&archive);
    let cksum = hasher.finalize().to_hex().to_string();
    println!("\x1b[1;32m  Checksum\x1b[0m {}", &cksum[..16]);

    let release_id = create_release(&gh, &user_repo, name, version)?;
    let dl_url = upload_asset(&gh, &user_repo, release_id, name, archive)?;
    println!("\x1b[1;32m  Uploaded\x1b[0m {}", dl_url);

    push_git_ref_and_tag(name, version);

    let pod = PodVersion {
        name: name.to_string(),
        vers: version.to_string(),
        deps: vec![],
        cksum,
        dl: dl_url,
        yanked: false,
        olive_req: Some(env!("CARGO_PKG_VERSION").to_string()),
    };

    let pr_url = create_registry_pr(&gh, &pod)?;
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

fn push_git_ref_and_tag(name: &str, version: &str) {
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
    if let Ok(out) = res {
        if out.status.success() {
            println!("\x1b[1;32m  Pushed\x1b[0m git branch and tag {}", tag_name);
        }
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

    if let Some(id) = resp["id"].as_u64() {
        return Ok(id);
    }

    // Release already exists — fetch it by tag.
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
    let asset_name = format!("{}.pit.zst", name);

    // Delete the old asset if it exists so we can re-upload cleanly.
    let assets_url = format!(
        "https://api.github.com/repos/{}/releases/{}/assets",
        repo, release_id
    );
    if let Ok(resp) = gh.get(&assets_url).send() {
        if let Ok(assets) = resp.json::<Value>() {
            if let Some(arr) = assets.as_array() {
                for asset in arr {
                    if asset["name"].as_str() == Some(&asset_name) {
                        if let Some(id) = asset["id"].as_u64() {
                            let _ = gh
                                .client
                                .delete(&format!(
                                    "https://api.github.com/repos/{}/releases/assets/{}",
                                    repo, id
                                ))
                                .header("Authorization", format!("token {}", gh.token))
                                .header("User-Agent", "pit/0.1.0")
                                .send();
                        }
                    }
                }
            }
        }
    }

    let upload_url = format!(
        "https://uploads.github.com/repos/{}/releases/{}/assets?name={}",
        repo, release_id, asset_name
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

    // Find out what the fork's default branch is called (could be master or main).
    let fork_default_branch = gh
        .get(&format!("https://api.github.com/repos/{}", fork_repo))
        .send()
        .ok()
        .and_then(|r| r.json::<Value>().ok())
        .and_then(|v| v["default_branch"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "master".to_string());

    // Sync fork with upstream first so we get the latest registry state.
    let _ = gh
        .post(&format!(
            "https://api.github.com/repos/{}/merge-upstream",
            fork_repo
        ))
        .json(&json!({ "branch": fork_default_branch }))
        .send();

    // Retry up to 20s — a freshly created fork can take a moment to populate.
    let base_sha = {
        let mut sha: Option<String> = None;
        for _ in 0..10 {
            if let Ok(resp) = gh
                .get(&format!(
                    "https://api.github.com/repos/{}/branches/{}",
                    fork_repo, fork_default_branch
                ))
                .send()
            {
                if let Ok(val) = resp.json::<Value>() {
                    if let Some(s) = val["commit"]["sha"].as_str() {
                        sha = Some(s.to_string());
                        break;
                    }
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
        sha.ok_or(
            "could not get fork main SHA — fork may still be initializing, try again in a moment",
        )?
    };

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
            "base": fork_default_branch,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
