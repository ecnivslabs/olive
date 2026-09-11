use crate::tooling::registry::PodVersion;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum InstallError {
    Download(String),
    Checksum(String),
    Extraction(String),
    Io(std::io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            InstallError::Download(msg) => write!(f, "download failed: {}", msg),
            InstallError::Checksum(msg) => write!(f, "checksum mismatch: {}", msg),
            InstallError::Extraction(msg) => write!(f, "extraction failed: {}", msg),
            InstallError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}
impl std::error::Error for InstallError {}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Io(e)
    }
}

/// Decompresses and extracts a `.pit.zst` archive into `dest_dir`.
///
/// Pods are downloaded from URLs published in registry entries that reach the
/// registry through a reviewed pull request, but the archive itself is untrusted
/// content once it leaves that review: a malicious or buggy publish could still
/// craft an entry that tries to escape `dest_dir` (`..` components), overwrite
/// arbitrary files through a symlink, or plant a `native/` directory to race the
/// separately-verified native artifact download. All three are rejected here
/// before any file is written.
fn extract_pod_archive(compressed_data: &[u8], dest_dir: &Path) -> Result<(), InstallError> {
    let decompressed =
        zstd::decode_all(compressed_data).map_err(|e| InstallError::Extraction(e.to_string()))?;
    let mut archive = tar::Archive::new(decompressed.as_slice());

    for entry in archive
        .entries()
        .map_err(|e| InstallError::Extraction(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| InstallError::Extraction(e.to_string()))?;
        let raw_path = entry
            .path()
            .map_err(|e| InstallError::Extraction(e.to_string()))?
            .into_owned();

        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(InstallError::Extraction(format!(
                "archive entry '{}' has type {:?}; pods may contain only regular files and directories",
                raw_path.display(),
                kind
            )));
        }

        let stripped: PathBuf = raw_path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        for component in stripped.components() {
            if !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            ) {
                return Err(InstallError::Extraction(format!(
                    "archive entry '{}' escapes the pod directory",
                    raw_path.display()
                )));
            }
        }
        if stripped.starts_with("native") {
            return Err(InstallError::Extraction(
                "archive contains a 'native/' directory; native artifacts are published \
                 as separate release assets and may not be packed into the .pit.zst"
                    .to_string(),
            ));
        }

        let dest = dest_dir.join(&stripped);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.set_preserve_permissions(false);
        entry
            .unpack(&dest)
            .map_err(|e| InstallError::Extraction(e.to_string()))?;
    }
    Ok(())
}

