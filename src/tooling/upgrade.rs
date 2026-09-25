use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;

use crate::tooling::target;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_SOURCE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SOURCE_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_RELEASE_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
fn get_repo() -> String {
    env::var("PIT_UPSTREAM_REPO").unwrap_or_else(|_| "ecnivslabs/olive".to_string())
}

/// Release asset name for the `pit` binary itself on this host, e.g.
/// `pit-linux-x86_64` or `pit-windows-x86_64.exe`. Distinct from
/// `target::asset_name`, which names a shared library, not an executable.
fn target_triple() -> Option<String> {
    let key = target::host()?;
    Some(if key == "windows-x86_64" {
        format!("pit-{key}.exe")
    } else {
        format!("pit-{key}")
    })
}

fn target_lib_triple() -> Option<String> {
    target::asset_name("olive_std", target::host()?)
}

fn target_lib_file() -> Option<String> {
    target::local_name("olive_std")
}

fn target_static_asset() -> Option<String> {
    target::static_asset_name("olive_std", target::host()?)
}

fn target_static_file() -> Option<String> {
    target::local_static_name("olive_std", target::host()?)
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

    if resp
        .content_length()
        .is_some_and(|size| size > MAX_RELEASE_DOWNLOAD_BYTES)
    {
        return Err("download exceeds the 1 GiB artifact limit".to_string());
    }
    let mut limited = resp.take(MAX_RELEASE_DOWNLOAD_BYTES + 1);
    let mut data = Vec::new();
    limited
        .read_to_end(&mut data)
        .map_err(|e| format!("read failed: {e}"))?;
    if data.len() as u64 > MAX_RELEASE_DOWNLOAD_BYTES {
        return Err("download exceeds the 1 GiB artifact limit".to_string());
    }
    Ok(data)
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

fn stdlib_relative_path(path: &Path) -> Result<Option<PathBuf>, String> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(name)) if name == OsStr::new("olive-src"))
    {
        return Ok(None);
    }
    if !matches!(components.next(), Some(Component::Normal(name)) if name == OsStr::new("lib")) {
        return Ok(None);
    }

    let mut relative = PathBuf::new();
    for component in components {
        let Component::Normal(name) = component else {
            return Err(format!("unsafe path in source archive: {}", path.display()));
        };
        if !crate::tooling::safe_archive_component(name) {
            return Err(format!("unsafe path in source archive: {}", path.display()));
        }
        relative.push(name);
    }

    if relative.as_os_str().is_empty() {
        Ok(None)
    } else {
        Ok(Some(relative))
    }
}

fn extract_stdlib_archive(source: &[u8], destination: &Path) -> Result<(), String> {
    let destination = fs::canonicalize(destination)
        .map_err(|error| format!("failed to open extraction directory: {error}"))?;
    let decoder = GzDecoder::new(Cursor::new(source));
    let mut limited = decoder.take(MAX_SOURCE_ARCHIVE_BYTES + 1);
    let mut decompressed = Vec::new();
    limited
        .read_to_end(&mut decompressed)
        .map_err(|error| format!("failed to decompress source archive: {error}"))?;
    if decompressed.len() as u64 > MAX_SOURCE_ARCHIVE_BYTES {
        return Err(format!(
            "source archive exceeds the {MAX_SOURCE_ARCHIVE_BYTES} byte decompressed limit"
        ));
    }
    let mut archive = Archive::new(Cursor::new(decompressed));
    let entries = archive
        .entries()
        .map_err(|error| format!("failed to read tar entries: {error}"))?;
    let mut extracted_files = 0usize;
    let mut entry_count = 0usize;
    let mut seen = std::collections::HashSet::new();

    for entry in entries {
        entry_count += 1;
        if entry_count > MAX_SOURCE_ARCHIVE_ENTRIES {
            return Err(format!(
                "source archive exceeds the {MAX_SOURCE_ARCHIVE_ENTRIES} entry limit"
            ));
        }
        let mut entry = entry.map_err(|error| format!("failed to read tar entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("invalid tar entry path: {error}"))?;
        let Some(relative) = stdlib_relative_path(&path)? else {
            continue;
        };
        if !seen.insert(relative.clone()) {
            return Err(format!(
                "source archive contains duplicate entry '{}'",
                path.display()
            ));
        }
        let entry_type = entry.header().entry_type();
        if !entry_type.is_dir() && !entry_type.is_file() && !entry_type.is_contiguous() {
            return Err(format!(
                "unsupported entry type in source archive: {}",
                path.display()
            ));
        }
        let target = destination.join(relative);

        if entry_type.is_dir() {
            fs::create_dir_all(&target).map_err(|error| {
                format!(
                    "failed to create source directory {}: {error}",
                    target.display()
                )
            })?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "failed to create source directory {}: {error}",
                        parent.display()
                    )
                })?;
                let canonical_parent = fs::canonicalize(parent).map_err(|error| {
                    format!(
                        "failed to resolve source directory {}: {error}",
                        parent.display()
                    )
                })?;
                if !canonical_parent.starts_with(&destination) {
                    return Err(format!(
                        "source archive path escapes extraction directory: {}",
                        path.display()
                    ));
                }
            }
            if fs::symlink_metadata(&target)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err(format!(
                    "source archive path is a symlink: {}",
                    path.display()
                ));
            }
            entry
                .unpack(&target)
                .map_err(|error| format!("failed to unpack {}: {error}", target.display()))?;
            extracted_files += 1;
        }
    }

    if extracted_files == 0 {
        return Err("source archive contains no regular files under olive-src/lib".to_string());
    }
    Ok(())
}

