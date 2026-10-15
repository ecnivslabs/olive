use super::{GhClient, PodVersion, REGISTRY_REPO, get_current_user};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::thread;
use std::time::Duration;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceRepository {
    pub repository_id: u64,
    pub repository: String,
}

const MAX_METADATA_BYTES: u64 = 4 * 1024 * 1024;

pub(super) struct RegistrySnapshot {
    base_sha: String,
    base_branch: String,
    file_sha: Option<String>,
    content: String,
    new_package: bool,
}

pub(super) fn valid_repository_name(repository: &str) -> bool {
    let parts: Vec<_> = repository.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.'))
        })
}

fn request_json(
    request: reqwest::blocking::RequestBuilder,
    context: &str,
) -> Result<Value, String> {
    request
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("{context}: {error}"))?
        .json()
        .map_err(|error| format!("{context}: invalid GitHub response: {error}"))
}

pub(super) fn resolve_source(gh: &GhClient, repository: &str) -> Result<SourceRepository, String> {
    let value = request_json(
        gh.get(&format!("https://api.github.com/repos/{repository}")),
        "could not resolve source repository",
    )?;
    let source = SourceRepository {
        repository_id: value["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or("source repository response has no valid repository ID")?,
        repository: value["full_name"]
            .as_str()
            .filter(|name| valid_repository_name(name))
            .ok_or("source repository response has no valid repository name")?
            .to_string(),
    };
    Ok(source)
}

fn read_file(
    gh: &GhClient,
    repository: &str,
    path: &str,
    reference: &str,
) -> Result<Option<(String, String)>, String> {
    let response = gh
        .get(&format!(
            "https://api.github.com/repos/{repository}/contents/{path}"
        ))
        .query(&[("ref", reference)])
        .send()
        .map_err(|error| format!("could not read registry file {path}: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let value: Value = response
        .error_for_status()
        .map_err(|error| format!("could not read registry file {path}: {error}"))?
        .json()
        .map_err(|error| format!("invalid registry response for {path}: {error}"))?;
    decode_registry_file(&value, path, |sha| {
        request_json(
            gh.get(&format!(
                "https://api.github.com/repos/{repository}/git/blobs/{sha}"
            )),
            &format!("could not read registry blob for {path}"),
        )
    })
    .map(Some)
}

fn decode_registry_file(
    value: &Value,
    path: &str,
    read_blob: impl FnOnce(&str) -> Result<Value, String>,
) -> Result<(String, String), String> {
    if value["type"] != "file" {
        return Err(format!("registry path {path} is not a regular file"));
    }
    metadata_size(value, path)?;
    let sha = value["sha"]
        .as_str()
        .filter(|sha| !sha.is_empty() && sha.bytes().all(|ch| ch.is_ascii_hexdigit()))
        .ok_or_else(|| format!("registry file {path} has no valid SHA"))?;
    let content = if value["encoding"] == "none" {
        let blob = read_blob(sha)?;
        if blob["sha"].as_str() != Some(sha) {
            return Err(format!("registry blob SHA does not match {path}"));
        }
        if metadata_size(&blob, path)? != metadata_size(value, path)? {
            return Err(format!("registry blob size does not match {path}"));
        }
        decode_content(&blob, path)?
    } else {
        decode_content(value, path)?
    };
    Ok((sha.to_string(), content))
}

fn metadata_size(value: &Value, path: &str) -> Result<u64, String> {
    let size = value["size"]
        .as_u64()
        .ok_or_else(|| format!("registry file {path} has no valid size"))?;
    if size > MAX_METADATA_BYTES {
        return Err(format!("registry file {path} exceeds 4 MiB"));
    }
    Ok(size)
}

fn decode_content(value: &Value, path: &str) -> Result<String, String> {
    let size = metadata_size(value, path)?;
    if value["encoding"] != "base64" {
        return Err(format!(
            "registry file {path} has unsupported content encoding"
        ));
    }
    let encoded = value["content"]
        .as_str()
        .ok_or_else(|| format!("registry file {path} has no content"))?;
    let bytes = B64
        .decode(encoded.replace(['\n', '\r'], ""))
        .map_err(|error| format!("invalid registry encoding for {path}: {error}"))?;
    if bytes.len() as u64 != size {
        return Err(format!("registry file {path} does not match declared size"));
    }
    String::from_utf8(bytes).map_err(|error| format!("registry file {path} is not UTF-8: {error}"))
}

fn check_publish_state(
    content: Option<&str>,
    ownership: Option<&str>,
    name: &str,
    version: &str,
    source: &SourceRepository,
) -> Result<bool, String> {
    if let Some(content) = content {
        for (line_number, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(line).map_err(|error| {
                format!(
                    "invalid registry record for {name} on line {}: {error}",
                    line_number + 1
                )
            })?;
            if record["name"].as_str() != Some(name) || !record["vers"].is_string() {
                return Err(format!("invalid registry record for {name}"));
            }
            if record["vers"].as_str() == Some(version) {
                return Err(format!(
                    "{name}@{version} is already registered; publish a new version"
                ));
            }
        }
        if content.trim().is_empty() {
            return Err(format!("registered pod {name} has an empty index"));
        }
    }
    match (content, ownership) {
        (Some(_), None) => Err(format!(
            "registered pod {name} has no repository binding; registry maintainer must migrate its ownership"
        )),
        (None, Some(_)) => Err(format!(
            "pod {name} has a repository binding but no registry index"
        )),
        (_, Some(ownership)) => {
            let binding: SourceRepository = serde_json::from_str(ownership)
                .map_err(|error| format!("invalid ownership binding for {name}: {error}"))?;
            if binding.repository_id == 0 || !valid_repository_name(&binding.repository) {
                return Err(format!("invalid ownership binding for {name}"));
            }
            if binding.repository_id != source.repository_id {
                return Err(format!(
                    "pod {name} belongs to repository ID {}, source {} has ID {}",
                    binding.repository_id, source.repository, source.repository_id
                ));
            }
            Ok(false)
        }
        (None, None) => Ok(true),
    }
}

pub(super) fn prepare_publish(
    gh: &GhClient,
    name: &str,
    version: &str,
    source: &SourceRepository,
) -> Result<RegistrySnapshot, String> {
    let repository = request_json(
        gh.get(&format!("https://api.github.com/repos/{REGISTRY_REPO}")),
        "could not read registry repository",
    )?;
    let base_branch = repository["default_branch"]
        .as_str()
        .ok_or("registry repository has no default branch")?
        .to_string();
    let branch = request_json(
        gh.get(&format!(
            "https://api.github.com/repos/{REGISTRY_REPO}/commits"
        ))
        .query(&[("sha", base_branch.as_str()), ("per_page", "1")]),
        "could not read registry base commit",
    )?;
    let base_sha = branch[0]["sha"]
        .as_str()
        .ok_or("registry base response has no commit SHA")?
        .to_string();
    let prefix: String = name.chars().take(2).collect();
    let index = read_file(gh, REGISTRY_REPO, &format!("{prefix}/{name}"), &base_sha)?;
    let owner = read_file(gh, REGISTRY_REPO, &format!("owners/{name}.json"), &base_sha)?;
    let new_package = check_publish_state(
        index.as_ref().map(|(_, content)| content.as_str()),
        owner.as_ref().map(|(_, content)| content.as_str()),
        name,
        version,
        source,
    )?;
    let (file_sha, content) = match index {
        Some((sha, content)) => (Some(sha), content),
        None => (None, String::new()),
    };
    Ok(RegistrySnapshot {
        base_sha,
        base_branch,
        file_sha,
        content,
        new_package,
    })
}

fn ensure_fork(gh: &GhClient, user: &str) -> Result<String, String> {
    let fork_repo = format!("{user}/pit-registry");
    let upstream = request_json(
        gh.get(&format!("https://api.github.com/repos/{REGISTRY_REPO}")),
        "could not read registry repository",
    )?;
    let response = gh
        .get(&format!("https://api.github.com/repos/{fork_repo}"))
        .send()
        .map_err(|error| format!("could not inspect registry fork: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        request_json(
            gh.post(&format!(
                "https://api.github.com/repos/{REGISTRY_REPO}/forks"
            )),
            "could not create registry fork",
        )?;
    } else {
        let fork: Value = response
            .error_for_status()
            .map_err(|error| format!("could not inspect registry fork: {error}"))?
            .json()
            .map_err(|error| format!("invalid registry fork response: {error}"))?;
        validate_fork(&fork, &upstream, &fork_repo)?;
        return Ok(fork_repo);
    }
    for _ in 0..15 {
        let response = gh
            .get(&format!("https://api.github.com/repos/{fork_repo}"))
            .send()
            .map_err(|error| format!("could not inspect new registry fork: {error}"))?;
        if response.status().is_success() {
            let fork: Value = response
                .json()
                .map_err(|error| format!("invalid new registry fork response: {error}"))?;
            validate_fork(&fork, &upstream, &fork_repo)?;
            return Ok(fork_repo);
        }
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(format!(
                "could not inspect new registry fork: HTTP {}",
                response.status()
            ));
        }
        thread::sleep(Duration::from_secs(2));
    }
    Err(format!(
        "fork not ready after 30s; check https://github.com/{fork_repo} and retry"
    ))
}

fn validate_fork(fork: &Value, upstream: &Value, name: &str) -> Result<(), String> {
    let parent_id = fork["parent"]["id"].as_u64();
    if parent_id.is_none() || parent_id != upstream["id"].as_u64() {
        return Err(format!(
            "{name} exists but is not a fork of {REGISTRY_REPO}"
        ));
    }
    Ok(())
}

fn append_record(content: &str, record: &str) -> String {
    if content.is_empty() || content.ends_with('\n') {
        format!("{content}{record}")
    } else {
        format!("{content}\n{record}")
    }
}

fn update_file(
    gh: &GhClient,
    fork: &str,
    branch: &str,
    path: &str,
    content: &str,
    sha: Option<&str>,
    message: &str,
) -> Result<(), String> {
    let mut body = json!({"message": message, "content": B64.encode(content), "branch": branch});
    if let Some(sha) = sha {
        body["sha"] = json!(sha);
    }
    request_json(
        gh.put(&format!(
            "https://api.github.com/repos/{fork}/contents/{path}"
        ))
        .json(&body),
        &format!("could not update registry file {path}"),
    )?;
    Ok(())
}

pub(super) fn create_registry_pr(
    gh: &GhClient,
    pod: &PodVersion,
    source: &SourceRepository,
    snapshot: &RegistrySnapshot,
) -> Result<String, String> {
    let user = get_current_user(gh)?;
    let fork = ensure_fork(gh, &user)?;
    let branch = format!("add-{}-{}", pod.name, pod.vers);
    let ref_url = format!("https://api.github.com/repos/{fork}/git/refs/heads/{branch}");
    let response = gh
        .client
        .delete(&ref_url)
        .header("Authorization", format!("token {}", gh.token))
        .header("User-Agent", "pit/0.1.0")
        .send()
        .map_err(|error| format!("could not replace publish branch: {error}"))?;
    if !response.status().is_success()
        && response.status() != reqwest::StatusCode::NOT_FOUND
        && response.status() != reqwest::StatusCode::UNPROCESSABLE_ENTITY
    {
        return Err(format!(
            "could not replace publish branch: HTTP {}",
            response.status()
        ));
    }
    request_json(
        gh.post(&format!("https://api.github.com/repos/{fork}/git/refs"))
            .json(&json!({"ref": format!("refs/heads/{branch}"), "sha": snapshot.base_sha})),
        "could not create publish branch",
    )?;
    let prefix: String = pod.name.chars().take(2).collect();
    let line = serde_json::to_string(pod).map_err(|error| error.to_string())?;
    update_file(
        gh,
        &fork,
        &branch,
        &format!("{prefix}/{}", pod.name),
        &append_record(&snapshot.content, &line),
        snapshot.file_sha.as_deref(),
        &format!("add {}@{}", pod.name, pod.vers),
    )?;
    if snapshot.new_package {
        let ownership = serde_json::to_string_pretty(source).map_err(|error| error.to_string())?;
        update_file(
            gh,
            &fork,
            &branch,
            &format!("owners/{}.json", pod.name),
            &format!("{ownership}\n"),
            None,
            &format!("register {} repository", pod.name),
        )?;
    }
    let response = request_json(
        gh.post(&format!("https://api.github.com/repos/{REGISTRY_REPO}/pulls"))
            .json(&json!({
                "title": format!("Add {}@{}", pod.name, pod.vers),
                "body": format!("Publish **{}** version `{}` from `{}` (repository ID {}).", pod.name, pod.vers, source.repository, source.repository_id),
                "head": format!("{user}:{branch}"),
                "base": snapshot.base_branch,
            })),
        "could not create registry PR",
    )?;
    response["html_url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "registry PR response has no URL".to_string())
}

#[cfg(test)]
mod tests;