pub async fn install_pod_atomic(pod: &PodVersion, final_dir: PathBuf) -> Result<(), InstallError> {
    if final_dir.exists() {
        return Ok(());
    }

    println!("\x1b[1;32m  Downloading\x1b[0m {}@{}", pod.name, pod.vers);

    let pods_base = final_dir.parent().unwrap().parent().unwrap();
    let mut rng = rand::rng();
    use rand::Rng;
    let tmp_dir = pods_base.join(format!(".tmp-{:x}", rng.next_u64()));
    fs::create_dir_all(&tmp_dir)?;

    let client = reqwest::Client::new();
    let mut resp = client
        .get(&pod.dl)
        .header("User-Agent", "pit/0.1.0")
        .send()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?;

    if !resp.status().is_success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Download(format!("HTTP {}", resp.status())));
    }

    let mut hasher = blake3::Hasher::new();
    let mut compressed_data = Vec::new();

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?
    {
        hasher.update(&chunk);
        compressed_data.extend_from_slice(&chunk);
    }

    let cksum = hasher.finalize().to_hex().to_string();
    if cksum != pod.cksum {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Checksum(format!(
            "expected {}, got {}",
            pod.cksum, cksum
        )));
    }

    let tmp_dir_clone = tmp_dir.clone();

    tokio::task::spawn_blocking(move || -> Result<(), InstallError> {
        extract_pod_archive(&compressed_data, &tmp_dir_clone)
    })
    .await
    .map_err(|e| InstallError::Extraction(format!("task panicked: {}", e)))??;

    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::rename(&tmp_dir, &final_dir) {
        Ok(_) => {
            println!("\x1b[1;32m  Installed\x1b[0m {}@{}", pod.name, pod.vers);
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&tmp_dir);
            if !final_dir.exists() {
                return Err(InstallError::Io(e));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tooling::registry::PodVersion;

    #[test]
    fn install_error_download_display() {
        let e = InstallError::Download("timeout".to_string());
        assert_eq!(format!("{e}"), "download failed: timeout");
    }

    #[test]
    fn install_error_checksum_display() {
        let e = InstallError::Checksum("expected abc, got def".to_string());
        assert_eq!(format!("{e}"), "checksum mismatch: expected abc, got def");
    }

    #[test]
    fn install_error_extraction_display() {
        let e = InstallError::Extraction("corrupt archive".to_string());
        assert_eq!(format!("{e}"), "extraction failed: corrupt archive");
    }

    #[test]
    fn install_error_io_display() {
        let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e = InstallError::Io(inner);
        assert!(format!("{e}").contains("I/O error"));
    }

    #[test]
    fn install_error_from_io() {
        let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let e: InstallError = inner.into();
        assert!(matches!(e, InstallError::Io(_)));
    }

    #[test]
    fn install_error_impl_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<InstallError>();
    }

    #[test]
    fn install_pod_atomic_returns_ok_when_dir_exists() {
        let dir = std::env::temp_dir().join("olive_install_test_exists");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let pod = PodVersion {
            name: "test".to_string(),
            vers: "1.0.0".to_string(),
            deps: vec![],
            cksum: String::new(),
            dl: String::new(),
            yanked: false,
            olive_req: None,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(install_pod_atomic(&pod, dir.clone()));
        assert!(result.is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    fn build_test_archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (path, kind, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(*kind);
                header.set_path(path).unwrap();
                header.set_size(data.len() as u64);
                header.set_mode(if kind.is_dir() { 0o755 } else { 0o644 });
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            }
            builder.finish().unwrap();
        }
        zstd::encode_all(tar_bytes.as_slice(), 3).unwrap()
    }

    /// `tar::Header::set_path` refuses to encode a `..` component at all, so a
    /// traversal attempt has to be built by writing the raw fixed-size `name`
    /// field directly, exactly as a hand-crafted malicious archive would.
    fn archive_with_raw_name(raw_name: &str, data: &[u8]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_old();
            header.set_entry_type(tar::EntryType::Regular);
            let name_field = &mut header.as_old_mut().name;
            let bytes = raw_name.as_bytes();
            name_field[..bytes.len()].copy_from_slice(bytes);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, data).unwrap();
            builder.finish().unwrap();
        }
        zstd::encode_all(tar_bytes.as_slice(), 3).unwrap()
    }

    #[test]
    fn extract_pod_archive_rejects_parent_dir_traversal() {
        let archive = archive_with_raw_name("pkg-1.0/../../../../etc/evil", b"malicious");
        let dest = std::env::temp_dir().join("olive_extract_test_traversal");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(_))));
        assert!(!dest.parent().unwrap().join("etc").exists());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_hardlink_entries() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Link);
            header.set_path("pkg-1.0/src/evil-hardlink").unwrap();
            header.set_link_name("/etc/passwd").unwrap();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
            builder.finish().unwrap();
        }
        let archive = zstd::encode_all(tar_bytes.as_slice(), 3).unwrap();
        let dest = std::env::temp_dir().join("olive_extract_test_hardlink");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(_))));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_symlink_entries() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_path("pkg-1.0/src/evil-link").unwrap();
            header.set_link_name("/etc/passwd").unwrap();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
            builder.finish().unwrap();
        }
        let archive = zstd::encode_all(tar_bytes.as_slice(), 3).unwrap();
        let dest = std::env::temp_dir().join("olive_extract_test_symlink");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(_))));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_native_directory() {
        let archive = build_test_archive(&[(
            "pkg-1.0/native/libtokenizer.so",
            tar::EntryType::Regular,
            b"not a real library",
        )]);
        let dest = std::env::temp_dir().join("olive_extract_test_native");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(msg)) if msg.contains("native/")));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_accepts_well_formed_pod() {
        let archive = build_test_archive(&[
            ("pkg-1.0/pit.toml", tar::EntryType::Regular, b"[pod]\n"),
            ("pkg-1.0/src", tar::EntryType::Directory, b""),
            (
                "pkg-1.0/src/lib.liv",
                tar::EntryType::Regular,
                b"fn main():\n    pass\n",
            ),
        ]);
        let dest = std::env::temp_dir().join("olive_extract_test_wellformed");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(result.is_ok(), "{:?}", result);
        assert!(dest.join("pit.toml").is_file());
        assert!(dest.join("src/lib.liv").is_file());
        let _ = fs::remove_dir_all(&dest);
    }
}