struct InstalledAsset {
    destination: PathBuf,
    backup: Option<PathBuf>,
}

fn staging_path(destination: &Path) -> Result<PathBuf, String> {
    static NEXT_STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = destination
        .file_name()
        .expect("release asset path has a filename")
        .to_os_string();
    let directory = destination
        .parent()
        .expect("release asset path has a parent");
    loop {
        let sequence = NEXT_STAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut candidate_name = name.clone();
        candidate_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
        let candidate = directory.join(candidate_name);
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => {
                return Err(format!(
                    "could not inspect staging path {}: {error}",
                    candidate.display()
                ));
            }
            Ok(_) => {}
        }
    }
}

struct StagingCleanup(Vec<PathBuf>);

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            match fs::symlink_metadata(path) {
                Ok(_) => {
                    if let Err(error) = remove_asset(path) {
                        eprintln!("Could not remove staging path {}: {error}", path.display());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    eprintln!("Could not inspect staging path {}: {error}", path.display());
                }
            }
        }
    }
}

fn remove_asset(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn backup_path(destination: &Path) -> Result<PathBuf, String> {
    static NEXT_BACKUP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let sequence = NEXT_BACKUP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let extension = format!("old-{}-{sequence}", std::process::id());
        let candidate = destination.with_extension(extension);
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => {
                return Err(format!(
                    "could not inspect backup path {}: {error}",
                    candidate.display()
                ));
            }
            Ok(_) => {}
        }
    }
}

fn restore_asset(asset: &InstalledAsset) -> Result<(), String> {
    remove_asset(&asset.destination).map_err(|error| {
        format!(
            "could not remove replacement {}: {error}",
            asset.destination.display()
        )
    })?;
    if let Some(backup) = &asset.backup {
        fs::rename(backup, &asset.destination).map_err(|error| {
            format!(
                "could not restore {} from {}: {error}",
                asset.destination.display(),
                backup.display()
            )
        })?;
    }
    Ok(())
}

fn install_asset(staged: &Path, destination: &Path) -> Result<InstalledAsset, String> {
    let destination_exists = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "could not inspect {}: {error}",
                destination.display()
            ));
        }
    };
    let backup = if destination_exists {
        let backup = backup_path(destination)?;
        fs::rename(destination, &backup)
            .map_err(|error| format!("could not move {} aside: {error}", destination.display()))?;
        Some(backup)
    } else {
        None
    };
    if let Err(error) = fs::rename(staged, destination) {
        let restore_error = backup
            .as_ref()
            .and_then(|path| fs::rename(path, destination).err());
        let mut message = format!("could not install {}: {error}", destination.display());
        if let Some(restore_error) = restore_error {
            message.push_str(&format!("; could not restore old asset: {restore_error}"));
        }
        return Err(message);
    }
    Ok(InstalledAsset {
        destination: destination.to_path_buf(),
        backup,
    })
}

