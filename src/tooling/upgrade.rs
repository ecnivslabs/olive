use std::env;
use std::fs;

use flate2::read::GzDecoder;
use tar::Archive;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
fn get_repo() -> String {
    env::var("PIT_UPSTREAM_REPO").unwrap_or_else(|_| "ecnivs-labs/olive".to_string())
}

fn target_triple() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("pit-linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("pit-linux-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("pit-macos-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("pit-macos-aarch64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("pit-windows-x86_64.exe")
    } else {
        None
    }
}

fn target_lib_triple() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("libolive_std-linux-x86_64.so")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("libolive_std-linux-aarch64.so")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("libolive_std-macos-x86_64.dylib")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("libolive_std-macos-aarch64.dylib")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("libolive_std-windows-x86_64.dll")
    } else {
        None
    }
}

fn target_lib_file() -> Option<&'static str> {
    if cfg!(target_os = "linux") {
        Some("libolive_std.so")
    } else if cfg!(target_os = "macos") {
        Some("libolive_std.dylib")
    } else if cfg!(target_os = "windows") {
        Some("libolive_std.dll")
    } else {
        None
    }
}

fn fetch_latest_tag() -> Result<String, String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        get_repo()
    );
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", format!("pit/{}", CURRENT_VERSION))
        .send()
        .map_err(|e| format!("could not reach GitHub API: {}", e))?;

    let json: serde_json::Value = resp
        .json()
        .map_err(|e| format!("invalid API response: {}", e))?;

    json["tag_name"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "missing tag_name in release response".to_string())
}

fn download_artifact(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .header("User-Agent", format!("pit/{}", CURRENT_VERSION))
        .send()
        .map_err(|e| format!("download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("download failed with status: {}", resp.status()));
    }

    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("read failed: {}", e))
}

fn verify_blake3(buf: &[u8], filename: &str, checksums: &str) -> Result<(), String> {
    let mut expected = None;
    for line in checksums.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() == 2 && parts[1] == filename {
            expected = Some(parts[0]);
            break;
        }
    }
    let expected = expected.ok_or_else(|| format!("no checksum found for {}", filename))?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(buf);
    let hash = hasher.finalize().to_hex().to_string();

    if hash != expected {
        return Err(format!(
            "checksum mismatch for {}: expected {}, got {}",
            filename, expected, hash
        ));
    }
    Ok(())
}

