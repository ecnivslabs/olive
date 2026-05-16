use std::env;
use std::fs;
use std::io::Read;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "olive-language/olive";

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

fn fetch_latest_tag() -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", REPO);
    let resp = ureq::get(&url)
        .set("User-Agent", &format!("pit/{}", CURRENT_VERSION))
        .call()
        .map_err(|e| format!("could not reach GitHub API: {}", e))?;

    let json: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("invalid API response: {}", e))?;

    json["tag_name"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "missing tag_name in release response".to_string())
}

pub fn upgrade() -> Result<(), String> {
    let artifact =
        target_triple().ok_or_else(|| "no prebuilt binary for this platform".to_string())?;

    let latest = fetch_latest_tag()?;
    let latest_ver = latest.trim_start_matches('v');

    if latest_ver == CURRENT_VERSION {
        println!("Already on the latest version ({}).", CURRENT_VERSION);
        return Ok(());
    }

    println!("Upgrading {} -> {}...", CURRENT_VERSION, latest_ver);

    let url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        REPO, latest, artifact
    );

    let mut buf = Vec::new();
    ureq::get(&url)
        .set("User-Agent", &format!("pit/{}", CURRENT_VERSION))
        .call()
        .map_err(|e| format!("download failed: {}", e))?
        .into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {}", e))?;

    let current_exe =
        env::current_exe().map_err(|e| format!("could not find current executable: {}", e))?;

    let tmp_path = current_exe.with_extension("tmp");
    fs::write(&tmp_path, &buf).map_err(|e| format!("could not write temporary file: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("could not set permissions: {}", e))?;
    }

    fs::rename(&tmp_path, &current_exe).map_err(|e| format!("could not replace binary: {}", e))?;

    println!("Updated to {}.", latest_ver);
    Ok(())
}