fn install_assets(assets: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut installed = Vec::with_capacity(assets.len());
    for (staged, destination) in assets {
        match install_asset(staged, destination) {
            Ok(asset) => installed.push(asset),
            Err(mut message) => {
                let failures: Vec<String> = installed
                    .iter()
                    .rev()
                    .filter_map(|asset| restore_asset(asset).err())
                    .collect();
                if !failures.is_empty() {
                    message.push_str(&format!("; rollback failed: {}", failures.join("; ")));
                }
                return Err(message);
            }
        }
    }
    for asset in installed {
        if let Some(backup) = asset.backup
            && let Err(error) = remove_asset(&backup)
        {
            eprintln!(
                "Upgrade completed, but could not remove backup {}: {error}",
                backup.display()
            );
        }
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
    verify_blake3(&bin_buf, &artifact, &checksums)?;

    let lib_url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        repo, latest, lib_artifact
    );
    let lib_buf = download_artifact(&client, &lib_url)?;
    verify_blake3(&lib_buf, &lib_artifact, &checksums)?;

    let static_asset = target_static_asset();
    let static_file = target_static_file();
    let static_buf = if let (Some(static_asset), Some(_)) = (&static_asset, &static_file) {
        let static_url = format!(
            "https://github.com/{}/releases/download/{}/{}",
            repo, latest, static_asset
        );
        let buf = download_artifact(&client, &static_url)?;
        verify_blake3(&buf, static_asset, &checksums)?;
        Some(buf)
    } else {
        None
    };

    let src_artifact = "olive-src.tar.gz";
    let source_url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        repo, latest, src_artifact
    );
    let source_buf = download_artifact(&client, &source_url)?;
    verify_blake3(&source_buf, src_artifact, &checksums)?;

    let current_exe =
        env::current_exe().map_err(|e| format!("could not find current executable: {}", e))?;

    let mut staging_cleanup = StagingCleanup(Vec::new());
    let tmp_path = staging_path(&current_exe)?;
    staging_cleanup.0.push(tmp_path.clone());
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

    let lib_path = lib_dir.join(&lib_file);
    let lib_tmp = staging_path(&lib_path)?;
    staging_cleanup.0.push(lib_tmp.clone());
    fs::write(&lib_tmp, &lib_buf).map_err(|e| format!("could not write lib tmp file: {}", e))?;

    let static_tmp = static_file
        .as_ref()
        .map(|file| staging_path(&lib_dir.join(file)))
        .transpose()?;
    if let Some(path) = &static_tmp {
        staging_cleanup.0.push(path.clone());
    }
    if let (Some(path), Some(buf)) = (&static_tmp, &static_buf) {
        fs::write(path, buf).map_err(|e| format!("could not write static lib tmp file: {}", e))?;
    }

    let stdlib_tmp_dir = staging_path(&stdlib_src_dir)?;
    staging_cleanup.0.push(stdlib_tmp_dir.clone());
    fs::create_dir_all(&stdlib_tmp_dir)
        .map_err(|e| format!("could not create stdlib tmp dir: {}", e))?;

    extract_stdlib_archive(&source_buf, &stdlib_tmp_dir)?;

    let mut assets = vec![
        (stdlib_tmp_dir, stdlib_src_dir),
        (lib_tmp, lib_path.clone()),
    ];
    if let (Some(static_tmp), Some(static_path)) = (
        static_tmp,
        static_file.as_ref().map(|file| lib_dir.join(file)),
    ) {
        assets.push((static_tmp, static_path));
    }
    assets.push((tmp_path, current_exe.clone()));
    install_assets(&assets)?;

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
    use flate2::{Compression, write::GzEncoder};
    use std::io::empty;
    use tar::{Builder, EntryType, Header};

    fn source_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let encoder = GzEncoder::new(&mut bytes, Compression::default());
            let mut archive = Builder::new(encoder);
            for (path, contents) in entries {
                let mut header = Header::new_gnu();
                header.set_size(contents.len() as u64);
                header.set_mode(0o644);
                let path_bytes = path.as_bytes();
                assert!(path_bytes.len() <= 100);
                header.as_old_mut().name.fill(0);
                header.as_old_mut().name[..path_bytes.len()].copy_from_slice(path_bytes);
                header.set_cksum();
                archive.append(&header, *contents).unwrap();
            }
            let encoder = archive.into_inner().unwrap();
            encoder.finish().unwrap();
        }
        bytes
    }

    fn symlink_archive(path: &str, target: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let encoder = GzEncoder::new(&mut bytes, Compression::default());
            let mut archive = Builder::new(encoder);
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::symlink());
            header.set_mode(0o777);
            header.set_path(path).unwrap();
            header.set_link_name(target).unwrap();
            header.set_cksum();
            archive.append(&header, empty()).unwrap();
            let encoder = archive.into_inner().unwrap();
            encoder.finish().unwrap();
        }
        bytes
    }

    fn source_archive_with_outside_link() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let encoder = GzEncoder::new(&mut bytes, Compression::default());
            let mut archive = Builder::new(encoder);
            let mut file_header = Header::new_gnu();
            file_header.set_path("olive-src/lib/value.txt").unwrap();
            file_header.set_size(5);
            file_header.set_mode(0o644);
            file_header.set_cksum();
            archive.append(&file_header, &b"value"[..]).unwrap();
            let mut link_header = Header::new_gnu();
            link_header.set_entry_type(EntryType::symlink());
            link_header.set_path("docs/link").unwrap();
            link_header.set_link_name("/tmp/outside").unwrap();
            link_header.set_size(0);
            link_header.set_mode(0o777);
            link_header.set_cksum();
            archive.append(&link_header, empty()).unwrap();
            let encoder = archive.into_inner().unwrap();
            encoder.finish().unwrap();
        }
        bytes
    }

    fn test_case(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("olive-upgrade-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn failed_asset_swap_restores_previous_installation() {
        let case = test_case("rollback");
        let old_sources = case.join("sources");
        let staged_sources = case.join("staged-sources");
        fs::create_dir_all(&old_sources).unwrap();
        fs::create_dir_all(&staged_sources).unwrap();
        fs::write(old_sources.join("value"), "old").unwrap();
        fs::write(staged_sources.join("value"), "new").unwrap();
        let library = case.join("library");
        fs::write(&library, "old library").unwrap();

        let result = install_assets(&[
            (staged_sources, old_sources.clone()),
            (case.join("missing-library"), library.clone()),
        ]);

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(old_sources.join("value")).unwrap(),
            "old"
        );
        assert_eq!(fs::read_to_string(&library).unwrap(), "old library");
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn successful_asset_swap_removes_backups() {
        let case = test_case("swap");
        let old = case.join("library");
        let staged = case.join("staged-library");
        fs::write(&old, "old").unwrap();
        fs::write(&staged, "new").unwrap();

        install_assets(&[(staged, old.clone())]).unwrap();

        assert_eq!(fs::read_to_string(&old).unwrap(), "new");
        assert_eq!(fs::read_dir(&case).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn windows_library_staging_paths_are_distinct() {
        let directory = Path::new("lib");
        let dynamic = staging_path(&directory.join("olive_std.dll")).unwrap();
        let static_library = staging_path(&directory.join("olive_std.lib")).unwrap();
        assert_ne!(dynamic, static_library);
        assert!(
            dynamic
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("olive_std.dll.tmp-")
        );
        assert!(
            static_library
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("olive_std.lib.tmp-")
        );
    }

    #[test]
    fn staging_cleanup_removes_unconsumed_assets() {
        let case = test_case("staging-cleanup");
        let staged_file = staging_path(&case.join("binary")).unwrap();
        let staged_dir = staging_path(&case.join("sources")).unwrap();
        fs::write(&staged_file, "binary").unwrap();
        fs::create_dir_all(&staged_dir).unwrap();
        fs::write(staged_dir.join("source"), "source").unwrap();

        drop(StagingCleanup(vec![
            staged_file.clone(),
            staged_dir.clone(),
        ]));

        assert!(!staged_file.exists());
        assert!(!staged_dir.exists());
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn source_archive_rejects_parent_traversal() {
        let case = test_case("traversal");
        let destination = case.join("extract");
        fs::create_dir_all(&destination).unwrap();
        let archive = source_archive(&[("olive-src/lib/../escaped.txt", b"pwned")]);

        let result = extract_stdlib_archive(&archive, &destination);

        assert!(result.is_err());
        assert!(!case.join("escaped.txt").exists());
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn source_archive_rejects_links() {
        let case = test_case("link");
        let destination = case.join("extract");
        fs::create_dir_all(&destination).unwrap();
        let archive = symlink_archive("olive-src/lib/link", "/tmp/outside");

        let result = extract_stdlib_archive(&archive, &destination);

        assert!(result.is_err());
        assert!(!destination.join("link").exists());
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn source_archive_extracts_only_stdlib_files() {
        let case = test_case("valid");
        let destination = case.join("extract");
        fs::create_dir_all(&destination).unwrap();
        let archive = source_archive(&[
            ("olive-src/README.md", b"readme"),
            ("olive-src/lib/nested/value.txt", b"value"),
        ]);

        extract_stdlib_archive(&archive, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("nested/value.txt")).unwrap(),
            "value"
        );
        assert!(!destination.join("README.md").exists());
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn source_archive_rejects_wrong_root_and_empty_lib() {
        let wrong_root = source_archive(&[("attacker/lib/value.txt", b"value")]);
        let empty_lib = source_archive(&[("olive-src/README.md", b"readme")]);
        for archive in [wrong_root, empty_lib] {
            let case = test_case("invalid-root");
            let destination = case.join("extract");
            fs::create_dir_all(&destination).unwrap();
            assert!(extract_stdlib_archive(&archive, &destination).is_err());
            let _ = fs::remove_dir_all(&case);
        }
    }

    #[test]
    fn source_archive_ignores_links_outside_stdlib() {
        let case = test_case("outside-link");
        let destination = case.join("extract");
        fs::create_dir_all(&destination).unwrap();
        let archive = source_archive_with_outside_link();
        extract_stdlib_archive(&archive, &destination).unwrap();
        assert!(destination.join("value.txt").is_file());
        assert!(!destination.join("link").exists());
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn source_archive_rejects_windows_special_components() {
        let case = test_case("windows-component");
        let destination = case.join("extract");
        fs::create_dir_all(&destination).unwrap();
        let archive = source_archive(&[("olive-src/lib/trailing. ", b"value")]);
        assert!(extract_stdlib_archive(&archive, &destination).is_err());
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn source_archive_rejects_duplicate_lib_paths() {
        let case = test_case("duplicate-lib");
        let destination = case.join("extract");
        fs::create_dir_all(&destination).unwrap();
        let archive = source_archive(&[
            ("olive-src/lib/value.txt", b"first"),
            ("olive-src/lib/value.txt", b"second"),
        ]);
        assert!(extract_stdlib_archive(&archive, &destination).is_err());
        let _ = fs::remove_dir_all(&case);
    }

    #[test]
    fn source_archive_rejects_corrupt_gzip_trailer() {
        let case = test_case("corrupt-gzip");
        let destination = case.join("extract");
        fs::create_dir_all(&destination).unwrap();
        let mut archive = source_archive(&[("olive-src/lib/value.txt", b"value")]);
        let last = archive.len() - 1;
        archive[last] ^= 0xff;
        assert!(extract_stdlib_archive(&archive, &destination).is_err());
        let _ = fs::remove_dir_all(&case);
    }

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
        let expected = if cfg!(target_os = "windows") {
            "olive_std.dll"
        } else if cfg!(target_os = "macos") {
            "libolive_std.dylib"
        } else {
            "libolive_std.so"
        };
        assert_eq!(file, expected);
    }

    #[test]
    fn target_static_names_match_release_layout() {
        assert_eq!(
            target::static_asset_name("olive_std", "windows-x86_64").unwrap(),
            "olive_std-windows-x86_64.lib"
        );
        assert_eq!(
            target::local_static_name("olive_std", "windows-x86_64").unwrap(),
            "olive_std.lib"
        );
        if let Some(key) = target::host() {
            assert_eq!(
                target_static_asset(),
                target::static_asset_name("olive_std", key)
            );
        }
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