pub fn upgrade() -> Result<(), String> {
    let artifact =
        target_triple().ok_or_else(|| "no prebuilt binary for this platform".to_string())?;

    let lib_artifact =
        target_lib_triple().ok_or_else(|| "no prebuilt stdlib for this platform".to_string())?;

    let lib_file =
        target_lib_file().ok_or_else(|| "no lib file format for this platform".to_string())?;

    let latest = fetch_latest_tag()?;
    let latest_ver = latest.trim_start_matches('v');

    let current_semver = semver::Version::parse(CURRENT_VERSION)
        .map_err(|e| format!("invalid current version: {}", e))?;
    let latest_semver =
        semver::Version::parse(latest_ver).map_err(|e| format!("invalid latest version: {}", e))?;

    if latest_semver <= current_semver {
        println!("Already on the latest version ({}).", CURRENT_VERSION);
        return Ok(());
    }

    if latest_semver.major > current_semver.major {
        return Err(format!(
            "refusing to upgrade across major version boundary ({} -> {}). manual upgrade required.",
            CURRENT_VERSION, latest_ver
        ));
    }

    println!("Upgrading {} -> {}...", CURRENT_VERSION, latest_ver);

    let client = reqwest::blocking::Client::new();
    let repo = get_repo();

    let sums_url = format!(
        "https://github.com/{}/releases/download/{}/checksums.txt",
        repo, latest
    );
    let sums_buf = download_artifact(&client, &sums_url)
        .map_err(|_| "missing checksums.txt in release".to_string())?;
    let checksums =
        String::from_utf8(sums_buf).map_err(|_| "invalid checksums.txt format".to_string())?;

    let bin_url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        repo, latest, artifact
    );
    let bin_buf = download_artifact(&client, &bin_url)?;
    verify_blake3(&bin_buf, artifact, &checksums)?;

    let lib_url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        repo, latest, lib_artifact
    );
    let lib_buf = download_artifact(&client, &lib_url)?;
    verify_blake3(&lib_buf, lib_artifact, &checksums)?;

    let src_artifact = "olive-src.tar.gz";
    let source_url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        repo, latest, src_artifact
    );
    let source_buf = download_artifact(&client, &source_url)?;
    verify_blake3(&source_buf, src_artifact, &checksums)?;

    let current_exe =
        env::current_exe().map_err(|e| format!("could not find current executable: {}", e))?;

    let tmp_path = current_exe.with_extension("tmp");
    fs::write(&tmp_path, &bin_buf).map_err(|e| format!("could not write temporary file: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("could not set permissions: {}", e))?;
    }

    let install_dir = current_exe
        .parent()
        .ok_or("no parent dir for current_exe")?;
    let lib_dir = install_dir
        .parent()
        .ok_or("no parent dir for install_dir")?
        .join("lib");
    let stdlib_src_dir = lib_dir.join("olive");

    fs::create_dir_all(&lib_dir).map_err(|e| format!("could not create lib directory: {}", e))?;

    let lib_path = lib_dir.join(lib_file);
    let lib_tmp = lib_path.with_extension("tmp");
    fs::write(&lib_tmp, &lib_buf).map_err(|e| format!("could not write lib tmp file: {}", e))?;

    let stdlib_tmp_dir = stdlib_src_dir.with_extension("tmp");
    let _ = fs::remove_dir_all(&stdlib_tmp_dir);
    fs::create_dir_all(&stdlib_tmp_dir)
        .map_err(|e| format!("could not create stdlib tmp dir: {}", e))?;

    let tar = GzDecoder::new(source_buf.as_slice());
    let mut archive = Archive::new(tar);

    for file in archive
        .entries()
        .map_err(|e| format!("failed to read tar entries: {}", e))?
    {
        if let Ok(mut file) = file
            && let Ok(path) = file.path()
        {
            let mut components = path.components();
            components.next();
            if let Some(std::path::Component::Normal(comp)) = components.next()
                && comp == "lib"
            {
                let relative_path = components.as_path();
                if relative_path.as_os_str().is_empty() {
                    continue;
                }

                let target_path = stdlib_tmp_dir.join(relative_path);

                if file.header().entry_type().is_dir() {
                    let _ = fs::create_dir_all(&target_path);
                } else {
                    if let Some(parent) = target_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = file.unpack(&target_path);
                }
            }
        }
    }

    let old_stdlib_dir = stdlib_src_dir.with_extension("old");
    let _ = fs::remove_dir_all(&old_stdlib_dir);
    if stdlib_src_dir.exists() {
        fs::rename(&stdlib_src_dir, &old_stdlib_dir)
            .map_err(|e| format!("could not move old stdlib dir: {}", e))?;
    }
    fs::rename(&stdlib_tmp_dir, &stdlib_src_dir).map_err(|e| {
        let _ = fs::rename(&old_stdlib_dir, &stdlib_src_dir);
        format!("could not swap stdlib dir: {}", e)
    })?;

    let old_lib_path = lib_path.with_extension("old");
    let _ = fs::remove_file(&old_lib_path);
    if lib_path.exists() {
        fs::rename(&lib_path, &old_lib_path)
            .map_err(|e| format!("could not move old lib file: {}", e))?;
    }
    fs::rename(&lib_tmp, &lib_path).map_err(|e| {
        let _ = fs::rename(&old_lib_path, &lib_path);
        format!("could not swap lib file: {}", e)
    })?;

    #[cfg(windows)]
    {
        let old_exe = current_exe.with_extension("old");
        let _ = fs::remove_file(&old_exe);
        fs::rename(&current_exe, &old_exe)
            .map_err(|e| format!("could not move current binary: {}", e))?;
    }
    fs::rename(&tmp_path, &current_exe).map_err(|e| format!("could not replace binary: {}", e))?;

    let _ = fs::remove_dir_all(&old_stdlib_dir);
    let _ = fs::remove_file(&old_lib_path);

    // Clean up legacy/shadowing library in bin directory if present
    let bin_lib_path = install_dir.join(lib_file);
    if bin_lib_path.exists() && bin_lib_path != lib_path {
        let _ = fs::remove_file(&bin_lib_path);
    }

    println!("Updated to {}.", latest_ver);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_triple_returns_some() {
        assert!(target_triple().is_some());
    }

    #[test]
    fn target_triple_format() {
        let triple = target_triple().unwrap();
        assert!(triple.starts_with("pit-"));
    }

    #[test]
    fn target_lib_triple_returns_some() {
        assert!(target_lib_triple().is_some());
    }

    #[test]
    fn target_lib_triple_format() {
        let triple = target_lib_triple().unwrap();
        assert!(triple.starts_with("libolive_std-"));
    }

    #[test]
    fn target_lib_file_returns_some() {
        assert!(target_lib_file().is_some());
    }

    #[test]
    fn target_lib_file_format() {
        let file = target_lib_file().unwrap();
        assert!(file.starts_with("libolive_std"));
    }

    #[test]
    fn verify_blake3_matching() {
        let data = b"hello world";
        let mut h = blake3::Hasher::new();
        h.update(data);
        let hash = h.finalize().to_hex().to_string();
        let checksums = format!("{hash}  file.txt");
        assert!(verify_blake3(data, "file.txt", &checksums).is_ok());
    }

    #[test]
    fn verify_blake3_mismatch() {
        let data = b"hello world";
        let checksums =
            "0000000000000000000000000000000000000000000000000000000000000000  file.txt";
        let result = verify_blake3(data, "file.txt", checksums);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("checksum mismatch"));
    }

    #[test]
    fn verify_blake3_missing_filename() {
        let data = b"hello world";
        let checksums =
            "0000000000000000000000000000000000000000000000000000000000000000  other.txt";
        let result = verify_blake3(data, "file.txt", checksums);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no checksum found"));
    }

    #[test]
    fn verify_blake3_multiple_entries() {
        let data = b"data";
        let mut h = blake3::Hasher::new();
        h.update(data);
        let hash = h.finalize().to_hex().to_string();
        let checksums = format!("aaa  a.bin\n{hash}  file.txt\nbbb  b.bin");
        assert!(verify_blake3(data, "file.txt", &checksums).is_ok());
    }

    #[test]
    fn verify_blake3_skips_empty_lines() {
        let data = b"data";
        let mut h = blake3::Hasher::new();
        h.update(data);
        let hash = h.finalize().to_hex().to_string();
        let checksums = format!("\n\n{hash}  file.txt\n\n");
        assert!(verify_blake3(data, "file.txt", &checksums).is_ok());
    }
}
